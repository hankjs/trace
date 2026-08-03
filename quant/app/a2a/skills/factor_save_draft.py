"""factor.save_draft skill。"""
from __future__ import annotations

from typing import Any

from sqlalchemy import select
from sqlalchemy.exc import IntegrityError

from ...catalog import FILTER_FIELDS
from ...factors.defs import invalidate_factor_cache
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
        "owner_id": def_.owner_id,
        "parent_factor_key": def_.parent_factor_key,
        "created_at": def_.created_at.isoformat(sep=" ") if def_.created_at else None,
    }


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """保存 disabled 因子草稿,归属调用者。"""
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

    # parent 谱系校验:必须存在且对当前用户可读(系统因子或自己的)
    parent_factor_key = payload.get("parent_factor_key")
    if parent_factor_key is not None:
        parent_key = str(parent_factor_key)
        parent = ctx.db.execute(
            select(FactorDef).where(FactorDef.key == parent_key)
        ).scalar_one_or_none()
        if parent is None or (
            parent.owner_id != ctx.user_id and not parent.is_system
        ):
            raise ValueError(
                f"parent_factor_key {parent_key} 不存在或不可读"
            )
        parent_factor_key = parent_key
    else:
        parent_factor_key = None

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
        owner_id=ctx.user_id,
        parent_factor_key=parent_factor_key,
    )
    ctx.db.add(def_)
    try:
        ctx.db.commit()
    except IntegrityError as exc:
        ctx.db.rollback()
        raise ValueError(f"因子 key {key} 已存在") from exc
    ctx.db.refresh(def_)
    # 与 REST create/patch/delete 一致:立即失效进程缓存,否则 60s 内
    # load_all_defs 看不到新草稿,紧接着的 backfill 会找不到它。
    invalidate_factor_cache()
    return {"factor_draft": _factor_out(def_)}


__all__ = ["handle"]
