"""selection.screen skill。"""
from __future__ import annotations

from typing import Any

from ...selection.screener import InvalidFilterError, structured_screen
from ._common import A2AContext


MAX_SCREEN_LIMIT = 50


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """结构化筛选；limit 上限 50。"""
    payload = dict(payload)
    limit = min(int(payload.get("limit") or 100), MAX_SCREEN_LIMIT)
    payload["limit"] = limit
    try:
        result = structured_screen(ctx.db, payload, user_id=ctx.user_id)
    except InvalidFilterError as exc:
        raise ValueError(str(exc)) from exc
    return {
        "screen_result": {
            "date": result["date"],
            "total": result["total"],
            "items": result["items"],
            "field_coverage": result["field_coverage"],
            "truncated": result["total"] > len(result["items"]),
        }
    }


__all__ = ["handle"]
