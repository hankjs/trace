"""AkShare 基本面与估值数据采集。

单股接口失败只影响该股票；没有可靠公告日期的降级数据以抓取日作为
available_date，宁可少参与历史筛选，也不把后来获得的数据泄漏到过去。
"""
from __future__ import annotations

import asyncio
import json
import logging
import threading
import time
from datetime import date, datetime
from typing import Any

import akshare as ak
import httpx
import pandas as pd
from sqlalchemy import func, select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.orm import Session

from ..models import (FundamentalSnapshot, Stock,
                      ValuationSnapshot, WatchlistItem)
from . import akshare_client
from .universe import pool_at

logger = logging.getLogger(__name__)

VALUATION_FIELDS = (
    "pe_ttm", "pb", "ps_ttm", "dividend_yield", "total_market_cap",
)
FUNDAMENTAL_FIELDS = (
    "roe", "revenue_yoy", "profit_yoy", "gross_margin", "net_margin",
    "debt_ratio", "cashflow_ratio",
)

_SYNC_LOCK = threading.Lock()

EM_VALUATION_URL = "https://datacenter-web.eastmoney.com/api/data/v1/get"
EM_FINANCIAL_URL = "https://datacenter.eastmoney.com/securities/api/data/get"
EM_PAGE_SIZE = 1000
DEFAULT_REQUEST_INTERVAL = 10.0
# 单日估值、A 股单报告期正常约 3~8 页。过滤条件若失效会返回数百页，
# 仍会被下方上限拦截，避免参数错误变成高频全站爬取。
MAX_MARKET_PAGES = 10
MAX_RESPONSE_BYTES = 25 * 1024 * 1024
MAX_RESPONSE_SECONDS = 45.0

VALUATION_COLUMNS = (
    "SECUCODE,SECURITY_CODE,TRADE_DATE,BOARD_NAME,PE_TTM,PB_MRQ,PS_TTM,"
    "TOTAL_MARKET_CAP"
)
FINANCIAL_COLUMNS = (
    "SECUCODE,REPORT_DATE,NOTICE_DATE,UPDATE_DATE,ROEJQ,"
    "TOTALOPERATEREVETZ,PARENTNETPROFITTZ,XSMLL,XSJLL,ZCFZL,NCO_NETPROFIT"
)


class FundamentalSyncInProgressError(RuntimeError):
    """同一进程中已有估值或财务同步任务。"""


class FundamentalRateLimitError(RuntimeError):
    """外部数据源已限流；调用方应停止本轮，不继续重试其他分页。"""


class FundamentalSafetyLimitError(RuntimeError):
    """响应大小或耗时越过安全阈值；本轮立即停止。"""


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


def _em_secu_to_code(value: Any) -> str | None:
    """东财 000001.SZ / 600519.SH / 920000.BJ -> 本地代码。"""
    text = str(value or "").strip().upper()
    if "." not in text:
        return None
    symbol, market = text.split(".", 1)
    if market not in {"SH", "SZ", "BJ"} or not (
        symbol.isdigit() and len(symbol) == 6
    ):
        return None
    return f"{market.lower()}.{symbol}"


async def _request_em_json_async(
    url: str, params: dict[str, str],
) -> dict[str, Any]:
    timeout = httpx.Timeout(connect=5, read=15, write=5, pool=5)
    body = bytearray()
    async with httpx.AsyncClient(timeout=timeout) as client:
        async with asyncio.timeout(MAX_RESPONSE_SECONDS):
            async with client.stream("GET", url, params=params) as response:
                if response.status_code in {403, 429}:
                    raise FundamentalRateLimitError(
                        f"东财限流 HTTP {response.status_code}，已停止本轮同步"
                    )
                if 400 <= response.status_code < 500:
                    raise RuntimeError(
                        f"东财请求被拒绝 HTTP {response.status_code}"
                    )
                response.raise_for_status()
                async for chunk in response.aiter_bytes():
                    body.extend(chunk)
                    if len(body) > MAX_RESPONSE_BYTES:
                        raise FundamentalSafetyLimitError(
                            "东财单页响应超过 "
                            f"{MAX_RESPONSE_BYTES // 1024 // 1024}MB，已停止本轮同步"
                        )
    payload = json.loads(body)
    if not payload.get("success"):
        raise RuntimeError(
            f"东财接口失败: {payload.get('message') or 'unknown'}"
        )
    return payload


