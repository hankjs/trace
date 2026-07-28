"""数据覆盖率与信任信号。

回测/选股是否可信,取决于 ST 历史、估值/财务点时数据和复权因子是否完整。
本模块只做只读汇总,不触发采集;供看板、admin 与回测 metrics.data_quality 使用。

性能约束:quant_daily_bar 可达千万级。全表 COUNT / COUNT(DISTINCT) 会卡死看板,
因此默认 ST/活跃股票口径落在「最近 lookback 日历日」窗口内,走 date 索引;
进程内短 TTL 缓存避免重复重扫。
"""
from __future__ import annotations

import threading
import time
from datetime import date, timedelta
from typing import Any

from sqlalchemy import case, func, select
from sqlalchemy.orm import Session

from ..models import (
    AdjustFactor,
    DailyBar,
    FundamentalSnapshot,
    Stock,
    ValuationSnapshot,
)
from .clock import today_cst

# 回测 frames 侧字段名 → 库内列名(估值表 total_market_cap / 财务表 yoy 命名不同)
_VALUATION_FRAME_FIELDS = {
    "pe_ttm": "pe_ttm",
    "pb": "pb",
    "ps_ttm": "ps_ttm",
    "market_cap": "total_market_cap",
}
_FUNDAMENTAL_FRAME_FIELDS = {
    "roe": "roe",
    "revenue_growth": "revenue_yoy",
    "profit_growth": "profit_yoy",
    "gross_margin": "gross_margin",
    "debt_ratio": "debt_ratio",
    "cashflow_quality": "cashflow_ratio",
}

# 看板摘要的告警阈值
ST_COMPLETE_WARN = 0.85
ST_COMPLETE_CRITICAL = 0.50
VALUATION_WARN = 0.10

# 全库扫描代价过高:默认只统计最近 N 个日历日(约 40 个交易日)的 ST / 活跃股票
DEFAULT_LOOKBACK_DAYS = 60
# 看板/admin 共享的报告缓存(秒);数据质量非秒级变化
REPORT_CACHE_TTL_SECONDS = 300.0

_cache_lock = threading.Lock()
_report_cache: dict[str, tuple[float, Any]] = {}


def _ratio(num: int, den: int) -> float:
    if den <= 0:
        return 0.0
    return round(num / den, 4)


def _cache_get(key: str) -> Any | None:
    with _cache_lock:
        hit = _report_cache.get(key)
        if hit is None:
            return None
        ts, value = hit
        if time.monotonic() - ts >= REPORT_CACHE_TTL_SECONDS:
            _report_cache.pop(key, None)
            return None
        return value


def _cache_set(key: str, value: Any) -> Any:
    with _cache_lock:
        _report_cache[key] = (time.monotonic(), value)
    return value


def clear_quality_cache() -> None:
    """测试或主动刷新时清空进程内缓存。"""
    with _cache_lock:
        _report_cache.clear()


def _latest_bar_date(db: Session) -> date | None:
    return db.execute(select(func.max(DailyBar.date))).scalar()


def _default_window(
    db: Session,
    *,
    lookback_days: int = DEFAULT_LOOKBACK_DAYS,
) -> tuple[date | None, date | None]:
    """无显式区间时,用最新 bar 日期回推 lookback 天,避免全表扫。"""
    latest = _latest_bar_date(db)
    if latest is None:
        return None, None
    return latest - timedelta(days=lookback_days), latest


