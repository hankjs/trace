"""AkShare 基本面与估值数据采集。

单股接口失败只影响该股票；没有可靠公告日期的降级数据以抓取日作为
available_date，宁可少参与历史筛选，也不把后来获得的数据泄漏到过去。
"""
from __future__ import annotations

import logging
import threading
from datetime import date, datetime
from typing import Any

import akshare as ak
import pandas as pd
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import (FundamentalSnapshot, IndexMember, Stock,
                      ValuationSnapshot, WatchlistItem)
from . import akshare_client
from .universe import current_pool

logger = logging.getLogger(__name__)

VALUATION_FIELDS = (
    "pe_ttm", "pb", "ps_ttm", "dividend_yield", "total_market_cap",
)
FUNDAMENTAL_FIELDS = (
    "roe", "revenue_yoy", "profit_yoy", "gross_margin", "net_margin",
    "debt_ratio", "cashflow_ratio",
)

_SYNC_LOCK = threading.Lock()


class FundamentalSyncInProgressError(RuntimeError):
    """同一进程中已有估值或财务同步任务。"""


def _as_float(value: Any) -> float | None:
    if value is None or isinstance(value, bool):
        return None
    try:
        if pd.isna(value):
            return None
    except (TypeError, ValueError):
        pass
    if isinstance(value, str):
        text = value.strip().replace(",", "")
        if not text or text in {"-", "--", "None", "nan"}:
            return None
        multiplier = 1.0
        if text.endswith("%"):
            text = text[:-1]
        elif text.endswith("万亿"):
            text, multiplier = text[:-2], 1e12
        elif text.endswith("亿"):
            text, multiplier = text[:-1], 1e8
        elif text.endswith("万"):
            text, multiplier = text[:-1], 1e4
        try:
            return float(text) * multiplier
        except ValueError:
            return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _as_percent_ratio(value: Any) -> float | None:
    """AkShare 财务百分数统一转换为小数比例，12.5% -> 0.125。"""
    parsed = _as_float(value)
    return None if parsed is None else parsed / 100.0


def _as_date(value: Any) -> date | None:
    if value is None:
        return None
    try:
        if pd.isna(value):
            return None
    except (TypeError, ValueError):
        pass
    if isinstance(value, datetime):
        return value.date()
    if isinstance(value, date):
        return value
    try:
        return pd.to_datetime(value).date()
    except (TypeError, ValueError):
        return None


def _xq_symbol(code: str) -> str:
    market, symbol = code.lower().split(".", 1)
    return f"{market.upper()}{symbol}"


def _em_symbol(code: str) -> str:
    market, symbol = code.lower().split(".", 1)
    return f"{symbol}.{market.upper()}"


def _fetch_xq_valuation(code: str, as_of: date) -> dict[str, Any]:
    fetch = getattr(ak, "stock_individual_spot_xq", None)
    if fetch is None:
        raise RuntimeError("当前 AkShare 不提供 stock_individual_spot_xq")
    df = akshare_client._retry(  # noqa: SLF001 - 复用统一网络重试策略
        lambda: fetch(symbol=_xq_symbol(code), timeout=12),
        retries=2,
        delay=1.0,
    )
    if df is None or df.empty:
        raise RuntimeError("雪球估值接口返回空结果")
    values = dict(zip(df["item"], df["value"], strict=False))
    observed = _as_date(values.get("时间"))
    if observed is None:
        if as_of < date.today():
            raise RuntimeError("雪球当前快照缺少日期，不能用于历史研究")
        observed = as_of
    if observed > as_of:
        raise RuntimeError(
            f"雪球只返回 {observed} 当前快照，不能作为 {as_of} 的历史估值"
        )
    data_date = observed
    return {
        "code": code,
        "data_date": data_date,
        "report_period": None,
        "available_date": data_date,
        "source": "akshare:stock_individual_spot_xq",
        "pe_ttm": _as_float(values.get("市盈率(TTM)")),
        "pb": _as_float(values.get("市净率")),
        "ps_ttm": _as_float(values.get("市销率")),
        "dividend_yield": _as_percent_ratio(values.get("股息率(TTM)")),
        "total_market_cap": _as_float(values.get("资产净值/总市值")),
    }


