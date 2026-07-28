"""实验注册表 API:创建实验、列出 trial、发起试验回测、归档。"""
from __future__ import annotations

from datetime import date
from typing import Any

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..experiment.service import (
    archive_experiment,
    create_experiment,
    create_trial_and_run,
    experiment_out,
    get_experiment,
    list_experiments,
    trial_out,
)
from ..strategy.multiple_testing import multiplicity_report

router = APIRouter(prefix="/api/experiments", tags=["experiments"])


class ExperimentCreateIn(BaseModel):
    title: str = Field(..., min_length=1, max_length=128)
    hypothesis: str = Field(..., min_length=1)
    permanent_candidate_id: str = Field(..., min_length=2, max_length=64)
    spec: dict
    strategy_id: int | None = None
    family_id: str | None = Field(None, max_length=64)
    universe_snapshot: dict | None = None
    cost_snapshot: dict | None = None


class TrialCreateIn(BaseModel):
    codes: list[str] = Field(..., min_length=1, max_length=800)
    start: date
    end: date
    param_patch: dict = Field(default_factory=dict)
    costs: dict = Field(default_factory=dict)
    pool_id: int | None = None
    dynamic_universe: bool = False


@router.get("")
def api_list_experiments(
    include_archived: bool = False,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    owner_id = user_id_from_claims(claims)
    items = list_experiments(
        db, owner_id, include_archived=include_archived,
    )
    return {"count": len(items), "items": items}


@router.post("")
def api_create_experiment(
    body: ExperimentCreateIn,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    owner_id = user_id_from_claims(claims)
    try:
        exp = create_experiment(
            db,
            owner_id=owner_id,
            title=body.title,
            hypothesis=body.hypothesis,
            permanent_candidate_id=body.permanent_candidate_id,
            spec=body.spec,
            strategy_id=body.strategy_id,
            family_id=body.family_id,
            universe_snapshot=body.universe_snapshot,
            cost_snapshot=body.cost_snapshot,
        )
    except ValueError as exc:
        raise HTTPException(400, str(exc))
    return experiment_out(exp, trial_count=0)


@router.get("/{experiment_id}")
def api_get_experiment(
    experiment_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    owner_id = user_id_from_claims(claims)
    try:
        exp, trials = get_experiment(db, experiment_id, owner_id)
    except LookupError as exc:
        raise HTTPException(404, str(exc))
    trial_items = [trial_out(t) for t in trials]
    # 多重检验提示:基于已成功 trial 的年化
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
    return {
        **experiment_out(exp, trial_count=len(trial_items)),
        "trials": trial_items,
        "multiplicity": multiplicity_report(rows),
    }


@router.post("/{experiment_id}/trials")
def api_create_trial(
    experiment_id: int,
    body: TrialCreateIn,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    owner_id = user_id_from_claims(claims)
    codes = list(dict.fromkeys(c.lower() for c in body.codes))
    try:
        trial, result = create_trial_and_run(
            db,
            experiment_id=experiment_id,
            owner_id=owner_id,
            codes=codes,
            start=body.start,
            end=body.end,
            param_patch=body.param_patch,
            costs=body.costs or None,
            pool_id=body.pool_id,
            dynamic_universe=body.dynamic_universe,
        )
    except LookupError as exc:
        raise HTTPException(404, str(exc))
    except ValueError as exc:
        raise HTTPException(400, str(exc))
    payload: dict[str, Any] = {"trial": trial_out(trial)}
    if result is not None:
        payload["backtest"] = {
            "run_id": result.get("run_id"),
            "metrics": result.get("metrics"),
            "validation": result.get("validation"),
            "data_quality": result.get("data_quality"),
            "execution_attribution": (
                (result.get("metrics") or {}).get("execution_attribution")
            ),
            "execution_fingerprint": result.get("execution_fingerprint"),
        }
    return payload


@router.post("/{experiment_id}/archive")
def api_archive_experiment(
    experiment_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    owner_id = user_id_from_claims(claims)
    try:
        exp = archive_experiment(db, experiment_id, owner_id)
    except LookupError as exc:
        raise HTTPException(404, str(exc))
    return experiment_out(exp)


@router.delete("/{experiment_id}/trials/{trial_id}")
def api_delete_trial_denied(
    experiment_id: int,
    trial_id: int,
    claims: dict = Depends(require_client),
):
    """失败与成功 trial 均不可删除,保留完整搜索账本。"""
    _ = (experiment_id, trial_id, claims)
    raise HTTPException(405, "试验记录不可删除,仅可归档整个实验")
