"""factor.save_draft skill。"""
from __future__ import annotations

from typing import Any

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError

from ...catalog import FILTER_FIELDS
from ...models import FactorDef
from ...strategy.spec import SUPPORTED_FIELDS, validate_expression
from ._common import A2AContext


def _available_fields(db):
    from ...data.ingest import BAR_FIELDS, snapshot_available_fields
    return BAR_FIELDS | snapshot_available_fields(db)


def _factor_out(def_: FactorDef) -> dict[str, Any]:
    return {
        "id": def_.id,
        "key": def_.key,
        "name": def_.name,
        "description": def_.description or None,
        "category": def_.category or None,
        "expression": def_.expression,
        "expression_hash": def_.expression_hash,
        "min_bars": def_.min_bars,
        "enabled": bool(def_.enabled),
        "is_system": bool(def_.is_system),
        "created_at": def_.created_at.isoformat(sep=" ") if def_.created_at else None,
    }


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """admin 保存 disabled 因子草稿。"""
    if not ctx.claims.get("can_admin"):
        raise ValueError("factor.save_draft 仅管理员可用")
    if payload.get("enabled") is True:
        raise ValueError("A2A 保存因子草稿固定 enabled=false，不允许传入 enabled:true")

    key = str(payload["key"])
    if key in FILTER_FIELDS or key in SUPPORTED_FIELDS:
        raise ValueError(f"key {key} 与系统保留字段冲突，请更换")

    expression = payload["expression"]
    if expression is None:
        raise ValueError("payload 必须包含 expression")
    result = validate_expression(
        expression, require_type="number", available_fields=_available_fields(ctx.db),
    )
    if not result.valid:
        raise ValueError("表达式校验失败")

    def_ = FactorDef(
        key=key,
        name=str(payload["name"]),
        description=str(payload.get("description") or ""),
        category=str(payload.get("category") or ""),
        expression=expression,
        expression_hash=result.expression_hash or "",
        min_bars=result.min_bars or 1,
        enabled=False,
        is_system=False,
    )
    ctx.db.add(def_)
    try:
        ctx.db.commit()
    except IntegrityError as exc:
        ctx.db.rollback()
        raise ValueError(f"因子 key {key} 已存在") from exc
    ctx.db.refresh(def_)
    return {"factor_draft": _factor_out(def_)}


__all__ = ["handle"]