def _fetch_em_valuations(code: str, as_of: date,
                         history: bool) -> list[dict[str, Any]]:
    """东财历史估值。接口虽返回全历史，默认只保留最新一行控制写入量。"""
    fetch = getattr(ak, "stock_value_em", None)
    if fetch is None:
        raise RuntimeError("当前 AkShare 不提供 stock_value_em")
    symbol = akshare_client.code_to_symbol(code)
    df = akshare_client._retry(
        lambda: fetch(symbol=symbol), retries=2, delay=1.0,
    )
    if df is None or df.empty:
        raise RuntimeError("东财历史估值接口返回空结果")
    records = []
    for row in df.to_dict("records"):
        data_date = _as_date(row.get("数据日期"))
        if data_date is None or data_date > as_of:
            continue
        records.append({
            "code": code,
            "data_date": data_date,
            "report_period": None,
            "available_date": data_date,
            "source": "akshare:stock_value_em",
            "pe_ttm": _as_float(row.get("PE(TTM)")),
            "pb": _as_float(row.get("市净率")),
            "ps_ttm": _as_float(row.get("市销率")),
            "dividend_yield": None,
            "total_market_cap": _as_float(row.get("总市值")),
        })
    if not records:
        raise RuntimeError("东财历史估值没有目标日期前的数据")
    records.sort(key=lambda row: row["data_date"])
    return records if history else records[-1:]


def _fetch_spot_valuation_map(codes: set[str], as_of: date) -> dict[str, dict]:
    """东财全市场快照降级源，仅使用口径明确的 PB 和总市值。"""
    fetch = getattr(ak, "stock_zh_a_spot_em", None)
    if fetch is None:
        return {}
    df = akshare_client._retry(fetch, retries=2, delay=1.0)
    if df is None or df.empty:
        return {}
    result: dict[str, dict] = {}
    for row in df.to_dict("records"):
        symbol = str(row.get("代码", "")).zfill(6)
        if not symbol:
            continue
        code = akshare_client.symbol_to_code(symbol)
        if code not in codes:
            continue
        result[code] = {
            "code": code,
            "data_date": as_of,
            "report_period": None,
            "available_date": as_of,
            "source": "akshare:stock_zh_a_spot_em",
            "pe_ttm": None,
            "pb": _as_float(row.get("市净率")),
            "ps_ttm": None,
            "dividend_yield": None,
            "total_market_cap": _as_float(row.get("总市值")),
        }
    return result


def fetch_valuations(code: str, as_of: date | None = None,
                     history: bool = False) -> list[dict[str, Any]]:
    """拉取估值；当前快照走低成本雪球，显式历史回填才请求较慢的东财。"""
    as_of = as_of or date.today()
    if not history and as_of >= date.today():
        return [_fetch_xq_valuation(code, as_of)]
    if not history:
        return _fetch_em_valuations(code, as_of, history=False)
    try:
        records = _fetch_em_valuations(code, as_of, history)
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError(f"东财历史估值不可用 {code}: {exc}") from exc

    if as_of >= date.today():
        try:
            xq = _fetch_xq_valuation(code, as_of)
        except Exception as exc:  # noqa: BLE001
            logger.info("雪球当前估值补充不可用 %s: %s", code, exc)
        else:
            latest = records[-1]
            if xq["data_date"] == latest["data_date"]:
                latest["dividend_yield"] = xq["dividend_yield"]
                latest["source"] = (
                    f"{latest['source']}+stock_individual_spot_xq"
                )
    return records


def fetch_valuation(code: str, as_of: date | None = None) -> dict[str, Any]:
    """兼容单记录调用，返回目标日期前最新估值。"""
    return fetch_valuations(code, as_of=as_of, history=False)[-1]


def _fetch_financial_em(code: str, as_of: date) -> list[dict[str, Any]]:
    fetch = getattr(ak, "stock_financial_analysis_indicator_em", None)
    if fetch is None:
        raise RuntimeError("当前 AkShare 不提供东财财务指标接口")
    df = akshare_client._retry(
        lambda: fetch(symbol=_em_symbol(code), indicator="按报告期"),
        retries=2,
        delay=1.0,
    )
    if df is None or df.empty:
        raise RuntimeError("东财财务指标接口返回空结果")

    records = []
    for row in df.to_dict("records"):
        report_period = _as_date(row.get("REPORT_DATE"))
        notice_date = _as_date(row.get("NOTICE_DATE"))
        update_date = _as_date(row.get("UPDATE_DATE"))
        dates = [d for d in (notice_date, update_date) if d is not None]
        if report_period is None or not dates:
            continue
        available_date = max(dates)
        if report_period > as_of or available_date > as_of:
            continue
        records.append({
            "code": code,
            "data_date": report_period,
            "report_period": report_period,
            "available_date": available_date,
            "source": "akshare:stock_financial_analysis_indicator_em",
            "roe": _as_percent_ratio(row.get("ROEJQ")),
            "revenue_yoy": _as_percent_ratio(row.get("TOTALOPERATEREVETZ")),
            "profit_yoy": _as_percent_ratio(row.get("PARENTNETPROFITTZ")),
            "gross_margin": _as_percent_ratio(row.get("XSMLL")),
            "net_margin": _as_percent_ratio(row.get("XSJLL")),
            "debt_ratio": _as_percent_ratio(row.get("ZCFZL")),
            "cashflow_ratio": _as_float(row.get("NCO_NETPROFIT")),
        })
    if not records:
        raise RuntimeError("东财财务指标没有已披露报告")
    return records


