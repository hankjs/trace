"""条件筛选器:基于 quant_factor_daily + 当日 K 线的实时筛选(不落库)。

条件:当日涨幅区间、量比下限、均线多头(close>ma20>ma60)、
距 N 日新高幅度上限、20 日日均成交额下限。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta
from typing import Any

import pandas as pd
from fastapi import HTTPException
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from ..catalog import FILTER_FIELDS
from ..models import (
    DailyBar,
    FactorDaily,
    FundamentalSnapshot,
    Stock,
    ValuationSnapshot,
    WatchlistItem,
)

logger = logging.getLogger(__name__)

VALUATION_MAX_AGE_DAYS = 7
MAX_HIGH_WINDOW = 750  # 约 3 年交易日,防止无上界的窗口打爆查询区间


class InvalidFilterError(ValueError):
    """结构化筛选条件不合法。"""


def _latest_factor_date(db: Session, day: date | None) -> date | None:
    q = select(FactorDaily.date)
    if day:
        q = q.where(FactorDaily.date <= day)
    return db.execute(q.order_by(FactorDaily.date.desc()).limit(1)).scalar()


def _load_bars_batch(db: Session, codes: list[str], start: date,
                     end: date) -> dict[str, pd.DataFrame]:
    """一次查询取回多只股票的日线,按 code 切成 DataFrame。

    取代"逐只 load_bars_df"的 N+1:全A 默认口径下 5400 只 × 每只一次往返
    必然把接口拖超时。这里按 code IN (...) 分批查询,单次扫描后在内存分组。
    """
    if not codes:
        return {}
    frames: dict[str, pd.DataFrame] = {}
    chunk = 500  # 避免 IN 列表过长撑爆 SQL 解析
    for i in range(0, len(codes), chunk):
        batch = codes[i:i + chunk]
        rows = db.execute(
            select(DailyBar.code, DailyBar.date, DailyBar.open, DailyBar.high,
                   DailyBar.low, DailyBar.close, DailyBar.volume,
                   DailyBar.amount)
            .where(DailyBar.code.in_(batch),
                   DailyBar.date >= start, DailyBar.date <= end)
            .order_by(DailyBar.code, DailyBar.date)
        ).all()
        if not rows:
            continue
        df = pd.DataFrame(rows, columns=["code", "date", "open", "high", "low",
                                         "close", "volume", "amount"])
        for code, group in df.groupby("code", sort=False):
            frames[str(code)] = group.drop(columns=["code"]).reset_index(drop=True)
    return frames


def screen(db: Session, day: date | None = None,
           pct_chg_min: float | None = None,
           pct_chg_max: float | None = None,
           vol_ratio_min: float | None = None,
           ma_bull: bool = False,
           high_dist_max: float | None = None,
           high_window: int = 60,
           amount_min: float | None = None,
           limit: int = 100) -> dict:
    """条件筛选。返回 {date, total, items:[{code, name, 因子..., pct_chg, high_dist}]}

    total 为**匹配总数**(截断前),items 才受 limit 限制。
    """
    if limit <= 0:
        raise InvalidFilterError("limit 必须为正整数")
    if high_window <= 0 or high_window > MAX_HIGH_WINDOW:
        raise InvalidFilterError(f"high_window 必须在 1 到 {MAX_HIGH_WINDOW} 之间")
    fdate = _latest_factor_date(db, day)
    if fdate is None:
        return {"date": None, "total": 0, "items": []}

    q = select(FactorDaily).where(FactorDaily.date == fdate)
    if vol_ratio_min is not None:
        q = q.where(FactorDaily.vol_ratio5 >= vol_ratio_min)
    if amount_min is not None:
        q = q.where(FactorDaily.amount_avg20 >= amount_min)
    rows = db.execute(q).scalars().all()

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
    start = fdate - timedelta(days=max(high_window, 60) * 2 + 30)
    # 批量取日线,避免逐只往返(全A 口径下 N+1 会直接超时)
    bars = _load_bars_batch(db, [r.code for r in rows], start, fdate)

    items = []
    for r in rows:
        df = bars.get(r.code)
        if df is None or len(df) < 2 or df["date"].iat[-1] != fdate:
            continue
        close = float(df["close"].iat[-1])
        prev = float(df["close"].iat[-2])
        pct_chg = close / prev - 1 if prev else None
        if pct_chg_min is not None and (pct_chg is None or pct_chg < pct_chg_min):
            continue
        if pct_chg_max is not None and (pct_chg is None or pct_chg > pct_chg_max):
            continue
        ma20 = df["close"].rolling(20).mean().iat[-1]
        ma60 = df["close"].rolling(60).mean().iat[-1]
        if ma_bull and not (close > ma20 > ma60):
            continue
        high_n = float(df["high"].tail(high_window).max())
        high_dist = close / high_n - 1 if high_n else None  # <=0,0 表示创新高
        if high_dist_max is not None and (
                high_dist is None or high_dist < -abs(high_dist_max)):
            continue
        items.append({
            "code": r.code,
            "name": names.get(r.code, ""),
            "close": round(close, 3),
            "pct_chg": None if pct_chg is None else round(pct_chg, 4),
            "high_dist": None if high_dist is None else round(high_dist, 4),
            "mom20": r.mom20, "mom60": r.mom60, "rsi14": r.rsi14,
            "atr_pct": r.atr_pct, "vol_ratio5": r.vol_ratio5,
            "ma20_slope": r.ma20_slope, "amount_avg20": r.amount_avg20,
        })

    items.sort(key=lambda x: (-(x["mom20"] or -9), x["code"]))
    total = len(items)  # 截断前的匹配总数
    return {"date": str(fdate), "total": total, "items": items[:limit]}


def _coerce_scalar(value: Any, value_type: str) -> Any:
    if value_type == "number":
        if value is None or isinstance(value, bool):
            raise InvalidFilterError("数值条件必须填写数字")
        try:
            return float(value)
        except (TypeError, ValueError) as exc:
            raise InvalidFilterError(f"无法把 {value!r} 解析为数字") from exc
    if value_type == "boolean":
        if isinstance(value, bool):
            return value
        if isinstance(value, str) and value.lower() in {"true", "false"}:
            return value.lower() == "true"
        raise InvalidFilterError("布尔条件必须是 true 或 false")
    if value_type == "string":
        if value is None:
            raise InvalidFilterError("文本条件不能为空")
        return str(value)
    return value


def _compile_condition(raw: dict[str, Any], fallback_id: str) -> dict[str, Any]:
    field = str(raw.get("field", ""))
    metadata = FILTER_FIELDS.get(field)
    if metadata is None or not metadata.get("available", True):
        raise InvalidFilterError(f"不支持的筛选字段: {field}")
    operator = str(raw.get("operator", "")).lower()
    if operator not in metadata.get("operators", ()):
        raise InvalidFilterError(f"字段 {field} 不支持操作符 {operator}")

    compiled = {
        "id": str(raw.get("id") or fallback_id),
        "field": field,
        "field_name": metadata["name"],
        "operator": operator,
        "value": None,
        "value_to": None,
    }
    if operator in {"is_null", "not_null"}:
        return compiled

    value_type = metadata.get("value_type", "number")
    value = raw.get("value")
    if operator in {"in", "not_in"}:
        if isinstance(value, str):
            value = [part.strip() for part in value.split(",") if part.strip()]
        if not isinstance(value, (list, tuple, set)) or not value:
            raise InvalidFilterError(f"操作符 {operator} 需要非空数组 value")
        compiled["value"] = [_coerce_scalar(v, value_type) for v in value]
        return compiled

    compiled["value"] = _coerce_scalar(value, value_type)
    if operator == "between":
        value_to = raw.get("value_to", raw.get("value2"))
        compiled["value_to"] = _coerce_scalar(value_to, value_type)
        if compiled["value"] > compiled["value_to"]:
            raise InvalidFilterError(f"条件 {compiled['id']} 的区间下限不能大于上限")
    return compiled


def _matches(actual: Any, condition: dict[str, Any]) -> bool:
    operator = condition["operator"]
    if operator == "is_null":
        return actual is None or actual == ""
    if operator == "not_null":
        return actual is not None and actual != ""
    if actual is None or actual == "":
        return False

    value = condition["value"]
    if operator == "eq":
        return actual == value
    if operator == "ne":
        return actual != value
    if operator == "gt":
        return actual > value
    if operator == "gte":
        return actual >= value
    if operator == "lt":
        return actual < value
    if operator == "lte":
        return actual <= value
    if operator == "between":
        return value <= actual <= condition["value_to"]
    if operator == "in":
        return actual in value
    if operator == "not_in":
        return actual not in value
    raise InvalidFilterError(f"未知操作符: {operator}")


def evaluate_conditions(rows: list[dict[str, Any]], payload: dict[str, Any]) -> dict:
    """纯函数条件引擎，供 API 与测试共同使用。"""
    root_logic = str(payload.get("logic", "and")).lower()
    if root_logic not in {"and", "or"}:
        raise InvalidFilterError("logic 只能是 and 或 or")

    expressions: list[tuple[str, Any]] = []
    compiled_conditions: list[dict[str, Any]] = []
    used_ids: set[str] = set()

    def compile_one(raw: dict[str, Any], fallback_id: str) -> dict[str, Any] | None:
        if not raw.get("enabled", True):
            return None
        condition = _compile_condition(raw, fallback_id)
        if condition["id"] in used_ids:
            raise InvalidFilterError(f"条件 id 重复: {condition['id']}")
        used_ids.add(condition["id"])
        compiled_conditions.append(condition)
        return condition

    for index, raw in enumerate(payload.get("conditions") or [], start=1):
        condition = compile_one(raw, f"condition_{index}")
        if condition:
            expressions.append(("condition", condition))

    for group_index, raw_group in enumerate(payload.get("groups") or [], start=1):
        group_logic = str(raw_group.get("logic", "and")).lower()
        if group_logic not in {"and", "or"}:
            raise InvalidFilterError("分组 logic 只能是 and 或 or")
        group_id = str(raw_group.get("id") or f"group_{group_index}")
        group_conditions = []
        for condition_index, raw in enumerate(raw_group.get("conditions") or [], start=1):
            condition = compile_one(raw, f"{group_id}_condition_{condition_index}")
            if condition:
                group_conditions.append(condition)
        if group_conditions:
            expressions.append(("group", (group_logic, group_conditions)))

    independent_counts = {condition["id"]: 0 for condition in compiled_conditions}
    enabled_fields = list(dict.fromkeys(
        condition["field"] for condition in compiled_conditions
    ))
    field_coverage = {
        field: sum(
            1 for row in rows
            if row["values"].get(field) is not None
            and row["values"].get(field) != ""
        )
        for field in enabled_fields
    }
    matched_rows = []
    for row in rows:
        values = row["values"]
        states = {
            condition["id"]: _matches(values.get(condition["field"]), condition)
            for condition in compiled_conditions
        }
        for condition_id, matched in states.items():
            if matched:
                independent_counts[condition_id] += 1

        expression_states = []
        for kind, expression in expressions:
            if kind == "condition":
                expression_states.append(states[expression["id"]])
            else:
                logic, conditions = expression
                results = [states[condition["id"]] for condition in conditions]
                expression_states.append(all(results) if logic == "and" else any(results))
        combined = (
            True if not expression_states
            else all(expression_states) if root_logic == "and"
            else any(expression_states)
        )
        if combined:
            matched_ids = [key for key, matched in states.items() if matched]
            failed_ids = [key for key, matched in states.items() if not matched]
            reasons = [
                {
                    "condition_id": condition["id"],
                    "field": condition["field"],
                    "field_name": condition["field_name"],
                    "actual": values.get(condition["field"]),
                    "matched": states[condition["id"]],
                }
                for condition in compiled_conditions
            ]
            matched_rows.append({
                **row,
                "matched_conditions": matched_ids,
                "failed_conditions": failed_ids,
                "match_reasons": reasons,
            })

    return {
        "items": matched_rows,
        "independent_counts": independent_counts,
        "condition_counts": [
            {
                "id": condition["id"],
                "field": condition["field"],
                "field_name": condition["field_name"],
                "matched": independent_counts[condition["id"]],
                "available": field_coverage[condition["field"]],
                "total": len(rows),
            }
            for condition in compiled_conditions
        ],
        "field_coverage": field_coverage,
    }


def codes_for_pool(
    db: Session,
    day: date,
    *,
    pool_id: int | None,
    watchlist_only: bool,
    user_id: str | None,
) -> list[str]:
    """筛选范围解析:统一走 universe.resolve_pool,不在此重写任何池口径。

    早先这里是一套 `universe` 字符串分支(pool/hs300_zz500/hs300/zz500/
    watchlist/all),与 `universe.py` 各自实现 in_date/out_date 条件,两处会
    漂移;且它的 `universe='all'` 是全表无过滤,与 `kind='all'`(剔 ST/退市/
    新股)语义不同,同名不同义。现已统一。

    `watchlist_only` 是独立开关而非一种 kind:自选是用户关系不是池,做成池会
    引入「自选变化时池成员如何同步」的新问题。
    """
    if watchlist_only:
        if user_id is None:
            return []
        return [r[0] for r in db.execute(
            select(WatchlistItem.code).where(WatchlistItem.user_id == user_id)
            .order_by(WatchlistItem.code)
        ).all()]

    from ..api.pools import default_pool, get_pool_or_404, resolve_pool_codes

    if pool_id is None:
        pool = default_pool(db)
        if pool is None:
            raise InvalidFilterError(
                "系统缺少预置股票池，请先执行 alembic upgrade head")
    elif user_id is None:
        raise InvalidFilterError("指定股票池需要登录")
    else:
        pool = get_pool_or_404(db, pool_id, user_id)
    try:
        return resolve_pool_codes(db, pool, day)
    except HTTPException as exc:
        # resolve_pool_codes 面向 API 层,把数据完整性问题抛成 HTTPException(422);
        # 筛选层统一用 InvalidFilterError,由 post_screener 再转 422。
        if exc.status_code == 422:
            raise InvalidFilterError(str(exc.detail)) from exc
        raise


def _latest_rows_by_code(
    db: Session,
    model: Any,
    where: tuple[Any, ...],
    order_by: tuple[Any, ...],
) -> dict[str, Any]:
    """用窗口函数在数据库内把历史版本压缩为每只股票一行。"""
    ranked = select(
        model.id.label("row_id"),
        func.row_number().over(
            partition_by=model.code,
            order_by=order_by,
        ).label("row_number"),
    ).where(*where).subquery()
    rows = db.execute(
        select(model)
        .join(ranked, model.id == ranked.c.row_id)
        .where(ranked.c.row_number == 1)
    ).scalars()
    return {row.code: row for row in rows}


def _build_screen_rows(
    db: Session,
    day: date,
    *,
    pool_id: int | None,
    watchlist_only: bool,
    user_id: str | None,
) -> list[dict[str, Any]]:
    codes = codes_for_pool(
        db, day, pool_id=pool_id, watchlist_only=watchlist_only,
        user_id=user_id,
    )
    if not codes:
        return []
    code_set = set(codes)
    stocks = {
        row.code: row
        for row in db.execute(select(Stock).where(Stock.code.in_(codes))).scalars()
    }
    factors = {
        row.code: row
        for row in db.execute(select(FactorDaily).where(
            FactorDaily.date == day,
            FactorDaily.code.in_(codes),
        )).scalars()
    }
    listing_days = dict(db.execute(
        select(DailyBar.code, func.count()).where(
            DailyBar.code.in_(codes),
            DailyBar.date <= day,
        ).group_by(DailyBar.code)
    ).all())

    valuation_cutoff = day - timedelta(days=VALUATION_MAX_AGE_DAYS)
    valuations = _latest_rows_by_code(
        db,
        ValuationSnapshot,
        (
            ValuationSnapshot.code.in_(codes),
            ValuationSnapshot.data_date >= valuation_cutoff,
            ValuationSnapshot.data_date <= day,
            ValuationSnapshot.available_date <= day,
        ),
        (
            ValuationSnapshot.data_date.desc(),
            ValuationSnapshot.available_date.desc(),
            ValuationSnapshot.id.desc(),
        ),
    )

    fundamentals = _latest_rows_by_code(
        db,
        FundamentalSnapshot,
        (
            FundamentalSnapshot.code.in_(codes),
            FundamentalSnapshot.data_date <= day,
            FundamentalSnapshot.report_period <= day,
            FundamentalSnapshot.available_date <= day,
        ),
        (
            FundamentalSnapshot.report_period.desc(),
            FundamentalSnapshot.available_date.desc(),
            FundamentalSnapshot.id.desc(),
        ),
    )

    # 市场派生字段只对已有当日因子的研究股票加载，避免 all 范围读取全市场历史。
    factor_codes = sorted(code_set & set(factors))
    bars_by_code: dict[str, list[DailyBar]] = {}
    if factor_codes:
        start = day - timedelta(days=220)
        bar_rows = db.execute(
            select(DailyBar).where(
                DailyBar.code.in_(factor_codes),
                DailyBar.date >= start,
                DailyBar.date <= day,
            ).order_by(DailyBar.code, DailyBar.date)
        ).scalars().all()
        for bar in bar_rows:
            bars_by_code.setdefault(bar.code, []).append(bar)

    result = []
    for code in codes:
        stock = stocks.get(code)
        factor = factors.get(code)
        valuation = valuations.get(code)
        fundamental = fundamentals.get(code)
        bars = bars_by_code.get(code, [])
        latest = bars[-1] if bars and bars[-1].date == day else None
        close = latest.close if latest else None
        pct_chg = None
        high_dist = None
        ma_bull = None
        if latest and len(bars) >= 2 and bars[-2].close:
            pct_chg = latest.close / bars[-2].close - 1
        if latest:
            high_60 = max(bar.high for bar in bars[-60:])
            high_dist = latest.close / high_60 - 1 if high_60 else None
        if latest and len(bars) >= 60:
            ma20 = sum(bar.close for bar in bars[-20:]) / 20
            ma60 = sum(bar.close for bar in bars[-60:]) / 60
            ma_bull = latest.close > ma20 > ma60

        values = {key: None for key in FILTER_FIELDS}
        values["industry"] = stock.industry if stock else ""
        name = stock.name if stock else ""
        values["is_st"] = "ST" in name.upper() or "退" in name
        values["listing_days"] = listing_days.get(code, 0)
        if factor:
            for field in (
                "mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5",
                "ma20_slope", "amount_avg20",
            ):
                values[field] = getattr(factor, field)
        values.update({
            "pct_chg": pct_chg,
            "high_dist": high_dist,
            "ma_bull": ma_bull,
            "close": close,
        })
        if valuation:
            for field in ("pe_ttm", "pb", "ps_ttm", "dividend_yield",
                          "total_market_cap"):
                values[field] = getattr(valuation, field)
        if fundamental:
            for field in (
                "roe", "revenue_yoy", "profit_yoy", "gross_margin",
                "net_margin", "debt_ratio", "cashflow_ratio",
            ):
                values[field] = getattr(fundamental, field)
        values.update({
            "valuation_data_date": (
                str(valuation.data_date) if valuation else None
            ),
            "valuation_available_date": (
                str(valuation.available_date) if valuation else None
            ),
            "valuation_source": valuation.source if valuation else None,
            "report_period": (
                str(fundamental.report_period) if fundamental else None
            ),
            "fundamental_available_date": (
                str(fundamental.available_date) if fundamental else None
            ),
            "fundamental_source": fundamental.source if fundamental else None,
        })
        result.append({
            "code": code,
            "name": name,
            "industry": stock.industry if stock else "",
            "values": values,
        })
    return result


def structured_screen(db: Session, payload: dict[str, Any],
                      user_id: int | None = None) -> dict:
    """技术面与基本面结构化组合筛选。"""
    requested_day = payload.get("date")
    if isinstance(requested_day, str):
        try:
            requested_day = date.fromisoformat(requested_day)
        except ValueError as exc:
            raise InvalidFilterError("date 必须是 YYYY-MM-DD") from exc
    day = _latest_factor_date(db, requested_day)
    if day is None:
        day = requested_day or date.today()

    pool_id = payload.get("pool_id")
    watchlist_only = bool(payload.get("watchlist_only"))
    rows = _build_screen_rows(
        db,
        day,
        pool_id=None if pool_id is None else int(pool_id),
        watchlist_only=watchlist_only,
        user_id=user_id,
    )
    evaluated = evaluate_conditions(rows, payload)
    combined_count = len(evaluated["items"])
    limit = int(payload.get("limit") or 100)
    items = sorted(evaluated["items"], key=lambda row: row["code"])[:limit]
    return {
        "date": str(day),
        "pool_id": pool_id,
        "watchlist_only": watchlist_only,
        "candidate_count": len(rows),
        "total": combined_count,
        "combined_count": combined_count,
        "independent_counts": evaluated["independent_counts"],
        "condition_counts": evaluated["condition_counts"],
        "field_coverage": evaluated["field_coverage"],
        "data_policy": {
            "point_in_time": True,
            "valuation_max_age_days": VALUATION_MAX_AGE_DAYS,
        },
        "items": items,
    }
