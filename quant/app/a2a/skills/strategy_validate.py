"""strategy.validate skill。"""
from __future__ import annotations

from typing import Any

from ...api.strategies import validation_out
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """严格校验 StrategySpec，返回 validation_result。"""
    spec = payload.get("spec")
    if spec is None:
        raise ValueError("payload 必须包含 spec")
    result = validation_out(spec, db=ctx.db)
    return {
        "validation_result": {
            "valid": result["valid"],
            "capability": result["capability"],
            "spec_hash": result["spec_hash"],
            "errors": result["errors"],
        }
    }


__all__ = ["handle"]
