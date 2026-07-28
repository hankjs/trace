"""异步回测作业:pending → running → done|failed。

HTTP 层先冻结 request_snapshot 与 StrategySpec,插入 pending 行后返回 202;
worker 用独立 Session 抢占并执行,不重读当前策略行(只用快照)。
"""
from __future__ import annotations

import logging
from datetime import datetime
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..db import SessionLocal
from ..models import BacktestRun, Strategy
from ..strategy.evidence import advance_after_backtest
from .engine import run_backtest

logger = logging.getLogger(__name__)

CLAIMABLE = "pending"
RUNNING = "running"
DONE = "done"
FAILED = "failed"


def claim_run(db: Session, run_id: int) -> BacktestRun | None:
    """原子抢占:仅 pending → running 成功时返回行。"""
    stmt = select(BacktestRun).where(
        BacktestRun.id == run_id,
        BacktestRun.status == CLAIMABLE,
    )
    # MySQL 下用 FOR UPDATE 降低双 worker 竞态;sqlite 测试环境无此语义
    try:
        dialect = db.get_bind().dialect.name
    except Exception:  # noqa: BLE001
        dialect = ""
    if dialect == "mysql":
        stmt = stmt.with_for_update()
    run = db.execute(stmt).scalar_one_or_none()
    if run is None:
        return None
    run.status = RUNNING
    run.started_at = datetime.now()
    run.error = None
    db.commit()
    db.refresh(run)
    return run


def mark_failed(db: Session, run_id: int, message: str) -> None:
    run = db.get(BacktestRun, run_id)
    if run is None:
        return
    run.status = FAILED
    run.error = message[:4000]
    run.finished_at = datetime.now()
    db.commit()


def execute_backtest_run(run_id: int) -> None:
    """后台入口:独立 Session 执行并推进证据。"""
    with SessionLocal() as db:
        try:
            run = claim_run(db, run_id)
            if run is None:
                logger.info("回测作业 %s 不可抢占(已处理或不存在)", run_id)
                return
            snapshot = run.request_snapshot or {}
            strategy = db.get(Strategy, run.strategy_id)
            if strategy is None:
                mark_failed(db, run_id, "策略已被删除,无法执行回测")
                return
            execution_spec = run.strategy_spec_snapshot
            if not execution_spec:
                mark_failed(db, run_id, "缺少冻结的策略规格快照")
                return
            codes = list(run.codes or snapshot.get("codes") or [])
            result = run_backtest(
                db,
                strategy,
                codes,
                run.start,
                run.end,
                params=snapshot.get("params") or {},
                costs=run.costs or snapshot.get("costs") or {},
                save=True,
                dynamic_universe=bool(snapshot.get("dynamic_universe")),
                user_id=run.user_id,
                pool_id=run.pool_id,
                execution_spec=execution_spec,
                run_id=run.id,
            )
            transition = None
            try:
                # 重新取策略行:evidence 写在当前策略上,与冻结快照按身份哈希匹配
                strategy = db.get(Strategy, run.strategy_id)
                if strategy is not None:
                    transition = advance_after_backtest(db, strategy, result)
                    if transition:
                        db.commit()
            except Exception:  # noqa: BLE001
                logger.exception("证据状态推进失败 run_id=%s", run_id)
                db.rollback()
            logger.info(
                "回测作业完成 run_id=%s transition=%s", run_id, transition,
            )
        except Exception as exc:  # noqa: BLE001
            logger.exception("回测作业失败 run_id=%s", run_id)
            try:
                db.rollback()
                mark_failed(db, run_id, str(exc))
            except Exception:  # noqa: BLE001
                logger.exception("标记失败状态时出错 run_id=%s", run_id)


def pending_payload(run: BacktestRun) -> dict[str, Any]:
    return {
        "run_id": run.id,
        "status": run.status,
        "strategy_id": run.strategy_id,
        "start": str(run.start),
        "end": str(run.end),
        "codes": run.codes or [],
        "pool_id": run.pool_id,
        "strategy_spec_hash": run.strategy_spec_hash,
        "created_at": run.created_at.isoformat(sep=" ") if run.created_at else None,
        "error": run.error,
    }


__all__ = [
    "CLAIMABLE", "DONE", "FAILED", "RUNNING",
    "claim_run", "execute_backtest_run", "mark_failed", "pending_payload",
]
