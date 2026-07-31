"""experiment.get skill。"""
from __future__ import annotations

from typing import Any

from ...experiment.promotion import list_promotions
from ...experiment.service import get_experiment, experiment_out, trial_out
from ...strategy.multiple_testing import multiplicity_report
from ._common import A2AContext


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """返回实验详情 + trials + multiplicity + pending promotions。"""
    experiment_id = payload.get("experiment_id")
    if experiment_id is None:
        raise ValueError("payload 必须包含 experiment_id")
    try:
        exp, trials = get_experiment(ctx.db, int(experiment_id), ctx.user_id)
    except LookupError as exc:
        raise ValueError(str(exc)) from exc

    trial_items = [trial_out(t) for t in trials]
    rows = []
    for item in trial_items:
        summary = item.get("metrics_summary") or {}
        if item["outcome"] in {"ok", "no_trades", "rejected"}:
            rows.append({
                "params": item.get("param_patch") or {},
                "metrics": {
                    "annual_return_median": summary.get("annual_return"),
                    "sharpe": summary.get("sharpe"),
                },
            })

    promotions = list_promotions(
        ctx.db, ctx.user_id, status=None, experiment_id=exp.id,
    )
    pending = [p for p in promotions if p["status"] == "pending"]

    return {
        "experiment": experiment_out(exp, trial_count=len(trial_items)),
        "trials": trial_items,
        "multiplicity": multiplicity_report(rows),
        "evidence_promotions": promotions,
        "pending_promotions": pending,
    }


__all__ = ["handle"]
