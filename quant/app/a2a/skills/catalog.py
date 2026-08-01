"""catalog.get skill。"""
from __future__ import annotations

from typing import Any

from ...catalog import A2A_CATALOG_SECTIONS, catalog_payload
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """返回固定研究目录，支持按 sections 裁剪。"""
    sections = payload.get("sections")
    full = catalog_payload(ctx.db, include_strategy_authoring=True)
    if sections is not None:
        if isinstance(sections, str):
            sections = [sections]
        if not isinstance(sections, list) or not all(
            isinstance(section, str) for section in sections
        ):
            raise ValueError("sections 必须是字符串数组")
        unknown = sorted(set(sections) - set(A2A_CATALOG_SECTIONS))
        if unknown:
            raise ValueError(
                f"未知 catalog sections: {unknown}; 可用值: {list(A2A_CATALOG_SECTIONS)}"
            )
        allowed = set(sections)
        full = {
            k: v for k, v in full.items()
            if k in allowed or k in {"version", "product_boundary"}
        }
    return {"catalog": full}


__all__ = ["handle"]
