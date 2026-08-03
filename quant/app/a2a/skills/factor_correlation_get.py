"""factor.correlation_get skill。"""
from __future__ import annotations

from typing import Any

from ...factors.correlation import CorrelationNotFoundError, get_correlation
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """按 correlation_id 取本人相关性检验详情。"""
    raw = payload.get("correlation_id")
    if raw is None:
        raise ValueError("payload 必须包含 correlation_id")
    try:
        correlation_id = int(raw)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"correlation_id 必须是整数，收到 {raw!r}") from exc

    try:
        detail = get_correlation(
            ctx.db, user_id=ctx.user_id, correlation_id=correlation_id,
        )
    except CorrelationNotFoundError as exc:
        raise ValueError(str(exc)) from exc
    return {"factor_correlation": detail}


__all__ = ["handle"]
