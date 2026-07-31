"""factor.evaluate skill。"""
from __future__ import annotations

from datetime import date
from typing import Any

from ...factors.evaluation import evaluate_factor_efficacy
from ...tasks import (
    TaskConflictError,
    register_handler,
    submit_task,
)
from ._common import A2AContext, wait_for_task


MAX_LAYERS = 10


def _build_evaluation_artifact(row) -> dict[str, Any]:
    result = row.result or {}
    ic = result.get("ic") or {}
    return {
        "factor_evaluation": {
            "evaluation_id": row.id,
            "factor_key": row.factor_key,
            "expression_hash": row.expression_hash,
            "universe": row.universe,
            "window": {
                "start": str(row.start),
                "end": str(row.end),
                "rebalance": row.rebalance,
            },
            "ic": {
                "ic_mean": ic.get("ic_mean"),
                "icir": ic.get("icir"),
                "rank_ic_mean": ic.get("rank_ic_mean"),
                "rank_icir": ic.get("rank_icir"),
                "positive_ratio": ic.get("positive_ratio"),
                "n_periods": ic.get("n_periods"),
            },
            "layers": result.get("layers", []),
            "long_short": result.get("long_short", {}),
            "coverage": result.get("coverage", {}),
            "detail_ref": {"evaluation_id": row.id},
            "status": row.status,
            "error": row.error,
        }
    }


def _factor_evaluation_handler(db, task, *, cancel_event=None):
    """quant_task handler：执行因子评估。"""
    p = task.params or {}
    expression = p.get("expression")
    factor_key = p.get("factor_key")
    codes = p.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]
    row = evaluate_factor_efficacy(
        db,
        user_id=task.user_id,
        expression=expression,
        factor_key=factor_key,
        start=date.fromisoformat(p["start"]),
        end=date.fromisoformat(p["end"]),
        pool_id=p.get("pool_id"),
        codes=codes,
        layers=int(p.get("layers", 10)),
        rebalance=p["rebalance"],
        cancel_event=cancel_event,
    )
    return _build_evaluation_artifact(row)


register_handler("factor_evaluation", _factor_evaluation_handler, supports_cancel=True)


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """提交 factor_evaluation 类型 quant_task 并等待完成。"""
    expression = payload.get("expression")
    factor_key = payload.get("factor_key")
    if (expression is None) == (factor_key is None):
        raise ValueError("必须且只能提供 expression 或 factor_key 之一")

    start = date.fromisoformat(str(payload["start"]))
    end = date.fromisoformat(str(payload["end"]))
    layers = max(1, min(int(payload.get("layers") or 10), MAX_LAYERS))
    rebalance = str(payload.get("rebalance") or "weekly")
    if rebalance not in {"weekly", "monthly"}:
        raise ValueError("rebalance 只支持 weekly 或 monthly")

    codes = payload.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="factor_evaluation",
            title=f"factor evaluate · {factor_key or 'expression'}",
            params={
                "expression": expression,
                "factor_key": factor_key,
                "start": start.isoformat(),
                "end": end.isoformat(),
                "pool_id": payload.get("pool_id"),
                "codes": codes,
                "layers": layers,
                "rebalance": rebalance,
                "client_request_id": payload.get("client_request_id"),
            },
        )
    except TaskConflictError as exc:
        raise ValueError(str(exc)) from exc

    wait_for_task(ctx.db, task, cancel_event)
    if task.status != "done":
        raise ValueError(task.error or "因子评估执行失败")
    return task.result or {}


__all__ = ["handle"]
