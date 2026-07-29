"""试验 → 证据推进：质量闸门 + 用户待办。

原则（与产品约定一致）:
- 试验回测**从不**自动调用 ``advance_after_backtest``。
- 质量未达标：**不**生成待办，系统直接挡住（trial 上仅返回 checks 说明）。
- 质量达标且身份匹配：落一条 ``pending`` 待办，由用户「采纳为证据」或「忽略」。
- 采纳时再走 ``advance_after_backtest``，与回测页同一状态机。

质量闸门是硬门槛（研究卫生），不是「策略很强才给推」——通过 ≠ 可交易。
"""
from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import BacktestRun, EvidencePromotion, Experiment, ExperimentTrial, Strategy
from ..strategy.evidence import (
    STATUS_RANK,
    advance_after_backtest,
    candidate_spec_hashes,
)
from ..strategy.runtime import strategy_spec_for

# 硬门槛：过少交易视为样本不足，不生成推进待办
MIN_TRADE_COUNT = 3
MIN_ROUND_TRIPS = 1

PROMOTION_STATUSES = ("pending", "accepted", "dismissed", "superseded")
SUGGESTED_TARGETS = ("backtested", "oos_passed", "rejected")


def _check(check_id: str, ok: bool, message: str) -> dict[str, Any]:
    return {
        "id": check_id,
        "ok": ok,
        "message": message,
    }


def _now() -> datetime:
    return datetime.now()


def evaluate_promotion_quality(
    *,
    strategy: Strategy | None,
    trial_outcome: str,
    result: dict[str, Any] | None,
    param_patch: dict | None = None,
) -> dict[str, Any]:
    """评估一次 trial 是否有资格成为「推进证据」待办。

    返回::
        {
          "eligible": bool,
          "suggested_target": str | None,  # backtested|oos_passed|rejected
          "checks": [{id, ok, message}, ...],
          "block_reasons": [str, ...],  # 未达标项 message
        }
    """
    checks: list[dict[str, Any]] = []
    patch = param_patch or {}

    if strategy is None:
        checks.append(_check("STRATEGY", False, "试验未关联策略，无法写入证据"))
        return _pack(checks, None)

    try:
        spec = strategy_spec_for(strategy)
        current = spec.metadata.evidence_status
    except Exception:  # noqa: BLE001
        checks.append(_check("STRATEGY_SPEC", False, "策略规格不可解析"))
        return _pack(checks, None)

    checks.append(_check(
        "EVIDENCE_BASE",
        current != "unverified",
        (
            "策略仍为「未验证」，请先标记验证设计完成"
            if current == "unverified"
            else f"当前证据状态: {current}"
        ),
    ))
    checks.append(_check(
        "NOT_REJECTED",
        current != "rejected",
        (
            "策略已否决，请先复位后再考虑推进"
            if current == "rejected"
            else "策略未处于终态否决"
        ),
    ))

    if result is None:
        checks.append(_check("RUN", False, "无回测结果"))
        return _pack(checks, None)

    run_id = result.get("run_id")
    checks.append(_check(
        "RUN_ID",
        bool(run_id),
        "必须有落库 run_id 才能作为证据" if not run_id else f"run_id={run_id}",
    ))

    run_hash = result.get("strategy_spec_hash")
    identity_ok = bool(run_hash) and run_hash in candidate_spec_hashes(spec)
    # 空 patch 的基准 trial 才是「对当前规格」的证据；改参变体默认不当作身份证据
    is_baseline = not bool(patch)
    checks.append(_check(
        "IDENTITY",
        identity_ok and is_baseline,
        (
            "仅空 param_patch 的基准试验可推进当前规格证据；改参变体请另存规格后再验证"
            if not is_baseline
            else (
                "回测规格与当前策略身份不一致（临时参数或旧快照）"
                if not identity_ok
                else "规格身份匹配"
            )
        ),
    ))

    outcome = trial_outcome or "error"
    validation = result.get("validation") or {}
    rejection = validation.get("rejection") or {}
    oos = validation.get("oos") or {}
    verdict = rejection.get("verdict")

    metrics = result.get("metrics") or {}
    trade_count = metrics.get("trade_count")
    round_trips = metrics.get("round_trips")
    try:
        trade_n = int(trade_count) if trade_count is not None else 0
    except (TypeError, ValueError):
        trade_n = 0
    try:
        rt_n = int(round_trips) if round_trips is not None else 0
    except (TypeError, ValueError):
        rt_n = 0

    # 否决结论：即使交易少，也允许生成「确认写入否决」待办（仍需用户点）
    is_reject_path = verdict == "rejected"
    if is_reject_path:
        checks.append(_check(
            "OUTCOME",
            True,
            "validation 命中否决，可生成「确认写入否决」待办",
        ))
        suggested = "rejected"
    else:
        outcome_ok = outcome == "ok"
        checks.append(_check(
            "OUTCOME",
            outcome_ok,
            (
                f"试验结果为 {outcome}，质量不足，不生成推进待办"
                if not outcome_ok
                else "试验完成(ok)"
            ),
        ))
        sample_ok = trade_n >= MIN_TRADE_COUNT or rt_n >= MIN_ROUND_TRIPS
        checks.append(_check(
            "SAMPLE_SIZE",
            sample_ok,
            (
                f"交易样本不足(trade_count={trade_n}, round_trips={rt_n};"
                f" 至少 trade≥{MIN_TRADE_COUNT} 或 round_trips≥{MIN_ROUND_TRIPS})"
                if not sample_ok
                else f"样本量可接受(trades={trade_n}, round_trips={rt_n})"
            ),
        ))
        if verdict == "passed" and oos.get("available"):
            suggested = "oos_passed"
        else:
            suggested = "backtested"

    # 数据信任：有报告且明确失败时硬拦
    dq = result.get("data_quality")
    if isinstance(dq, dict) and dq.get("ok") is False:
        checks.append(_check(
            "DATA_QUALITY",
            False,
            "回测数据质量未通过，系统拦截推进待办",
        ))
    else:
        checks.append(_check(
            "DATA_QUALITY",
            True,
            "数据质量未报失败" if not isinstance(dq, dict) else "数据质量检查通过",
        ))

    # 若建议目标不能前进（已 oos_passed 再 backtested），不生成待办
    if suggested != "rejected" and STATUS_RANK.get(suggested, 0) <= STATUS_RANK.get(current, 0):
        checks.append(_check(
            "FORWARD",
            False,
            f"建议目标 {suggested} 不高于当前状态 {current}，无需推进",
        ))
    else:
        checks.append(_check(
            "FORWARD",
            True,
            f"建议推进至 {suggested}",
        ))

    return _pack(checks, suggested)


