"""数据覆盖率与信任信号。

回测/选股是否可信,取决于 ST 历史、估值/财务点时数据和复权因子是否完整。
本模块对源表只读;完整报告物化到旁路表 quant_data_quality_cache,
供看板、admin 与回测 metrics.data_quality 使用。

性能约束:quant_daily_bar 可达千万级。全表 COUNT / COUNT(DISTINCT) 会卡死看板,
因此默认 ST/活跃股票口径落在「最近 lookback 日历日」窗口内,走 date 索引;
报告结果写入旁路缓存表(不进进程内存),采集任务结束后 refresh。
"""
from __future__ import annotations

from datetime import date, datetime, timedelta
from typing import Any

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from ..models import (
    AdjustFactor,
    DailyBar,
    DataQualityCache,
    FundamentalSnapshot,
    Stock,
    ValuationSnapshot,
)
from .clock import now_cst, today_cst

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
    "net_margin": "net_margin",
    "debt_ratio": "debt_ratio",
    "cashflow_quality": "cashflow_ratio",
}

# 看板摘要的告警阈值
ST_COMPLETE_WARN = 0.85
ST_COMPLETE_CRITICAL = 0.50
VALUATION_WARN = 0.10

# 股票级 ST 覆盖口径:窗口内非空 is_st bar 占比达到该阈值,才计为「ST 历史完整」。
# 避免一只股票仅靠 1 根非空 bar 就被计为已覆盖(宽口径失真)
ST_STOCK_MIN_KNOWN_SHARE = 0.8

# 财务覆盖率的近期窗口:只统计 report_period 落在 as_of 前 N 天(约最近 4 个报告期)
# 内的财报。窗口固定覆盖 4 个季度末,任意 as_of 下至少 2 个报告期的披露截止日
# (季报 4/8/10 月底、年报 4 月底)已过,正常披露滞后不会被误判为缺口
FUNDAMENTAL_RECENT_DAYS = 365

# 全库扫描代价过高:默认只统计最近 N 个日历日(约 40 个交易日)的 ST / 活跃股票
DEFAULT_LOOKBACK_DAYS = 60

# 旁路缓存固定主键;全库只保留最新一份完整报告
CACHE_SCOPE_LATEST = "latest"


def _ratio(num: int, den: int) -> float | None:
    """占比 0~1。分母为 0(空库/空窗口)时返回 None,与「真的 0% 覆盖」区分。"""
    if den <= 0:
        return None
    return round(num / den, 4)


def _coverage_ratio(num: int, den: int) -> float | None:
    """覆盖率 0~1。分子偶发大于分母(口径窗口不一致)时钳到 1,避免看板 100.02%。
    分母为 0 时返回 None,与「真的 0% 覆盖」区分。"""
    if den <= 0:
        return None
    return round(min(num, den) / den, 4)


def _naive_now() -> datetime:
    return now_cst().replace(tzinfo=None)


def _cache_row(db: Session) -> DataQualityCache | None:
    return db.get(DataQualityCache, CACHE_SCOPE_LATEST)


def _load_cached_report(db: Session) -> dict[str, Any] | None:
    row = _cache_row(db)
    if row is None or not isinstance(row.payload, dict):
        return None
    return row.payload


def _store_report(db: Session, *, as_of: date, report: dict[str, Any]) -> dict[str, Any]:
    """把完整报告写入旁路表并 commit。不触碰任何源表。"""
    now = _naive_now()
    # 在 payload 内嵌 meta,读路径无需二次 join 即可返回 computed_at
    stored = {
        **report,
        "cache": {
            "scope": CACHE_SCOPE_LATEST,
            "as_of": str(as_of),
            "computed_at": now.isoformat(timespec="seconds"),
        },
    }
    row = _cache_row(db)
    if row is None:
        db.add(DataQualityCache(
            scope=CACHE_SCOPE_LATEST,
            as_of=as_of,
            payload=stored,
            computed_at=now,
        ))
    else:
        row.as_of = as_of
        row.payload = stored
        row.computed_at = now
    db.commit()
    return stored


def clear_quality_cache(db: Session | None = None) -> None:
    """清空旁路缓存行。测试或主动作废时调用;不影响源数据。"""
    if db is None:
        return
    row = _cache_row(db)
    if row is not None:
        db.delete(row)
        db.commit()


