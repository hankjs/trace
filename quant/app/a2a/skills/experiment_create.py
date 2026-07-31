"""experiment.create skill。"""
from __future__ import annotations

from typing import Any

from ...api.strategies import get_strategy_or_404
from ...experiment.service import create_experiment, experiment_out
from ...strategy.runtime import strategy_spec_for
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """注册实验；A2A 契约要求必须提供 strategy_id。"""
    strategy_id = payload.get("strategy_id")
    if strategy_id is None:
        raise ValueError(
            "experiment.create 必须提供 strategy_id；请先调用 strategy.save_draft"
        )
    strategy = get_strategy_or_404(ctx.db, int(strategy_id), ctx.user_id)

    spec = payload.get("spec")
    if spec is None:
        spec = strategy_spec_for(strategy).model_dump(mode="json")

    try:
        exp = create_experiment(
            ctx.db,
            owner_id=ctx.user_id,
            title=payload["title"],
            hypothesis=payload["hypothesis"],
            permanent_candidate_id=payload["permanent_candidate_id"],
            spec=spec,
            strategy_id=strategy.id,
            family_id=payload.get("family_id"),
            universe_snapshot=payload.get("universe_snapshot"),
            cost_snapshot=payload.get("cost_snapshot"),
        )
    except KeyError as exc:
        raise ValueError(f"缺少必填字段: {exc}") from exc
    except ValueError as exc:
        raise ValueError(str(exc)) from exc

    return {"experiment": experiment_out(exp, trial_count=0)}


__all__ = ["handle"]
