"""factor.evaluation_get skill。"""
from __future__ import annotations

from typing import Any

from ...factors.listing import EvaluationNotFoundError, get_evaluation
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """按 evaluation_id 取本人评估详情（含分层与 IC 衰减全量）。"""
    raw = payload.get("evaluation_id")
    if raw is None:
        raise ValueError("payload 必须包含 evaluation_id")
    try:
        evaluation_id = int(raw)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"evaluation_id 必须是整数，收到 {raw!r}") from exc

    try:
        detail = get_evaluation(
            ctx.db, user_id=ctx.user_id, evaluation_id=evaluation_id,
        )
    except EvaluationNotFoundError as exc:
        raise ValueError(str(exc)) from exc
    return {"factor_evaluation": detail}


__all__ = ["handle"]