def _pack(checks: list[dict[str, Any]], suggested: str | None) -> dict[str, Any]:
    failed = [c for c in checks if not c["ok"]]
    return {
        "eligible": len(failed) == 0 and suggested is not None,
        "suggested_target": suggested if not failed else None,
        "checks": checks,
        "block_reasons": [c["message"] for c in failed],
    }


def promotion_out(row: EvidencePromotion) -> dict[str, Any]:
    return {
        "id": row.id,
        "owner_id": row.owner_id,
        "strategy_id": row.strategy_id,
        "experiment_id": row.experiment_id,
        "trial_id": row.trial_id,
        "backtest_run_id": row.backtest_run_id,
        "status": row.status,
        "suggested_target": row.suggested_target,
        "quality_checks": list(row.quality_checks or []),
        "metrics_summary": dict(row.metrics_summary or {}),
        "created_at": row.created_at.isoformat(sep=" ") if row.created_at else None,
        "resolved_at": (
            row.resolved_at.isoformat(sep=" ") if row.resolved_at else None
        ),
    }


def maybe_create_promotion_todo(
    db: Session,
    *,
    owner_id: str,
    strategy: Strategy | None,
    experiment: Experiment,
    trial: ExperimentTrial,
    result: dict[str, Any] | None,
) -> dict[str, Any]:
    """试验落库后调用：达标则写 pending 待办，否则只返回评估结果。

    不调用 advance_after_backtest。
    """
    evaluation = evaluate_promotion_quality(
        strategy=strategy,
        trial_outcome=trial.outcome,
        result=result,
        param_patch=trial.param_patch,
    )
    payload: dict[str, Any] = {
        "eligible": evaluation["eligible"],
        "suggested_target": evaluation["suggested_target"],
        "checks": evaluation["checks"],
        "block_reasons": evaluation["block_reasons"],
        "todo": None,
    }
    if not evaluation["eligible"] or strategy is None or result is None:
        return payload

    run_id = result.get("run_id")
    if not run_id:
        return payload

    existing = db.execute(
        select(EvidencePromotion).where(EvidencePromotion.trial_id == trial.id),
    ).scalar_one_or_none()
    if existing is not None:
        if existing.status == "pending":
            existing.suggested_target = evaluation["suggested_target"]
            existing.quality_checks = evaluation["checks"]
            existing.metrics_summary = dict(trial.metrics_summary or {})
            existing.backtest_run_id = int(run_id)
            db.flush()
            payload["todo"] = promotion_out(existing)
            return payload
        # 已处理过的 trial 不再新建
        payload["todo"] = promotion_out(existing)
        payload["eligible"] = False
        payload["block_reasons"] = [
            f"该试验已有推进记录(status={existing.status})",
        ]
        return payload

    # 同策略仅保留一条 pending：新的达标基准覆盖旧 pending（旧标 superseded）
    old_pending = db.execute(
        select(EvidencePromotion).where(
            EvidencePromotion.strategy_id == strategy.id,
            EvidencePromotion.owner_id == owner_id,
            EvidencePromotion.status == "pending",
        ),
    ).scalars().all()
    for row in old_pending:
        row.status = "superseded"
        row.resolved_at = _now()

    todo = EvidencePromotion(
        owner_id=owner_id,
        strategy_id=strategy.id,
        experiment_id=experiment.id,
        trial_id=trial.id,
        backtest_run_id=int(run_id),
        status="pending",
        suggested_target=evaluation["suggested_target"] or "backtested",
        quality_checks=evaluation["checks"],
        metrics_summary=dict(trial.metrics_summary or {}),
        created_at=_now(),
    )
    db.add(todo)
    db.flush()
    payload["todo"] = promotion_out(todo)
    return payload