def _request_em_json(url: str, params: dict[str, str]) -> dict[str, Any]:
    """低频请求东财；单页总超时，任何失败均不自动重试。"""
    try:
        return asyncio.run(_request_em_json_async(url, params))
    except (FundamentalRateLimitError, FundamentalSafetyLimitError):
        raise
    except TimeoutError as exc:
        raise FundamentalSafetyLimitError(
            f"东财单页响应超过 {MAX_RESPONSE_SECONDS:.0f} 秒，已停止本轮同步"
        ) from exc
    except (httpx.HTTPError, ValueError, RuntimeError) as exc:
        raise RuntimeError(f"东财请求失败，未自动重试: {exc}") from exc


def _fetch_em_pages(
    url: str,
    params: dict[str, str],
    *,
    page_param: str,
    size_param: str,
    request_interval: float,
) -> tuple[list[dict[str, Any]], int]:
    """串行拉取一个已严格过滤的全市场报表，返回记录和请求数。"""
    if request_interval < 0:
        raise ValueError("request_interval 不能小于 0")
    first_params = {**params, page_param: "1", size_param: str(EM_PAGE_SIZE)}
    first = _request_em_json(url, first_params)
    result = first.get("result") or {}
    pages = int(result.get("pages") or 0)
    if pages > MAX_MARKET_PAGES:
        raise RuntimeError(
            f"东财报表返回 {pages} 页，超过安全上限 {MAX_MARKET_PAGES}；"
            "可能是过滤条件失效，已停止同步"
        )
    rows = list(result.get("data") or [])
    requests_made = 1
    logger.info("东财分页进度 1/%d，已收 %d 行", pages, len(rows))
    for page in range(2, pages + 1):
        if request_interval:
            time.sleep(request_interval)
        payload = _request_em_json(
            url, {**params, page_param: str(page), size_param: str(EM_PAGE_SIZE)},
        )
        rows.extend((payload.get("result") or {}).get("data") or [])
        requests_made += 1
        logger.info("东财分页进度 %d/%d，已收 %d 行", page, pages, len(rows))
    return rows, requests_made


def fetch_market_valuations(
    day: date,
    *,
    available_codes: set[str] | None = None,
    request_interval: float = DEFAULT_REQUEST_INTERVAL,
) -> tuple[list[dict[str, Any]], dict[str, str], int]:
    """按交易日批量拉全市场估值；正常约六次千行分页请求。"""
    raw_rows, requests_made = _fetch_em_pages(
        EM_VALUATION_URL,
        {
            "sortColumns": "SECURITY_CODE",
            "sortTypes": "1",
            "reportName": "RPT_VALUEANALYSIS_DET",
            "columns": VALUATION_COLUMNS,
            "quoteColumns": "",
            "source": "WEB",
            "client": "WEB",
            "filter": f"(TRADE_DATE='{day.isoformat()}')",
        },
        page_param="pageNumber",
        size_param="pageSize",
        request_interval=request_interval,
    )
    records: list[dict[str, Any]] = []
    industries: dict[str, str] = {}
    for row in raw_rows:
        code = _em_secu_to_code(row.get("SECUCODE"))
        data_date = _as_date(row.get("TRADE_DATE"))
        if code is None or data_date != day:
            continue
        if available_codes is not None and code not in available_codes:
            continue
        records.append({
            "code": code,
            "data_date": data_date,
            "report_period": None,
            "available_date": data_date,
            "source": "eastmoney:RPT_VALUEANALYSIS_DET",
            "pe_ttm": _as_float(row.get("PE_TTM")),
            "pb": _as_float(row.get("PB_MRQ")),
            "ps_ttm": _as_float(row.get("PS_TTM")),
            # 该报表没有 TTM 股息率，不能拿动态市盈率等其他口径冒充。
            "dividend_yield": None,
            "total_market_cap": _as_float(row.get("TOTAL_MARKET_CAP")),
        })
        industry = str(row.get("BOARD_NAME") or "").strip()
        if industry:
            industries[code] = industry
    return records, industries, requests_made