def refresh_data_quality_cache(
    db: Session,
    *,
    as_of: date | None = None,
) -> dict[str, Any]:
    """强制重算并落库(调度/回填后调用)。源表只读。"""
    as_of = as_of or today_cst()
    report = _build_data_quality_report(db, as_of=as_of, include_fields=True)
    return _store_report(db, as_of=as_of, report=report)


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

    bar 级 = 非空 is_st bar / 窗口内全部 bar;股票级 = 窗口内非空 is_st bar 占比
    >= ST_STOCK_MIN_KNOWN_SHARE 的股票数 / 窗口内有 bar 的股票数(阈值口径,
    只有零星非空 bar 的股票不算已覆盖)。分母为 0 时比率字段为 None。

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

    # 单次扫描:COUNT(col) 自动跳过 NULL;股票总数用 DISTINCT
    q = select(
        func.count().label("total_bars"),
        func.count(DailyBar.is_st).label("known_bars"),
        func.count(func.distinct(DailyBar.code)).label("total_stocks"),
    ).select_from(DailyBar)
    for f in filters:
        q = q.where(f)

    row = db.execute(q).one()
    total_bars = int(row.total_bars or 0)
    known_bars = int(row.known_bars or 0)
    total_stocks = int(row.total_stocks or 0)

    # 股票级口径:窗口内该股票非空 is_st bar 占比 >= ST_STOCK_MIN_KNOWN_SHARE
    # 才计为「ST 历史完整」;只有 1 根非空 bar 的股票不算已覆盖
    qualified_q = (
        select(DailyBar.code)
        .select_from(DailyBar)
        .group_by(DailyBar.code)
        .having(
            func.count(DailyBar.is_st) >= func.count() * ST_STOCK_MIN_KNOWN_SHARE
        )
    )
    for f in filters:
        qualified_q = qualified_q.where(f)
    known_stocks = len(db.execute(qualified_q).all())

    return {
        "total_bars": total_bars,
        "known_bars": known_bars,
        "null_bars": total_bars - known_bars,
        "bar_coverage_ratio": _ratio(known_bars, total_bars),
        "total_stocks_with_bars": total_stocks,
        "stocks_with_st_history": known_stocks,
        "stock_coverage_ratio": _ratio(known_stocks, total_stocks),
        # 股票级口径说明:非空 is_st bar 占比阈值(见 ST_STOCK_MIN_KNOWN_SHARE)
        "stock_min_known_share": ST_STOCK_MIN_KNOWN_SHARE,
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
    """估值/财务在研究日附近的可用覆盖(点时:available_date <= as_of)。

    估值侧限定 data_date 在最近 lookback_days 窗口;财务侧对齐「近期窗口」口径:
    只统计 report_period 落在 as_of 前 FUNDAMENTAL_RECENT_DAYS 天(约最近 4 个
    报告期)内的财报,避免「历史上任何一期有过财报即算覆盖」的失真。窗口内恒有
    披露截止日已过的报告期,正常披露滞后不会被误判为缺口。
    """
    as_of = as_of or today_cst()
    window_start = as_of - timedelta(days=lookback_days)
    fun_period_start = as_of - timedelta(days=FUNDAMENTAL_RECENT_DAYS)

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
        FundamentalSnapshot.report_period >= fun_period_start,
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
                "ratio": _coverage_ratio(available, universe),
            }
        for frame_name, col in _FUNDAMENTAL_FRAME_FIELDS.items():
            column = getattr(FundamentalSnapshot, col)
            q = select(func.count(func.distinct(FundamentalSnapshot.code))).where(
                FundamentalSnapshot.available_date <= as_of,
                FundamentalSnapshot.report_period >= fun_period_start,
                FundamentalSnapshot.report_period <= as_of,
                column.is_not(None),
            )
            if codes is not None:
                q = q.where(FundamentalSnapshot.code.in_(codes))
            available = int(db.execute(q).scalar() or 0)
            fields[frame_name] = {
                "available": available,
                "total": universe,
                "ratio": _coverage_ratio(available, universe),
            }

    return {
        "as_of": str(as_of),
        "lookback_days": lookback_days,
        # 财务侧近期窗口下界:report_period >= 该日期才计入覆盖(约最近 4 个报告期)
        "fundamental_period_start": str(fun_period_start),
        "universe_stocks": universe,
        "valuation_stocks": val_codes,
        "fundamental_stocks": fun_codes,
        # 估值/财务股票集合与「近 lookback 有 bar」宇宙不完全同构,覆盖率钳制到 1
        "valuation_ratio": _coverage_ratio(val_codes, universe),
        "fundamental_ratio": _coverage_ratio(fun_codes, universe),
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
            "net_margin", "debt_ratio", "cashflow_quality",
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
        if stats["total"] and stats["ratio"] is not None and stats["ratio"] < 0.5:
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


def _alert_level(st_ratio: float | None, valuation_ratio: float | None) -> str:
    # None 表示分母为 0(空库/空窗口),无从判定覆盖好坏:
    # 不按 0% 误报 critical/warning,直接跳过该项判定
    if st_ratio is not None and st_ratio < ST_COMPLETE_CRITICAL:
        return "critical"
    if (st_ratio is not None and st_ratio < ST_COMPLETE_WARN) or (
        valuation_ratio is not None and valuation_ratio < VALUATION_WARN
    ):
        return "warning"
    return "ok"


def _build_data_quality_report(
    db: Session,
    *,
    as_of: date,
    include_fields: bool,
) -> dict[str, Any]:
    latest_bar = _latest_bar_date(db)
    # 研究日对齐最新日线:用「今天」当 as_of 时估值窗口会落在无 bar 的日历日上
    research_as_of = as_of
    if latest_bar is not None and as_of > latest_bar:
        research_as_of = latest_bar

    st = st_history_coverage(db)
    snaps = snapshot_coverage(db, as_of=research_as_of, include_fields=include_fields)
    factors = adjust_factor_summary(db)
    stock_count = int(db.execute(select(func.count()).select_from(Stock)).scalar() or 0)
    level = _alert_level(
        st["stock_coverage_ratio"],
        snaps["valuation_ratio"],
    )
    summary = {
        "as_of": str(research_as_of),
        "alert_level": level,
        "stock_count": stock_count,
        "latest_bar_date": str(latest_bar) if latest_bar else None,
        "st_stock_coverage_ratio": st["stock_coverage_ratio"],
        "st_bar_coverage_ratio": st["bar_coverage_ratio"],
        "valuation_coverage_ratio": snaps["valuation_ratio"],
        "fundamental_coverage_ratio": snaps["fundamental_ratio"],
        "adjust_factor_missing_stocks": factors["missing_factor_stocks"],
        # 前端可读的口径提示(覆盖率非涨跌幅)
        "st_window_days": st.get("lookback_days"),
        "st_window_start": st.get("window_start"),
        "st_window_end": st.get("window_end"),
    }
    return {
        "summary": summary,
        "st_history": st,
        "snapshots": snaps,
        "adjust_factors": factors,
    }


def data_quality_report(
    db: Session,
    *,
    as_of: date | None = None,
    force: bool = False,
) -> dict[str, Any]:
    """全库数据信任报告(admin / 看板摘要共用)。

    默认读旁路缓存;force=True 或缓存缺失时现算并写回。
    as_of 仅在重算时生效(缓存固定一份 latest)。
    """
    if not force:
        cached = _load_cached_report(db)
        if cached is not None:
            return cached
    as_of = as_of or today_cst()
    report = _build_data_quality_report(db, as_of=as_of, include_fields=True)
    return _store_report(db, as_of=as_of, report=report)


def data_quality_public_summary(
    db: Session,
    *,
    as_of: date | None = None,
    force: bool = False,
) -> dict[str, Any]:
    """登录用户可读摘要:只暴露比率与告警,不含运维细节表。

    与完整报告共用旁路缓存行;缓存命中时不再扫源表。
    """
    report = data_quality_report(db, as_of=as_of, force=force)
    summary = dict(report.get("summary") or {})
    cache_meta = report.get("cache") if isinstance(report.get("cache"), dict) else {}
    if cache_meta.get("computed_at"):
        summary["computed_at"] = cache_meta["computed_at"]
    return summary


__all__ = [
    "adjust_factor_summary",
    "clear_quality_cache",
    "data_quality_public_summary",
    "data_quality_report",
    "frames_data_quality",
    "refresh_data_quality_cache",
    "snapshot_coverage",
    "st_history_coverage",
]