def _fetch_financial_ths(code: str, as_of: date) -> list[dict[str, Any]]:
    fetch = getattr(ak, "stock_financial_abstract_ths", None)
    if fetch is None:
        raise RuntimeError("当前 AkShare 不提供同花顺财务摘要接口")
    symbol = akshare_client.code_to_symbol(code)
    df = akshare_client._retry(
        lambda: fetch(symbol=symbol, indicator="按报告期"),
        retries=2,
        delay=1.0,
    )
    if df is None or df.empty:
        raise RuntimeError("同花顺财务摘要接口返回空结果")

    records = []
    for row in df.to_dict("records"):
        report_period = _as_date(row.get("报告期"))
        if report_period is None or report_period > as_of:
            continue
        cash_per_share = _as_float(row.get("每股经营现金流"))
        eps = _as_float(row.get("基本每股收益"))
        cashflow_ratio = None
        if cash_per_share is not None and eps not in (None, 0):
            cashflow_ratio = cash_per_share / eps
        records.append({
            "code": code,
            "data_date": report_period,
            "report_period": report_period,
            # 该接口没有公告/修订日。同步日是唯一不会造成未来数据泄漏的口径。
            "available_date": as_of,
            "source": "akshare:stock_financial_abstract_ths",
            "roe": _as_percent_ratio(row.get("净资产收益率")),
            "revenue_yoy": _as_percent_ratio(row.get("营业总收入同比增长率")),
            "profit_yoy": _as_percent_ratio(row.get("净利润同比增长率")),
            "gross_margin": _as_percent_ratio(row.get("销售毛利率")),
            "net_margin": _as_percent_ratio(row.get("销售净利率")),
            "debt_ratio": _as_percent_ratio(row.get("资产负债率")),
            "cashflow_ratio": cashflow_ratio,
        })
    if not records:
        raise RuntimeError("同花顺财务摘要没有可用报告")
    return records


def fetch_financials(code: str, as_of: date | None = None) -> list[dict[str, Any]]:
    """拉取历史财务报告，东财失败后降级同花顺。"""
    as_of = as_of or date.today()
    try:
        return _fetch_financial_em(code, as_of)
    except Exception as exc:  # noqa: BLE001 - 数据源异常类型不可控
        logger.warning("东财财务指标不可用 %s，降级同花顺: %s", code, exc)
        return _fetch_financial_ths(code, as_of)


def _upsert_valuation(db: Session, values: dict[str, Any]) -> None:
    row = db.execute(
        select(ValuationSnapshot).where(
            ValuationSnapshot.code == values["code"],
            ValuationSnapshot.data_date == values["data_date"],
            ValuationSnapshot.available_date == values["available_date"],
        )
    ).scalar_one_or_none()
    if row is None:
        db.add(ValuationSnapshot(**values))
        return
    for field in VALUATION_FIELDS:
        value = values.get(field)
        if value is not None:
            setattr(row, field, value)
    row.source = values["source"]


def _upsert_financial(db: Session, values: dict[str, Any]) -> None:
    row = db.execute(
        select(FundamentalSnapshot).where(
            FundamentalSnapshot.code == values["code"],
            FundamentalSnapshot.report_period == values["report_period"],
            FundamentalSnapshot.available_date == values["available_date"],
        )
    ).scalar_one_or_none()
    if row is None:
        db.add(FundamentalSnapshot(**values))
        return
    for field in FUNDAMENTAL_FIELDS:
        value = values.get(field)
        if value is not None:
            setattr(row, field, value)
    row.source = values["source"]