def fetch_market_financials(
    report_period: date,
    *,
    available_codes: set[str] | None = None,
    request_interval: float = DEFAULT_REQUEST_INTERVAL,
) -> tuple[list[dict[str, Any]], int]:
    """按报告期批量拉全市场主要财务指标；正常约八次千行分页请求。"""
    raw_rows, requests_made = _fetch_em_pages(
        EM_FINANCIAL_URL,
        {
            "type": "RPT_F10_FINANCE_MAINFINADATA",
            "sty": FINANCIAL_COLUMNS,
            "quoteColumns": "",
            "filter": (
                '(SECURITY_TYPE_CODE in ("058001001","058001008"))'
                f"(REPORT_DATE='{report_period.isoformat()}')"
            ),
            "sr": "1",
            "st": "SECUCODE",
            "source": "HSF10",
            "client": "PC",
        },
        page_param="p",
        size_param="ps",
        request_interval=request_interval,
    )
    records: list[dict[str, Any]] = []
    for row in raw_rows:
        code = _em_secu_to_code(row.get("SECUCODE"))
        row_period = _as_date(row.get("REPORT_DATE"))
        dates = [
            value for value in (
                _as_date(row.get("NOTICE_DATE")),
                _as_date(row.get("UPDATE_DATE")),
            )
            if value is not None
        ]
        if code is None or row_period != report_period or not dates:
            continue
        if available_codes is not None and code not in available_codes:
            continue
        available_date = max(dates)
        records.append({
            "code": code,
            "data_date": row_period,
            "report_period": row_period,
            "available_date": available_date,
            "source": "eastmoney:RPT_F10_FINANCE_MAINFINADATA",
            "roe": _as_percent_ratio(row.get("ROEJQ")),
            "revenue_yoy": _as_percent_ratio(row.get("TOTALOPERATEREVETZ")),
            "profit_yoy": _as_percent_ratio(row.get("PARENTNETPROFITTZ")),
            "gross_margin": _as_percent_ratio(row.get("XSMLL")),
            "net_margin": _as_percent_ratio(row.get("XSJLL")),
            "debt_ratio": _as_percent_ratio(row.get("ZCFZL")),
            "cashflow_ratio": _as_float(row.get("NCO_NETPROFIT")),
        })
    return records, requests_made


def report_periods_between(start: date, end: date) -> list[date]:
    """返回区间内已结束的季度报告期。"""
    if start > end:
        raise ValueError("start 不能晚于 end")
    periods = [
        date(year, month, day)
        for year in range(start.year, end.year + 1)
        for month, day in ((3, 31), (6, 30), (9, 30), (12, 31))
    ]
    return [period for period in periods if start <= period <= end]


