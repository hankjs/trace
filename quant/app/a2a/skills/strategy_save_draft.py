"""strategy.save_draft skill。"""
from __future__ import annotations

from typing import Any

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError

from ...api.strategies import (
    _check_quota,
    _parse_definition,
    strategy_out,
)
from ...models import Strategy
from ...strategy.evidence import with_status
from ...strategy.spec import strategy_spec_hash
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """持久化 disabled unverified 策略草稿。"""
    name = payload.get("name", "").strip()
    if not name:
        raise ValueError("name 不能为空")
    spec = payload.get("spec")
    if spec is None:
        raise ValueError("payload 必须包含 spec")
    parent_strategy_id = payload.get("parent_strategy_id")

    parsed, capability = _parse_definition(spec=spec, db=ctx.db)
    parsed = with_status(parsed, "unverified")

    # parent 谱系校验：必须存在且对当前用户可见
    if parent_strategy_id is not None:
        parent = ctx.db.get(Strategy, int(parent_strategy_id))
        if parent is None or (
            parent.owner_id != ctx.user_id and not parent.is_system
        ):
            raise ValueError(
                f"parent_strategy_id {parent_strategy_id} 不存在或不可读"
            )

    _check_quota(ctx.db, ctx.user_id, adding=True, enabling=False)

    normalized = parsed.model_dump(mode="json")
    strategy = Strategy(
        owner_id=ctx.user_id,
        is_system=False,
        name=name,
        template="strategy_spec",
        kind=parsed.kind,
        params={
            "parent_strategy_id": int(parent_strategy_id),
        } if parent_strategy_id is not None else {},
        spec_schema_version=parsed.schema_version,
        spec=normalized,
        spec_hash=strategy_spec_hash(parsed),
        research_status="unverified",
        enabled=False,
    )
    ctx.db.add(strategy)
    try:
        ctx.db.commit()
    except IntegrityError as exc:
        ctx.db.rollback()
        raise ValueError(f"策略名「{name}」已存在，请改名后重试") from exc
    ctx.db.refresh(strategy)

    out = strategy_out(strategy, editable=True, usage=0, evidence_usage=0, db=ctx.db)
    return {
        "strategy_draft": {
            "strategy_id": strategy.id,
            "name": strategy.name,
            "spec_hash": out["spec_hash"],
            "parent_strategy_id": parent_strategy_id,
            "enabled": False,
            "research_status": "unverified",
            "capability": out["capability"],
        }
    }


__all__ = ["handle"]
