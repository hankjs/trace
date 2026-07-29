"""实验注册表领域逻辑。"""
from __future__ import annotations

import re
from copy import deepcopy
from datetime import datetime
from typing import Any

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from ..backtest.engine import DEFAULT_COSTS, run_backtest, _validate_costs
from ..models import Experiment, ExperimentTrial, Strategy
from ..strategy.evidence import spec_identity_hash
from ..strategy.spec import (
    CapabilityStatus,
    parse_strategy_spec,
    resolve_capabilities,
    strategy_spec_hash,
)
from .promotion import list_promotions, maybe_create_promotion_todo

EXPERIMENT_STATUSES = ("design", "running", "completed", "rejected", "archived")
TRIAL_OUTCOMES = ("ok", "no_trades", "error", "rejected")
_CANDIDATE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,62}[A-Za-z0-9]$")


def _now() -> datetime:
    return datetime.now()


def experiment_out(exp: Experiment, *, trial_count: int | None = None) -> dict[str, Any]:
    return {
        "id": exp.id,
        "owner_id": exp.owner_id,
        "permanent_candidate_id": exp.permanent_candidate_id,
        "family_id": exp.family_id,
        "title": exp.title,
        "hypothesis": exp.hypothesis,
        "strategy_id": exp.strategy_id,
        "frozen_spec_hash": exp.frozen_spec_hash,
        "identity_hash": exp.identity_hash,
        "validation_snapshot": deepcopy(exp.validation_snapshot),
        "universe_snapshot": deepcopy(exp.universe_snapshot),
        "cost_snapshot": deepcopy(exp.cost_snapshot),
        "status": exp.status,
        "trial_count": trial_count,
        "created_at": exp.created_at.isoformat(sep=" ") if exp.created_at else None,
        "updated_at": exp.updated_at.isoformat(sep=" ") if exp.updated_at else None,
        "frozen_spec_snapshot": deepcopy(exp.frozen_spec_snapshot),
    }


def trial_out(trial: ExperimentTrial) -> dict[str, Any]:
    return {
        "id": trial.id,
        "experiment_id": trial.experiment_id,
        "trial_index": trial.trial_index,
        "param_patch": deepcopy(trial.param_patch) or {},
        "backtest_run_id": trial.backtest_run_id,
        "outcome": trial.outcome,
        "metrics_summary": deepcopy(trial.metrics_summary),
        "error": trial.error,
        "data_fingerprint": trial.data_fingerprint,
        "universe_fingerprint": trial.universe_fingerprint,
        "cost_fingerprint": trial.cost_fingerprint,
        "execution_fingerprint": trial.execution_fingerprint,
        "oos_revealed_at": (
            trial.oos_revealed_at.isoformat(sep=" ")
            if trial.oos_revealed_at else None
        ),
        "created_at": trial.created_at.isoformat(sep=" ") if trial.created_at else None,
    }


def _next_trial_index(db: Session, experiment_id: int) -> int:
    current = db.execute(
        select(func.coalesce(func.max(ExperimentTrial.trial_index), 0)).where(
            ExperimentTrial.experiment_id == experiment_id,
        )
    ).scalar() or 0
    return int(current) + 1


def create_experiment(
    db: Session,
    *,
    owner_id: str,
    title: str,
    hypothesis: str,
    permanent_candidate_id: str,
    spec: dict[str, Any],
    strategy_id: int | None = None,
    family_id: str | None = None,
    universe_snapshot: dict | None = None,
    cost_snapshot: dict | None = None,
) -> Experiment:
    cid = permanent_candidate_id.strip()
    if not _CANDIDATE_RE.match(cid):
        raise ValueError(
            "permanent_candidate_id 须为 2~64 位字母数字(可含 ._: -)",
        )
    if not hypothesis.strip():
        raise ValueError("hypothesis 不能为空")
    parsed = parse_strategy_spec(spec)
    capability = resolve_capabilities(parsed)
    if capability.status != CapabilityStatus.SUPPORTED:
        raise ValueError(
            f"规格能力不足({capability.status}): "
            + "; ".join(i.message for i in capability.issues[:5])
        )
    if strategy_id is not None:
        strategy = db.get(Strategy, strategy_id)
        if strategy is None:
            raise ValueError(f"策略 {strategy_id} 不存在")
    exists = db.execute(
        select(Experiment.id).where(
            Experiment.owner_id == owner_id,
            Experiment.permanent_candidate_id == cid,
        )
    ).scalar_one_or_none()
    if exists is not None:
        raise ValueError(f"候选 ID「{cid}」已存在")

    now = _now()
    exp = Experiment(
        owner_id=owner_id,
        permanent_candidate_id=cid,
        family_id=family_id,
        title=title.strip()[:128],
        hypothesis=hypothesis.strip(),
        strategy_id=strategy_id,
        frozen_spec_snapshot=parsed.model_dump(mode="json"),
        frozen_spec_hash=strategy_spec_hash(parsed),
        identity_hash=spec_identity_hash(parsed),
        validation_snapshot=parsed.validation.model_dump(mode="json"),
        universe_snapshot=universe_snapshot or {
            "pool_id": parsed.universe.pool_id,
        },
        cost_snapshot=cost_snapshot or dict(DEFAULT_COSTS),
        status="design",
        created_at=now,
        updated_at=now,
    )
    db.add(exp)
    db.commit()
    db.refresh(exp)
    return exp


