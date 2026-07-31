"""experiment.list skill。"""
from __future__ import annotations

from typing import Any

from ...experiment.service import list_experiments
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """列出本人实验；limit 上限 50。"""
    limit = min(int(payload.get("limit") or 50), 50)
    include_archived = bool(payload.get("include_archived", False))
    items = list_experiments(
        ctx.db, ctx.user_id,
        include_archived=include_archived,
        limit=limit,
        offset=0,
    )
    return {"items": items, "count": len(items)}


__all__ = ["handle"]
