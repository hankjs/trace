"""factor.validate skill。"""
from __future__ import annotations

from typing import Any

from ...strategy.spec import validate_expression
from ._common import A2AContext


def _available_fields(db):
    from ...data.ingest import BAR_FIELDS, snapshot_available_fields
    return BAR_FIELDS | snapshot_available_fields(db)


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """校验因子表达式，返回 ExpressionValidationResult 字段。"""
    expression = payload.get("expression")
    if expression is None:
        raise ValueError("payload 必须包含 expression")
    result = validate_expression(
        expression, require_type="number", available_fields=_available_fields(ctx.db),
    )
    return {
        "factor_validation": result.model_dump(mode="json"),
    }


__all__ = ["handle"]
