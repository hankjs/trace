"""market.data_quality skill。"""
from __future__ import annotations

from typing import Any

from ...data.quality import data_quality_public_summary
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """无参快照；忽略任何 payload 字段。"""
    return {"data_quality": data_quality_public_summary(ctx.db)}


__all__ = ["handle"]