def list_promotions(
    db: Session,
    owner_id: str,
    *,
    status: str | None = "pending",
    strategy_id: int | None = None,
    experiment_id: int | None = None,
) -> list[dict[str, Any]]:
    q = select(EvidencePromotion).where(EvidencePromotion.owner_id == owner_id)
    if status:
        q = q.where(EvidencePromotion.status == status)
    if strategy_id is not None:
        q = q.where(EvidencePromotion.strategy_id == strategy_id)
    if experiment_id is not None:
        q = q.where(EvidencePromotion.experiment_id == experiment_id)
    q = q.order_by(EvidencePromotion.id.desc())
    rows = db.execute(q).scalars().all()
    return [promotion_out(r) for r in rows]


def _load_owned_todo(
    db: Session, todo_id: int, owner_id: str,
) -> EvidencePromotion:
    row = db.get(EvidencePromotion, todo_id)
    if row is None or row.owner_id != owner_id:
        raise LookupError("推进待办不存在")
    return row


def dismiss_promotion(
    db: Session, todo_id: int, owner_id: str,
) -> dict[str, Any]:
    row = _load_owned_todo(db, todo_id, owner_id)
    if row.status != "pending":
        raise ValueError(f"仅 pending 可忽略，当前为 {row.status}")
    row.status = "dismissed"
    row.resolved_at = _now()
    db.flush()
    return promotion_out(row)


def accept_promotion(
    db: Session, todo_id: int, owner_id: str,
) -> dict[str, Any]:
    """用户确认：复检质量闸门后调用 advance_after_backtest。"""
    row = _load_owned_todo(db, todo_id, owner_id)
    if row.status != "pending":
        raise ValueError(f"仅 pending 可采纳，当前为 {row.status}")

    strategy = db.get(Strategy, row.strategy_id)
    if strategy is None:
        raise ValueError("关联策略已不存在")
    trial = db.get(ExperimentTrial, row.trial_id)
    if trial is None:
        raise ValueError("关联试验已不存在")

    run = db.get(BacktestRun, row.backtest_run_id)
    if run is None:
        raise ValueError("关联回测 run 不存在")

    # 从 run 重建 advance 所需的 result 形态
    result: dict[str, Any] = {
        "run_id": run.id,
        "strategy_spec_hash": run.strategy_spec_hash,
        "metrics": run.metrics if isinstance(run.metrics, dict) else {},
        "validation": (
            (run.metrics or {}).get("validation")
            if isinstance(run.metrics, dict)
            else None
        ) or {},
        "data_quality": (
            (run.metrics or {}).get("data_quality")
            if isinstance(run.metrics, dict)
            else None
        ),
    }
    # metrics 顶层也可能直接是指标
    if isinstance(run.metrics, dict):
        for k in (
            "total_return", "annual_return", "max_drawdown",
            "sharpe", "win_rate", "trade_count", "round_trips",
        ):
            if k in run.metrics and k not in result["metrics"]:
                result["metrics"][k] = run.metrics[k]

    evaluation = evaluate_promotion_quality(
        strategy=strategy,
        trial_outcome=trial.outcome,
        result=result,
        param_patch=trial.param_patch,
    )
    if not evaluation["eligible"]:
        raise ValueError(
            "质量闸门未通过，无法采纳: "
            + "; ".join(evaluation["block_reasons"][:5]),
        )

    transition = advance_after_backtest(db, strategy, result)
    row.status = "accepted"
    row.resolved_at = _now()
    db.flush()
    return {
        "todo": promotion_out(row),
        "evidence_transition": transition,
        "evaluation": evaluation,
    }


__all__ = [
    "MIN_ROUND_TRIPS",
    "MIN_TRADE_COUNT",
    "accept_promotion",
    "dismiss_promotion",
    "evaluate_promotion_quality",
    "list_promotions",
    "maybe_create_promotion_todo",
    "promotion_out",
]
