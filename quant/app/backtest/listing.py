"""回测 run 列表 domain 查询。"""
from __future__ import annotations

from copy import deepcopy
from datetime import date
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import BacktestRun

MAX_LIST_LIMIT = 50


def _run_summary(run: BacktestRun) -> dict[str, Any]:
    """与 GET /api/backtest/{run_id} summary 段一致的字段子集。

    validation 除保留 baselines/oos/rejection 全量外,把 rejection 的
    verdict/reasons 平铺到顶层(A2A 契约 §8.4 的 validation.verdict 形态):
    verdict ∈ passed | rejected | incomplete;reasons 为命中与未评估明细。
    """
    metrics = run.metrics or {}
    evidence = metrics.get("evidence") if isinstance(metrics.get("evidence"), dict) else {}
    validation = deepcopy(metrics.get("validation")) or {}
    rejection = validation.get("rejection") or {}
    if validation and "verdict" not in validation:
        reasons = [
            h.get("detail") or h.get("criterion") or str(h)
            for h in rejection.get("hits") or []
        ] + [
            u.get("reason") or u.get("criterion") or str(u)
            for u in rejection.get("unevaluated") or []
        ]
        validation["verdict"] = rejection.get("verdict")
        validation["reasons"] = reasons
    return {
        "run_id": run.id,
        "strategy_id": run.strategy_id,
        "status": getattr(run, "status", None) or "done",
        "error": getattr(run, "error", None),
        "start": str(run.start),
        "end": str(run.end),
        "codes": run.codes,
        "pool_id": run.pool_id,
        "costs": run.costs or {},
        "metrics": {
            "total_return": metrics.get("total_return"),
            "annual_return": metrics.get("annual_return"),
            "max_drawdown": metrics.get("max_drawdown"),
            "sharpe": metrics.get("sharpe"),
            "win_rate": metrics.get("win_rate"),
            "trade_count": metrics.get("trade_count"),
            "round_trips": metrics.get("round_trips"),
        },
        "validation": validation,
        "data_quality": metrics.get("data_quality"),
        "strategy_spec_hash": run.strategy_spec_hash,
        "execution_fingerprint": run.execution_fingerprint,
        "created_at": run.created_at.isoformat(sep=" ") if run.created_at else None,
        "started_at": (
            run.started_at.isoformat(sep=" ") if run.started_at else None
        ),
        "finished_at": (
            run.finished_at.isoformat(sep=" ") if run.finished_at else None
        ),
    }


def list_runs(
    db: Session,
    *,
    user_id: str,
    strategy_id: int | None = None,
    limit: int = 20,
    before_run_id: int | None = None,
) -> dict[str, Any]:
    """仅本人、id 倒序、limit clamp 到 50、before_run_id 游标分页。"""
    limit = max(1, min(int(limit), MAX_LIST_LIMIT))
    q = select(BacktestRun).where(BacktestRun.user_id == user_id)
    if strategy_id is not None:
        q = q.where(BacktestRun.strategy_id == strategy_id)
    if before_run_id is not None:
        q = q.where(BacktestRun.id < int(before_run_id))
    q = q.order_by(BacktestRun.id.desc()).limit(limit + 1)
    rows = list(db.execute(q).scalars().all())
    has_more = len(rows) > limit
    items = [_run_summary(run) for run in rows[:limit]]
    return {"items": items, "has_more": has_more}


__all__ = ["list_runs", "_run_summary"]