def recent_report_periods(day: date, count: int = 5) -> list[date]:
    """日常刷新最近若干报告期，以捕获集中披露和后续修订。"""
    if count <= 0:
        raise ValueError("count 必须大于 0")
    start = date(day.year - ((count + 3) // 4 + 1), 1, 1)
    return report_periods_between(start, day)[-count:]


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
        try:
            return [_fetch_xq_valuation(code, as_of)]
        except Exception as exc:  # noqa: BLE001 - 雪球 token 经常失效
            logger.info("雪球当前估值不可用 %s，降级东财最新估值: %s", code, exc)
            return _fetch_em_valuations(code, as_of, history=False)
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


def _bulk_upsert(
    db: Session,
    model: type[ValuationSnapshot] | type[FundamentalSnapshot],
    rows: list[dict[str, Any]],
    *,
    unique_fields: tuple[str, ...],
    metric_fields: tuple[str, ...],
    batch_size: int = 2000,
) -> int:
    """批量 upsert；整个调用由上层统一 commit，保证单个日期原子写入。"""
    if not rows:
        return 0
    table = model.__table__
    dialect = db.get_bind().dialect.name
    for start in range(0, len(rows), batch_size):
        batch = rows[start:start + batch_size]
        if dialect == "mysql":
            stmt = mysql_insert(table).values(batch)
            updates = {
                field: func.coalesce(stmt.inserted[field], table.c[field])
                for field in metric_fields
            }
            updates["source"] = stmt.inserted.source
            db.execute(stmt.on_duplicate_key_update(**updates))
        elif dialect == "sqlite":
            stmt = sqlite_insert(table).values(batch)
            updates = {
                field: func.coalesce(stmt.excluded[field], table.c[field])
                for field in metric_fields
            }
            updates["source"] = stmt.excluded.source
            db.execute(stmt.on_conflict_do_update(
                index_elements=[table.c[field] for field in unique_fields],
                set_=updates,
            ))
        else:
            raise RuntimeError(f"不支持的数据库方言: {dialect}")
    return len(rows)


def _field_coverage(
    rows: list[dict[str, Any]], fields: tuple[str, ...],
) -> dict[str, int]:
    return {
        field: sum(row.get(field) is not None for row in rows)
        for field in fields
    }


def sync_market_valuations(
    db: Session,
    day: date,
    *,
    request_interval: float = DEFAULT_REQUEST_INTERVAL,
) -> dict[str, Any]:
    """低请求量同步全市场单日估值，同时刷新当前行业分类。"""
    if not _SYNC_LOCK.acquire(blocking=False):
        raise FundamentalSyncInProgressError("已有基本面同步任务正在运行")
    try:
        stocks = {
            stock.code: stock
            for stock in db.execute(select(Stock)).scalars().all()
        }
        rows, industries, requests_made = fetch_market_valuations(
            day,
            available_codes=set(stocks),
            request_interval=request_interval,
        )
        if not rows:
            raise RuntimeError(f"东财全市场估值 {day} 返回空结果，未写库")
        written = _bulk_upsert(
            db,
            ValuationSnapshot,
            rows,
            unique_fields=("code", "data_date", "available_date"),
            metric_fields=VALUATION_FIELDS,
        )
        industry_updated = 0
        for code, industry in industries.items():
            stock = stocks.get(code)
            if stock is not None and stock.industry != industry:
                stock.industry = industry
                industry_updated += 1
        db.commit()
        return {
            "date": str(day),
            "requests": requests_made,
            "fetched": len(rows),
            "upserted": written,
            "industry_updated": industry_updated,
            "coverage": _field_coverage(rows, VALUATION_FIELDS),
        }
    except Exception:
        db.rollback()
        raise
    finally:
        _SYNC_LOCK.release()


def sync_market_financials(
    db: Session,
    report_periods: list[date],
    *,
    request_interval: float = DEFAULT_REQUEST_INTERVAL,
) -> dict[str, Any]:
    """低频串行刷新全市场报告期；每个报告期独立事务，便于断点续跑。"""
    if not _SYNC_LOCK.acquire(blocking=False):
        raise FundamentalSyncInProgressError("已有基本面同步任务正在运行")
    try:
        available_codes = {
            row[0] for row in db.execute(select(Stock.code)).all()
        }
        results: list[dict[str, Any]] = []
        total_requests = 0
        for index, report_period in enumerate(sorted(set(report_periods))):
            if index and request_interval:
                time.sleep(request_interval)
            try:
                rows, requests_made = fetch_market_financials(
                    report_period,
                    available_codes=available_codes,
                    request_interval=request_interval,
                )
                written = _bulk_upsert(
                    db,
                    FundamentalSnapshot,
                    rows,
                    unique_fields=("code", "report_period", "available_date"),
                    metric_fields=FUNDAMENTAL_FIELDS,
                )
                db.commit()
            except Exception:
                db.rollback()
                raise
            total_requests += requests_made
            results.append({
                "report_period": str(report_period),
                "fetched": len(rows),
                "upserted": written,
                "requests": requests_made,
                "coverage": _field_coverage(rows, FUNDAMENTAL_FIELDS),
            })
        return {
            "periods": results,
            "period_count": len(results),
            "requests": total_requests,
            "upserted": sum(item["upserted"] for item in results),
        }
    finally:
        _SYNC_LOCK.release()


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


def _universe_codes(db: Session, universe: str,
                    as_of: date | None = None) -> list[str]:
    """解析研究范围为代码列表。

    指数口径统一走 universe.py 的 point-in-time 解析(pool_at):此前这里只取
    `out_date IS NULL`(今天的成分),用它同步历史财报就是幸存者偏差——今天
    已被调出指数的股票在历史区间内本该在册,却永远同步不到。
    as_of 为 None 时按今天解析(日常增量同步的语义)。
    """
    universe = universe.lower()
    day = as_of or date.today()
    if universe == "watchlist":
        return [r[0] for r in db.execute(
            select(WatchlistItem.code).distinct().order_by(WatchlistItem.code)
        ).all()]
    if universe == "pool":
        return pool_at(db, day)
    if universe in {"hs300", "zz500"}:
        return pool_at(db, day, index_name=universe)
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