def st_history_coverage(
    db: Session,
    *,
    codes: list[str] | None = None,
    start: date | None = None,
    end: date | None = None,
    lookback_days: int | None = DEFAULT_LOOKBACK_DAYS,
) -> dict[str, Any]:
    """逐日 is_st 覆盖率(bar 级 + 股票级)。

    未指定 start/end/codes 时默认只扫最近 lookback_days 日历日(走 date 索引)。
    传入 start/end 或 codes 则按调用方范围统计;lookback_days=None 且无范围时
    才全表扫描(运维慎用)。
    """
    scope = "custom"
    window_start = start
    window_end = end
    if window_start is None and window_end is None and codes is None:
        if lookback_days is not None:
            window_start, window_end = _default_window(db, lookback_days=lookback_days)
            scope = "recent_window"
        else:
            scope = "full"

    filters = []
    if codes is not None:
        filters.append(DailyBar.code.in_(codes))
    if window_start is not None:
        filters.append(DailyBar.date >= window_start)
    if window_end is not None:
        filters.append(DailyBar.date <= window_end)

    # 单次扫描:COUNT(col) 自动跳过 NULL;股票级用 CASE 去重
    known_code = case((DailyBar.is_st.is_not(None), DailyBar.code), else_=None)
    q = select(
        func.count().label("total_bars"),
        func.count(DailyBar.is_st).label("known_bars"),
        func.count(func.distinct(DailyBar.code)).label("total_stocks"),
        func.count(func.distinct(known_code)).label("known_stocks"),
    ).select_from(DailyBar)
    for f in filters:
        q = q.where(f)

    row = db.execute(q).one()
    total_bars = int(row.total_bars or 0)
    known_bars = int(row.known_bars or 0)
    total_stocks = int(row.total_stocks or 0)
    known_stocks = int(row.known_stocks or 0)
    return {
        "total_bars": total_bars,
        "known_bars": known_bars,
        "null_bars": total_bars - known_bars,
        "bar_coverage_ratio": _ratio(known_bars, total_bars),
        "total_stocks_with_bars": total_stocks,
        "stocks_with_st_history": known_stocks,
        "stock_coverage_ratio": _ratio(known_stocks, total_stocks),
        "incomplete": total_bars > 0 and known_bars < total_bars,
        "scope": scope,
        "lookback_days": lookback_days if scope == "recent_window" else None,
        "window_start": str(window_start) if window_start else None,
        "window_end": str(window_end) if window_end else None,
    }


def adjust_factor_summary(
    db: Session,
    *,
    lookback_days: int = DEFAULT_LOOKBACK_DAYS,
) -> dict[str, Any]:
    """复权因子覆盖:权威 baostock / 自算 sina / 近期有日线无因子。

    stocks_with_bars 取最近 lookback 内出现过的股票,避免对日线全表 DISTINCT。
    """
    by_source = dict(
        db.execute(
            select(AdjustFactor.source, func.count(func.distinct(AdjustFactor.code)))
            .group_by(AdjustFactor.source)
        ).all()
    )
    window_start, window_end = _default_window(db, lookback_days=lookback_days)
    if window_start is None or window_end is None:
        stocks_with_bars = 0
    else:
        stocks_with_bars = int(db.execute(
            select(func.count(func.distinct(DailyBar.code))).where(
                DailyBar.date >= window_start,
                DailyBar.date <= window_end,
            )
        ).scalar() or 0)
    stocks_with_factors = int(db.execute(
        select(func.count(func.distinct(AdjustFactor.code)))
    ).scalar() or 0)
    return {
        "stocks_with_bars": stocks_with_bars,
        "stocks_with_factors": stocks_with_factors,
        "by_source": {str(k): int(v) for k, v in by_source.items()},
        "missing_factor_stocks": max(0, stocks_with_bars - stocks_with_factors),
        "lookback_days": lookback_days,
        "window_start": str(window_start) if window_start else None,
        "window_end": str(window_end) if window_end else None,
    }


