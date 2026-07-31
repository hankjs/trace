"""catalog.get skill。"""
from __future__ import annotations

from typing import Any

from ...catalog import catalog_payload
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """返回固定研究目录，支持按 sections 裁剪。"""
    sections = payload.get("sections")
    full = catalog_payload(ctx.db)
    if sections:
        if isinstance(sections, str):
            sections = [sections]
        allowed = set(sections)
        full = {
            k: v for k, v in full.items()
            if k in allowed or k in {"version", "product_boundary"}
        }
    return {"catalog": full}


__all__ = ["handle"]
