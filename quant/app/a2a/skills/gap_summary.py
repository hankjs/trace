"""system.gap_summary skill。"""
from __future__ import annotations

from typing import Any

from ..gaps import aggregate_gaps
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """聚合审计缺口列与 research findings。"""
    scope = str(payload.get("scope") or "me")
    if scope == "global" and not ctx.claims.get("can_admin"):
        raise ValueError("scope=global 仅管理员可用")
    limit = min(int(payload.get("limit") or 20), 50)
    since_days = min(int(payload.get("since_days") or 30), 90)

    result = aggregate_gaps(
        ctx.db,
        user_id=ctx.user_id,
        scope=scope,
        limit=limit,
        since_days=since_days,
    )
    return {"gap_summary": result}


__all__ = ["handle"]
