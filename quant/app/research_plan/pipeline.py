"""盘后组合研究计划生成流水线。"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.calendar import is_trading_day
from ..data.ingest import load_bars_df
from ..data.universe import (INDEX_NAMES, pool_eligibility_matrix,
                             resolve_pool, resolve_pool_during)
from ..models import Pool, Strategy
from ..strategy.store import enabled_strategies
from .service import create_portfolio_plan

logger = logging.getLogger(__name__)

PORTFOLIO_LOOKBACK_DAYS = 400


def _default_pool(db: Session) -> Pool | None:
    pool = db.execute(
        select(Pool).where(Pool.is_system.is_(True), Pool.kind == "all")
        .order_by(Pool.id)
    ).scalars().first()
    if pool is not None:
        return pool
    return db.execute(
        select(Pool).where(Pool.is_system.is_(True)).order_by(Pool.id)
    ).scalars().first()


def run_portfolio_plans(
    db: Session,
    *,
    day: date,
    pool: Pool | None = None,
    strategies: list[Strategy] | None = None,
) -> dict:
    """为启用的组合策略生成当日调仓或资格变化计划。

    策略实例当前没有自带 pool_id，因此与组合回测缺省逻辑一致，使用系统默认
    全 A 池。若未来策略实例持有池配置，只需让调用方传入对应池，不改变计划表。
    """
    if not is_trading_day(db, day):
        return {"date": str(day), "pool_id": None, "plans": [], "count": 0}
    pool = pool or _default_pool(db)
    if pool is None:
        logger.warning("没有系统股票池，跳过组合研究计划")
        return {"date": str(day), "pool_id": None, "plans": [], "count": 0,
                "skipped": "missing_default_pool"}
    targets = strategies if strategies is not None else enabled_strategies(
        db, kind="portfolio")
    if not targets:
        return {"date": str(day), "pool_id": pool.id, "plans": [], "count": 0}

    index_name = pool.ref if pool.ref in INDEX_NAMES else None
    start = day - timedelta(days=PORTFOLIO_LOOKBACK_DAYS)
    if pool.kind == "index":
        # 起点快照缺失必须先明确报错，不能让区间并集掩盖历史空洞。
        resolve_pool(
            db, start, kind=pool.kind, index_name=index_name, pool_id=pool.id,
            min_list_days=pool.min_list_days)
    codes = resolve_pool_during(
        db, start, day, kind=pool.kind, index_name=index_name, pool_id=pool.id,
        min_list_days=pool.min_list_days)
    if not codes:
        raise ValueError(f"股票池「{pool.name}」在 {day} 没有可用成分")
    pool_dfs = {}
    for code in codes:
        frame = load_bars_df(db, code, start=start, end=day)
        if not frame.empty:
            pool_dfs[code] = frame
    if not pool_dfs:
        raise ValueError(f"股票池「{pool.name}」没有可用于计划的日线")
    dates = sorted({value for frame in pool_dfs.values() for value in frame["date"]})
    eligibility = pool_eligibility_matrix(
        db, dates, list(pool_dfs), kind=pool.kind,
        index_name=index_name, min_list_days=pool.min_list_days,
        daily_frames=pool_dfs,
    )

    created: list[dict] = []
    for strategy in targets:
        try:
            plan = create_portfolio_plan(
                db, strategy, data_date=day, pool_id=pool.id,
                pool_name=pool.name, pool_dfs=pool_dfs,
                eligibility=eligibility)
        except ValueError as exc:
            # 非调仓日且权重无变化是正常无事件；数据或参数问题保留日志，其他
            # 策略仍继续生成，避免一个用户配置拖垮全局盘后流水线。
            logger.info("组合策略「%s」当日未生成计划: %s", strategy.name, exc)
            continue
        created.append({"plan_id": plan.id, "strategy_id": strategy.id,
                        "strategy_name": strategy.name})
    db.commit()
    return {"date": str(day), "pool_id": pool.id, "pool_name": pool.name,
            "plans": created, "count": len(created)}


__all__ = ["run_portfolio_plans"]