def snapshot_coverage(
    db: Session,
    *,
    as_of: date | None = None,
    codes: list[str] | None = None,
    lookback_days: int = 7,
    include_fields: bool = True,
) -> dict[str, Any]:
    """估值/财务在研究日附近的可用覆盖(点时:available_date <= as_of)。"""
    as_of = as_of or today_cst()
    window_start = as_of - timedelta(days=lookback_days)

    universe_q = select(func.count(func.distinct(DailyBar.code))).where(
        DailyBar.date >= window_start,
        DailyBar.date <= as_of,
    )
    if codes is not None:
        universe_q = universe_q.where(DailyBar.code.in_(codes))
    universe = int(db.execute(universe_q).scalar() or 0)

    val_codes_q = select(func.count(func.distinct(ValuationSnapshot.code))).where(
        ValuationSnapshot.available_date <= as_of,
        ValuationSnapshot.data_date >= window_start,
        ValuationSnapshot.data_date <= as_of,
    )
    fun_codes_q = select(func.count(func.distinct(FundamentalSnapshot.code))).where(
        FundamentalSnapshot.available_date <= as_of,
        FundamentalSnapshot.report_period <= as_of,
    )
    if codes is not None:
        val_codes_q = val_codes_q.where(ValuationSnapshot.code.in_(codes))
        fun_codes_q = fun_codes_q.where(FundamentalSnapshot.code.in_(codes))

    val_codes = int(db.execute(val_codes_q).scalar() or 0)
    fun_codes = int(db.execute(fun_codes_q).scalar() or 0)

    fields: dict[str, dict[str, Any]] = {}
    if include_fields:
        for frame_name, col in _VALUATION_FRAME_FIELDS.items():
            column = getattr(ValuationSnapshot, col)
            q = select(func.count(func.distinct(ValuationSnapshot.code))).where(
                ValuationSnapshot.available_date <= as_of,
                ValuationSnapshot.data_date >= window_start,
                ValuationSnapshot.data_date <= as_of,
                column.is_not(None),
            )
            if codes is not None:
                q = q.where(ValuationSnapshot.code.in_(codes))
            available = int(db.execute(q).scalar() or 0)
            fields[frame_name] = {
                "available": available,
                "total": universe,
                "ratio": _ratio(available, universe),
            }
        for frame_name, col in _FUNDAMENTAL_FRAME_FIELDS.items():
            column = getattr(FundamentalSnapshot, col)
            q = select(func.count(func.distinct(FundamentalSnapshot.code))).where(
                FundamentalSnapshot.available_date <= as_of,
                FundamentalSnapshot.report_period <= as_of,
                column.is_not(None),
            )
            if codes is not None:
                q = q.where(FundamentalSnapshot.code.in_(codes))
            available = int(db.execute(q).scalar() or 0)
            fields[frame_name] = {
                "available": available,
                "total": universe,
                "ratio": _ratio(available, universe),
            }

    return {
        "as_of": str(as_of),
        "lookback_days": lookback_days,
        "universe_stocks": universe,
        "valuation_stocks": val_codes,
        "fundamental_stocks": fun_codes,
        "valuation_ratio": _ratio(val_codes, universe),
        "fundamental_ratio": _ratio(fun_codes, universe),
        "fields": fields,
    }


def frames_data_quality(
    frames: dict[str, Any],
    *,
    required_fields: list[str] | None = None,
    max_incomplete_codes: int = 20,
) -> dict[str, Any]:
    """从已加载的回测 DataFrame 字典计算 data_quality(不访问 DB)。"""
    if not frames:
        return {
            "st_history_incomplete": True,
            "st_null_bar_ratio": 1.0,
            "st_incomplete_codes": [],
            "st_incomplete_code_count": 0,
            "field_coverage": {},
            "warnings": ["无可用日线数据"],
        }

    total_bars = 0
    null_st_bars = 0
    incomplete_codes: list[str] = []
    for code, frame in frames.items():
        if "is_st" not in frame.columns:
            incomplete_codes.append(code)
            total_bars += len(frame)
            null_st_bars += len(frame)
            continue
        series = frame["is_st"]
        n = len(series)
        nulls = int(series.isna().sum())
        total_bars += n
        null_st_bars += nulls
        if nulls > 0:
            incomplete_codes.append(code)

    st_ratio = _ratio(null_st_bars, total_bars)
    field_coverage: dict[str, dict[str, Any]] = {}
    fields = required_fields or []
    extra = [
        c for frame in frames.values() for c in frame.columns
        if c in (
            "pe_ttm", "pb", "ps_ttm", "market_cap", "roe",
            "revenue_growth", "profit_growth", "gross_margin",
            "debt_ratio", "cashflow_quality",
        )
    ]
    for field in sorted(set(fields) | set(extra)):
        available = 0
        total = 0
        for frame in frames.values():
            if field not in frame.columns:
                total += len(frame)
                continue
            series = frame[field]
            total += len(series)
            available += int(series.notna().sum())
        field_coverage[field] = {
            "available": available,
            "total": total,
            "ratio": _ratio(available, total),
        }

    warnings: list[str] = []
    incomplete = null_st_bars > 0 or any(
        "is_st" not in f.columns for f in frames.values()
    )
    if incomplete:
        warnings.append(
            f"ST 历史不完整: {null_st_bars}/{total_bars} 根 bar 的 is_st 为空,"
            f"涉及 {len(incomplete_codes)} 只股票;历史池解析不会回退当前 ST 标记"
        )
    for field, stats in field_coverage.items():
        if stats["total"] and stats["ratio"] < 0.5:
            warnings.append(
                f"字段 {field} 覆盖率仅 {stats['ratio']:.0%}"
                f"({stats['available']}/{stats['total']})"
            )

    return {
        "st_history_incomplete": incomplete,
        "st_null_bar_ratio": st_ratio,
        "st_incomplete_codes": incomplete_codes[:max_incomplete_codes],
        "st_incomplete_code_count": len(incomplete_codes),
        "field_coverage": field_coverage,
        "warnings": warnings,
    }


