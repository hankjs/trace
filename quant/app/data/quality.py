"""数据覆盖率与信任信号。

回测/选股是否可信,取决于 ST 历史、估值/财务点时数据和复权因子是否完整。
本模块只做只读汇总,不触发采集;供看板、admin 与回测 metrics.data_quality 使用。
"""
from __future__ import annotations

from datetime import date, timedelta
from typing import Any

from sqlalchemy import func, select
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


def _ratio(num: int, den: int) -> float:
    if den <= 0:
        return 0.0
    return round(num / den, 4)


def st_history_coverage(
    db: Session,
    *,
    codes: list[str] | None = None,
    start: date | None = None,
    end: date | None = None,
) -> dict[str, Any]:
    """逐日 is_st 覆盖率(bar 级 + 股票级)。"""
    filters = []
    if codes is not None:
        filters.append(DailyBar.code.in_(codes))
    if start is not None:
        filters.append(DailyBar.date >= start)
    if end is not None:
        filters.append(DailyBar.date <= end)

    total_q = select(func.count()).select_from(DailyBar)
    known_q = select(func.count()).select_from(DailyBar).where(
        DailyBar.is_st.is_not(None),
    )
    stock_q = select(func.count(func.distinct(DailyBar.code)))
    known_stock_q = select(func.count(func.distinct(DailyBar.code))).where(
        DailyBar.is_st.is_not(None),
    )
    for f in filters:
        total_q = total_q.where(f)
        known_q = known_q.where(f)
        stock_q = stock_q.where(f)
        known_stock_q = known_stock_q.where(f)

    total_bars = int(db.execute(total_q).scalar() or 0)
    known_bars = int(db.execute(known_q).scalar() or 0)
    total_stocks = int(db.execute(stock_q).scalar() or 0)
    known_stocks = int(db.execute(known_stock_q).scalar() or 0)
    return {
        "total_bars": total_bars,
        "known_bars": known_bars,
        "null_bars": total_bars - known_bars,
        "bar_coverage_ratio": _ratio(known_bars, total_bars),
        "total_stocks_with_bars": total_stocks,
        "stocks_with_st_history": known_stocks,
        "stock_coverage_ratio": _ratio(known_stocks, total_stocks),
        "incomplete": total_bars > 0 and known_bars < total_bars,
    }


def adjust_factor_summary(db: Session) -> dict[str, Any]:
    """复权因子覆盖:权威 baostock / 自算 sina / 有日线无因子。"""
    by_source = dict(
        db.execute(
            select(AdjustFactor.source, func.count(func.distinct(AdjustFactor.code)))
            .group_by(AdjustFactor.source)
        ).all()
    )
    stocks_with_bars = int(db.execute(
        select(func.count(func.distinct(DailyBar.code)))
    ).scalar() or 0)
    stocks_with_factors = int(db.execute(
        select(func.count(func.distinct(AdjustFactor.code)))
    ).scalar() or 0)
    return {
        "stocks_with_bars": stocks_with_bars,
        "stocks_with_factors": stocks_with_factors,
        "by_source": {str(k): int(v) for k, v in by_source.items()},
        "missing_factor_stocks": max(0, stocks_with_bars - stocks_with_factors),
    }


def snapshot_coverage(
    db: Session,
    *,
    as_of: date | None = None,
    codes: list[str] | None = None,
    lookback_days: int = 7,
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


def data_quality_report(db: Session, *, as_of: date | None = None) -> dict[str, Any]:
    """全库数据信任报告(admin / 看板摘要共用)。"""
    as_of = as_of or today_cst()
    st = st_history_coverage(db)
    snaps = snapshot_coverage(db, as_of=as_of)
    factors = adjust_factor_summary(db)
    stock_count = int(db.execute(select(func.count()).select_from(Stock)).scalar() or 0)
    latest_bar = db.execute(select(func.max(DailyBar.date))).scalar()
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


def data_quality_public_summary(db: Session, *, as_of: date | None = None) -> dict[str, Any]:
    """登录用户可读摘要:只暴露比率与告警,不含运维细节表。"""
    report = data_quality_report(db, as_of=as_of)
    return report["summary"]


__all__ = [
    "adjust_factor_summary",
    "data_quality_public_summary",
    "data_quality_report",
    "frames_data_quality",
    "snapshot_coverage",
    "st_history_coverage",
]