def list_experiments(
    db: Session, owner_id: str, *, include_archived: bool = False,
) -> list[dict[str, Any]]:
    q = select(Experiment).where(Experiment.owner_id == owner_id)
    if not include_archived:
        q = q.where(Experiment.status != "archived")
    rows = list(db.execute(q.order_by(Experiment.id.desc())).scalars().all())
    if not rows:
        return []
    counts = dict(
        db.execute(
            select(
                ExperimentTrial.experiment_id,
                func.count(ExperimentTrial.id),
            ).where(
                ExperimentTrial.experiment_id.in_([r.id for r in rows]),
            ).group_by(ExperimentTrial.experiment_id)
        ).all()
    )
    return [
        experiment_out(row, trial_count=int(counts.get(row.id, 0)))
        for row in rows
    ]


def get_experiment(
    db: Session, experiment_id: int, owner_id: str,
) -> tuple[Experiment, list[ExperimentTrial]]:
    exp = db.execute(
        select(Experiment).where(
            Experiment.id == experiment_id,
            Experiment.owner_id == owner_id,
        )
    ).scalar_one_or_none()
    if exp is None:
        raise LookupError(f"实验 {experiment_id} 不存在")
    trials = list(db.execute(
        select(ExperimentTrial).where(
            ExperimentTrial.experiment_id == experiment_id,
        ).order_by(ExperimentTrial.trial_index)
    ).scalars().all())
    return exp, trials


def archive_experiment(db: Session, experiment_id: int, owner_id: str) -> Experiment:
    exp, _ = get_experiment(db, experiment_id, owner_id)
    if exp.status == "archived":
        return exp
    exp.status = "archived"
    exp.updated_at = _now()
    db.commit()
    db.refresh(exp)
    return exp


def _apply_param_patch(spec_dict: dict, patch: dict[str, Any]) -> dict:
    """按 $.a.b.c 路径覆盖规格;用于试验变体。"""
    raw = deepcopy(spec_dict)
    for path, value in (patch or {}).items():
        if not isinstance(path, str) or not path.startswith("$."):
            raise ValueError(f"非法参数路径: {path}")
        parts = path[2:].split(".")
        target: Any = raw
        for part in parts[:-1]:
            if not isinstance(target, dict) or part not in target:
                raise ValueError(f"路径不存在: {path}")
            target = target[part]
        if not isinstance(target, dict) or parts[-1] not in target:
            raise ValueError(f"路径不存在: {path}")
        target[parts[-1]] = value
    return raw


def _outcome_from_result(result: dict[str, Any]) -> str:
    metrics = result.get("metrics") or {}
    rejection = (result.get("validation") or {}).get("rejection") or {}
    if rejection.get("verdict") == "rejected":
        return "rejected"
    trades = metrics.get("trade_count") or metrics.get("round_trips") or 0
    if not trades:
        return "no_trades"
    return "ok"