def _alert_level(st_ratio: float, valuation_ratio: float) -> str:
    if st_ratio < ST_COMPLETE_CRITICAL:
        return "critical"
    if st_ratio < ST_COMPLETE_WARN or valuation_ratio < VALUATION_WARN:
        return "warning"
    return "ok"


def _build_data_quality_report(
    db: Session,
    *,
    as_of: date,
    include_fields: bool,
) -> dict[str, Any]:
    st = st_history_coverage(db)
    snaps = snapshot_coverage(db, as_of=as_of, include_fields=include_fields)
    factors = adjust_factor_summary(db)
    stock_count = int(db.execute(select(func.count()).select_from(Stock)).scalar() or 0)
    latest_bar = _latest_bar_date(db)
    level = _alert_level(
        st["stock_coverage_ratio"],
        snaps["valuation_ratio"],
    )
    summary = {
        "as_of": str(as_of),
        "alert_level": level,
        "stock_count": stock_count,
        "latest_bar_date": str(latest_bar) if latest_bar else None,
        "st_stock_coverage_ratio": st["stock_coverage_ratio"],
        "st_bar_coverage_ratio": st["bar_coverage_ratio"],
        "valuation_coverage_ratio": snaps["valuation_ratio"],
        "fundamental_coverage_ratio": snaps["fundamental_ratio"],
        "adjust_factor_missing_stocks": factors["missing_factor_stocks"],
    }
    return {
        "summary": summary,
        "st_history": st,
        "snapshots": snaps,
        "adjust_factors": factors,
    }


def data_quality_report(db: Session, *, as_of: date | None = None) -> dict[str, Any]:
    """全库数据信任报告(admin / 看板摘要共用)。"""
    as_of = as_of or today_cst()
    cache_key = f"report:{as_of.isoformat()}"
    cached = _cache_get(cache_key)
    if cached is not None:
        return cached
    report = _build_data_quality_report(db, as_of=as_of, include_fields=True)
    return _cache_set(cache_key, report)


def data_quality_public_summary(db: Session, *, as_of: date | None = None) -> dict[str, Any]:
    """登录用户可读摘要:只暴露比率与告警,不含运维细节表。

    不跑 admin 字段级覆盖明细;并与完整报告共享缓存键前缀策略,独立缓存。
    """
    as_of = as_of or today_cst()
    cache_key = f"public:{as_of.isoformat()}"
    cached = _cache_get(cache_key)
    if cached is not None:
        return cached
    report = _build_data_quality_report(db, as_of=as_of, include_fields=False)
    return _cache_set(cache_key, report["summary"])


__all__ = [
    "adjust_factor_summary",
    "clear_quality_cache",
    "data_quality_public_summary",
    "data_quality_report",
    "frames_data_quality",
    "snapshot_coverage",
    "st_history_coverage",
]
