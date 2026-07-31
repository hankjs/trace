"""experiment.trial skill。"""
from __future__ import annotations

from datetime import date
from typing import Any

from ...experiment.service import create_trial_and_run, trial_out
from ...tasks import (
    TaskConflictError,
    register_handler,
    submit_task,
)
from ._common import A2AContext, wait_for_task


def _build_trial_result(trial, result: dict[str, Any] | None, promotion: dict[str, Any]) -> dict[str, Any]:
    out: dict[str, Any] = {
        "experiment_id": trial.experiment_id,
        "trial": trial_out(trial),
        "promotion": {
            "eligible": promotion.get("eligible", False),
            "suggested_target": promotion.get("suggested_target"),
            "todo": promotion.get("todo"),
        },
        "detail_ref": {
            "experiment_id": trial.experiment_id,
            "run_id": trial.backtest_run_id,
        },
    }
    if result is not None:
        out["backtest"] = {
            "run_id": result.get("run_id"),
            "metrics": result.get("metrics"),
            "validation": result.get("validation"),
            "data_quality": result.get("data_quality"),
        }
    return out


def _experiment_trial_handler(db, task, *, cancel_event=None):
    """quant_task handler：执行单条 trial。"""
    p = task.params or {}
    start = date.fromisoformat(p["start"])
    end = date.fromisoformat(p["end"])
    trial, result, promotion = create_trial_and_run(
        db,
        experiment_id=int(p["experiment_id"]),
        owner_id=task.user_id,
        codes=[str(c).lower() for c in p.get("codes", [])],
        start=start,
        end=end,
        param_patch=p.get("param_patch") or {},
        costs=p.get("costs") or None,
        pool_id=p.get("pool_id"),
        dynamic_universe=bool(p.get("dynamic_universe", False)),
    )
    return {"trial_result": _build_trial_result(trial, result, promotion)}


register_handler("experiment_trial", _experiment_trial_handler, supports_cancel=True)


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """提交 experiment_trial 类型 quant_task 并等待完成。"""
    experiment_id = int(payload["experiment_id"])
    start = date.fromisoformat(str(payload["start"]))
    end = date.fromisoformat(str(payload["end"]))
    if start >= end:
        raise ValueError("start 必须早于 end")

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="experiment_trial",
            title=f"trial · exp {experiment_id} · {start}~{end}",
            params={
                "experiment_id": experiment_id,
                "codes": list(dict.fromkeys(str(c).lower() for c in payload.get("codes", []))),
                "start": start.isoformat(),
                "end": end.isoformat(),
                "param_patch": payload.get("param_patch") or {},
                "costs": payload.get("costs") or None,
                "pool_id": payload.get("pool_id"),
                "dynamic_universe": bool(payload.get("dynamic_universe", False)),
                "client_request_id": payload.get("client_request_id"),
            },
        )
    except TaskConflictError as exc:
        raise ValueError(str(exc)) from exc

    # 等待完成（支持取消）
    wait_for_task(ctx.db, task, cancel_event)
    if task.status != "done":
        raise ValueError(task.error or "trial 执行失败")
    return task.result or {}


__all__ = ["handle"]
