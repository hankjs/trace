"""backtest.get skill。"""
from __future__ import annotations

from typing import Any

from sqlalchemy import select

from ...backtest.listing import _run_summary
from ...models import BacktestRun
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """读取本人回测 run；非本人按 not found 处理。"""
    run_id = payload.get("run_id")
    if run_id is None:
        raise ValueError("payload 必须包含 run_id")
    run = ctx.db.execute(
        select(BacktestRun).where(
            BacktestRun.id == int(run_id),
            BacktestRun.user_id == ctx.user_id,
        )
    ).scalar_one_or_none()
    if run is None:
        raise ValueError(f"回测 {run_id} 不存在")
    return {"backtest_summary": _run_summary(run)}


__all__ = ["handle"]
