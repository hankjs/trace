"""backtest.list skill。"""
from __future__ import annotations

from typing import Any

from ...backtest.listing import list_runs
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """列出本人回测；limit 上限 50。"""
    limit = min(int(payload.get("limit") or 20), 50)
    strategy_id = payload.get("strategy_id")
    before_run_id = payload.get("before_run_id")
    # artifact 名对齐契约 §8.8:backtest_list = { items, has_more }
    return {
        "backtest_list": list_runs(
            ctx.db,
            user_id=ctx.user_id,
            strategy_id=int(strategy_id) if strategy_id is not None else None,
            limit=limit,
            before_run_id=int(before_run_id) if before_run_id is not None else None,
        )
    }


__all__ = ["handle"]
