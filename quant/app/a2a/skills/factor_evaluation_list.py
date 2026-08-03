"""factor.evaluation_list skill。"""
from __future__ import annotations

from typing import Any

from ...factors.listing import list_evaluations
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """列出本人因子评估摘要；limit 上限 50，before_id 游标分页。"""
    before_id = payload.get("before_id")
    status = payload.get("status")
    if status is not None:
        status = str(status)
        if status not in {"running", "done", "failed", "cancelled"}:
            raise ValueError(
                "status 只支持 running / done / failed / cancelled"
            )
    return {
        "factor_evaluation_list": list_evaluations(
            ctx.db,
            user_id=ctx.user_id,
            factor_key=payload.get("factor_key"),
            status=status,
            limit=int(payload.get("limit") or 20),
            before_id=int(before_id) if before_id is not None else None,
        )
    }


__all__ = ["handle"]