def _sync_fundamentals_unlocked(db: Session, codes: list[str],
                                as_of: date | None = None,
                                include_valuation: bool = True,
                                include_financials: bool = True,
                                valuation_history: bool = False) -> dict:
    """防御式同步指定股票；单只或单数据源失败不会中断其余股票。"""
    as_of = as_of or date.today()
    codes = sorted(set(code.lower() for code in codes))
    result: dict[str, Any] = {
        "date": str(as_of),
        "requested": len(codes),
        "valuation_upserted": 0,
        "financial_upserted": 0,
        "failures": [],
    }
    pending_valuation_failures: dict[str, Exception] = {}

    for code in codes:
        if include_valuation:
            try:
                valuation_records = fetch_valuations(
                    code, as_of, history=valuation_history,
                )
                for values in valuation_records:
                    _upsert_valuation(db, values)
                db.commit()
                result["valuation_upserted"] += len(valuation_records)
            except Exception as exc:  # noqa: BLE001
                db.rollback()
                pending_valuation_failures[code] = exc

        if include_financials:
            try:
                records = fetch_financials(code, as_of)
                for values in records:
                    _upsert_financial(db, values)
                db.commit()
                result["financial_upserted"] += len(records)
            except Exception as exc:  # noqa: BLE001
                db.rollback()
                logger.warning("财务指标同步失败 %s: %s", code, exc)
                result["failures"].append({
                    "code": code, "stage": "financial", "error": str(exc),
                })

    allow_current_fallback = not valuation_history and as_of >= date.today()
    if pending_valuation_failures and allow_current_fallback:
        try:
            fallback_valuations = _fetch_spot_valuation_map(
                set(pending_valuation_failures), as_of,
            )
        except Exception as exc:  # noqa: BLE001
            logger.warning("东财估值降级快照不可用: %s", exc)
            fallback_valuations = {}
        for code, original_error in pending_valuation_failures.items():
            values = fallback_valuations.get(code)
            if values is not None:
                try:
                    _upsert_valuation(db, values)
                    db.commit()
                    result["valuation_upserted"] += 1
                    continue
                except Exception as exc:  # noqa: BLE001
                    db.rollback()
                    original_error = exc
            logger.warning("估值同步失败 %s: %s", code, original_error)
            result["failures"].append({
                "code": code, "stage": "valuation", "error": str(original_error),
            })
    elif pending_valuation_failures:
        for code, original_error in pending_valuation_failures.items():
            result["failures"].append({
                "code": code, "stage": "valuation", "error": str(original_error),
            })
    return result


def sync_fundamentals(db: Session, codes: list[str],
                      as_of: date | None = None,
                      include_valuation: bool = True,
                      include_financials: bool = True,
                      valuation_history: bool = False) -> dict:
    """串行执行同步，避免定时任务与手动任务并发写入。"""
    if not _SYNC_LOCK.acquire(blocking=False):
        raise FundamentalSyncInProgressError("已有基本面同步任务正在运行")
    try:
        return _sync_fundamentals_unlocked(
            db,
            codes,
            as_of=as_of,
            include_valuation=include_valuation,
            include_financials=include_financials,
            valuation_history=valuation_history,
        )
    finally:
        _SYNC_LOCK.release()


def _normalize_code(code: str) -> str:
    code = code.strip().lower()
    if "." in code:
        market, symbol = code.split(".", 1)
        if market not in {"sh", "sz", "bj"} or not (
            symbol.isdigit() and len(symbol) == 6
        ):
            raise ValueError(f"无效股票代码: {code}")
        return f"{market}.{symbol}"
    if not (code.isdigit() and len(code) <= 6):
        raise ValueError(f"无效股票代码: {code}")
    return akshare_client.symbol_to_code(code.zfill(6))


def _universe_codes(db: Session, universe: str) -> list[str]:
    universe = universe.lower()
    if universe == "watchlist":
        return [r[0] for r in db.execute(
            select(WatchlistItem.code).distinct().order_by(WatchlistItem.code)
        ).all()]
    if universe == "pool":
        return current_pool(db)
    if universe in {"hs300", "zz500"}:
        return [r[0] for r in db.execute(
            select(IndexMember.code).where(
                IndexMember.index_name == universe,
                IndexMember.out_date.is_(None),
            ).distinct().order_by(IndexMember.code)
        ).all()]
    if universe == "all":
        return [r[0] for r in db.execute(select(Stock.code).order_by(Stock.code)).all()]
    raise ValueError("universe 只能是 watchlist、pool、hs300、zz500 或 all")


def sync_fundamental_universe(
    db: Session,
    *,
    universe: str = "watchlist",
    codes: list[str] | None = None,
    max_codes: int = 100,
    include_valuation: bool = True,
    include_financials: bool = True,
    valuation_history: bool = False,
) -> dict:
    """按显式股票或研究范围同步，并用 max_codes 限制外部请求成本。"""
    selected = (
        sorted({_normalize_code(code) for code in codes if code.strip()})
        if codes is not None
        else _universe_codes(db, universe)
    )
    if codes is not None and len(selected) > max_codes:
        raise ValueError(f"显式 codes 共 {len(selected)} 只，超过 max_codes={max_codes}")
    available_codes = len(selected)
    selected = selected[:max_codes]
    result = sync_fundamentals(
        db,
        selected,
        include_valuation=include_valuation,
        include_financials=include_financials,
        valuation_history=valuation_history,
    )
    result.update({
        "universe": "explicit" if codes is not None else universe,
        "available_codes": available_codes,
        "truncated": available_codes > len(selected),
    })
    return result