def create_trial_and_run(
    db: Session,
    *,
    experiment_id: int,
    owner_id: str,
    codes: list[str],
    start,
    end,
    param_patch: dict | None = None,
    costs: dict | None = None,
    pool_id: int | None = None,
    dynamic_universe: bool = False,
    strategy: Strategy | None = None,
) -> tuple[ExperimentTrial, dict[str, Any] | None, dict[str, Any]]:
    """创建 trial 并同步执行回测(写入 run + 回填 trial)。

    失败也会落 trial(outcome=error),保证账本完整。
    第三项为证据推进评估(达标则含 pending todo;试验从不自动改 evidence_status)。
    """
    empty_promotion: dict[str, Any] = {
        "eligible": False,
        "suggested_target": None,
        "checks": [],
        "block_reasons": [],
        "todo": None,
    }
    exp, _ = get_experiment(db, experiment_id, owner_id)
    if exp.status == "archived":
        raise ValueError("已归档实验不能新增试验")
    if exp.status == "design":
        exp.status = "running"
        exp.updated_at = _now()

    patch = param_patch or {}
    try:
        patched = _apply_param_patch(exp.frozen_spec_snapshot, patch)
        parsed = parse_strategy_spec(patched)
    except Exception as exc:  # noqa: BLE001
        trial = ExperimentTrial(
            experiment_id=exp.id,
            trial_index=_next_trial_index(db, exp.id),
            param_patch=patch,
            outcome="error",
            error=str(exc)[:2000],
            created_at=_now(),
        )
        db.add(trial)
        db.commit()
        db.refresh(trial)
        return trial, None, {
            **empty_promotion,
            "block_reasons": [str(exc)[:200]],
        }

    try:
        costs_eff = _validate_costs(costs or exp.cost_snapshot or DEFAULT_COSTS)
    except ValueError as exc:
        trial = ExperimentTrial(
            experiment_id=exp.id,
            trial_index=_next_trial_index(db, exp.id),
            param_patch=patch,
            outcome="error",
            error=str(exc)[:2000],
            created_at=_now(),
        )
        db.add(trial)
        db.commit()
        db.refresh(trial)
        return trial, None, {
            **empty_promotion,
            "block_reasons": [str(exc)[:200]],
        }

    # 落库回测需要合法 strategy_id(FK RESTRICT);试验必须挂到真实策略行
    if strategy is None and exp.strategy_id is not None:
        strategy = db.get(Strategy, exp.strategy_id)
    if strategy is None:
        raise ValueError(
            "实验未关联策略 strategy_id,无法落库 trial 回测;"
            "请创建实验时传入 strategy_id,或从策略「另存为」后绑定",
        )

    trial = ExperimentTrial(
        experiment_id=exp.id,
        trial_index=_next_trial_index(db, exp.id),
        param_patch=patch,
        outcome="error",
        created_at=_now(),
    )
    db.add(trial)
    db.flush()

    result: dict[str, Any] | None = None
    try:
        result = run_backtest(
            db, strategy, list(codes), start, end,
            params={}, costs=costs_eff,
            save=True,
            dynamic_universe=dynamic_universe,
            user_id=owner_id,
            pool_id=pool_id,
            execution_spec=parsed,
        )
        trial.backtest_run_id = result.get("run_id")
        trial.outcome = _outcome_from_result(result)
        metrics = result.get("metrics") or {}
        trial.metrics_summary = {
            k: metrics.get(k)
            for k in (
                "total_return", "annual_return", "max_drawdown",
                "sharpe", "win_rate", "trade_count", "round_trips",
            )
        }
        trial.data_fingerprint = result.get("data_fingerprint")
        trial.universe_fingerprint = result.get("universe_fingerprint")
        trial.cost_fingerprint = result.get("cost_fingerprint")
        trial.execution_fingerprint = result.get("execution_fingerprint")
        if (result.get("validation") or {}).get("rejection", {}).get(
            "verdict",
        ) == "rejected":
            exp.status = "rejected"
        elif exp.status in {"design", "running"}:
            exp.status = "completed"
        # 试验永不自动推进 evidence_status;达标仅生成用户待办
        promotion = maybe_create_promotion_todo(
            db,
            owner_id=owner_id,
            strategy=strategy,
            experiment=exp,
            trial=trial,
            result=result,
        )
    except Exception as exc:  # noqa: BLE001
        trial.outcome = "error"
        trial.error = str(exc)[:2000]
        exp.status = "running"
        promotion = {
            "eligible": False,
            "suggested_target": None,
            "checks": [],
            "block_reasons": [str(exc)[:200]],
            "todo": None,
        }
    exp.updated_at = _now()
    db.commit()
    db.refresh(trial)
    return trial, result, promotion


def get_experiment_detail(
    db: Session, experiment_id: int, owner_id: str,
) -> dict[str, Any]:
    """实验详情 + trials + 该实验下的证据推进待办。"""
    exp, trials = get_experiment(db, experiment_id, owner_id)
    promotions = list_promotions(
        db, owner_id, status=None, experiment_id=experiment_id,
    )
    pending = [p for p in promotions if p["status"] == "pending"]
    return {
        **experiment_out(exp, trial_count=len(trials)),
        "trials": [trial_out(t) for t in trials],
        "evidence_promotions": promotions,
        "pending_promotions": pending,
    }


__all__ = [
    "archive_experiment",
    "create_experiment",
    "create_trial_and_run",
    "experiment_out",
    "get_experiment",
    "get_experiment_detail",
    "list_experiments",
    "trial_out",
]
