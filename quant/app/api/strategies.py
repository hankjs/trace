"""数据库 StrategySpec 的校验、CRUD 与可见性接口。

`quant_strategy.spec` 是当前完整定义的唯一事实来源。用户编辑时原地更新；
`kind`、规范化哈希和能力状态全部由服务端计算，不接受客户端伪造。
"""
from __future__ import annotations

from copy import deepcopy
from typing import Literal

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..backtest.engine import validate_params as validate_legacy_params
from ..catalog import STRATEGY_TEMPLATES, template_name
from ..data.ingest import BAR_FIELDS, snapshot_available_fields
from ..db import get_db
from ..models import BacktestRun, Strategy
from ..research_plan.domain import CAPABILITIES
from ..strategy.runtime import strategy_spec_for
from ..strategy.evidence import (
    DesignCompleteChecklistError,
    apply_manual_action,
    candidate_spec_hashes,
    design_complete_checks,
    manual_actions_for,
    resolve_status_on_edit,
    with_status,
)
from ..strategy.spec import (
    CapabilityReport,
    CapabilityStatus,
    StrategySpec,
    parse_strategy_spec,
    resolve_capabilities,
    strategy_spec_hash,
)
from ..strategy.store import (
    MAX_ENABLED_PER_USER,
    MAX_STRATEGIES_PER_USER,
    can_edit,
    count_owned,
    list_visible,
    visible_to,
)
from ..strategy.strategies import REGISTRY

router = APIRouter(prefix="/api/strategies", tags=["strategies"])

ResearchStatus = Literal["unverified", "verified", "rejected"]


class StrategyCreateIn(BaseModel):
    name: str = Field(..., min_length=1, max_length=64)
    # 完整 StrategySpec 是唯一创建路径;系统预设请用「另存为/复制」。
    spec: dict
    enabled: bool = True
    research_status: ResearchStatus = "unverified"


class StrategyPatchIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    spec: dict | None = None
    enabled: bool | None = None
    research_status: ResearchStatus | None = None


class StrategyDuplicateIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    spec: dict | None = None


class StrategyValidateIn(BaseModel):
    spec: dict
    # 可选:附带 design_complete 硬清单结果,不改变普通 valid 判定
    check_design_gate: bool = False


class StrategyEvidenceIn(BaseModel):
    """证据状态的手动操作;自动推进的状态(backtested/oos_passed)不允许手改。"""
    action: Literal["mark_design_complete", "reset_rejected"]


def _legacy_spec(template: str, params: dict | None = None) -> StrategySpec:
    if template not in REGISTRY or template not in STRATEGY_TEMPLATES:
        raise HTTPException(
            400, f"未知算法模板 {template}，可选: {', '.join(sorted(REGISTRY))}",
        )
    from ..strategy.presets import get_preset_spec

    try:
        effective = validate_legacy_params(template, params)
        return get_preset_spec(template, effective)
    except ValueError as exc:
        raise HTTPException(400, str(exc))


def _available_fields(db: Session | None) -> frozenset[str] | None:
    """有库连接时把「库里实际有数据的字段」纳入 available_fields。

    日线列恒可用;估值/财务字段按快照表是否已有非空数据判定,让
    missing_data 报告在保存/启用时就能如实提示,而不是回测时才暴露。
    """
    if db is None:
        return None
    return BAR_FIELDS | snapshot_available_fields(db)


def _parse_definition(*, spec: dict | None, template: str | None = None,
                      params: dict | None = None,
                      db: Session | None = None) -> tuple[StrategySpec, CapabilityReport]:
    raw = spec if spec is not None else _legacy_spec(template or "", params)
    capability = resolve_capabilities(raw, available_fields=_available_fields(db))
    try:
        parsed = parse_strategy_spec(raw)
    except Exception as exc:  # Pydantic 已给出精确路径，API 保留能力报告
        raise HTTPException(
            400,
            {
                "message": "StrategySpec 校验失败",
                "capability": capability.model_dump(mode="json"),
                "error": str(exc),
            },
        )
    return parsed, capability


def _require_supported(capability: CapabilityReport) -> None:
    if capability.status == CapabilityStatus.SUPPORTED:
        return
    raise HTTPException(
        400,
        {
            "message": "该策略当前不能启用或回测",
            "capability": capability.model_dump(mode="json"),
        },
    )


def validation_out(
    raw: object,
    *,
    db: Session | None = None,
    check_design_gate: bool = False,
) -> dict:
    available = _available_fields(db)
    capability = resolve_capabilities(raw, available_fields=available)
    parsed: StrategySpec | None = None
    error: str | None = None
    try:
        parsed = parse_strategy_spec(raw)  # type: ignore[arg-type]
    except Exception as exc:
        error = str(exc)
    normalized = parsed.model_dump(mode="json") if parsed is not None else None
    out: dict = {
        "valid": parsed is not None and capability.status == CapabilityStatus.SUPPORTED,
        "kind": parsed.kind if parsed is not None else None,
        "spec_schema_version": parsed.schema_version if parsed is not None else None,
        "normalized_spec": normalized,
        "spec": normalized,
        "spec_hash": strategy_spec_hash(parsed) if parsed is not None else None,
        "capability": capability.model_dump(mode="json"),
        "errors": [issue.message for issue in capability.issues]
        + ([error] if error else []),
    }
    if check_design_gate:
        if parsed is not None:
            checks = design_complete_checks(parsed, available_fields=available)
            out["design_complete_checks"] = checks
            out["design_complete_ready"] = all(item["ok"] for item in checks)
        else:
            out["design_complete_checks"] = []
            out["design_complete_ready"] = False
    return out


def _legacy_effective_params(strategy: Strategy) -> tuple[dict, bool]:
    if strategy.template not in STRATEGY_TEMPLATES:
        return dict(strategy.params or {}), True
    try:
        return validate_legacy_params(strategy.template, strategy.params), True
    except ValueError:
        return dict(strategy.params or {}), False


def strategy_out(strategy: Strategy, *, editable: bool = False,
                 usage: int | None = None,
                 evidence_usage: int | None = None,
                 db: Session | None = None,
                 available_fields: frozenset[str] | None = None) -> dict:
    fields = (
        available_fields if available_fields is not None
        else _available_fields(db)
    )
    try:
        spec = strategy_spec_for(strategy)
        normalized = spec.model_dump(mode="json")
        calculated_hash = strategy_spec_hash(spec)
        capability = resolve_capabilities(
            normalized, available_fields=fields,
        ).model_dump(mode="json")
    except Exception as exc:
        normalized = strategy.spec if isinstance(getattr(strategy, "spec", None), dict) else None
        calculated_hash = getattr(strategy, "spec_hash", None)
        capability = {
            "status": "missing_engine",
            "issues": [{
                "status": "missing_engine", "path": "$.spec",
                "code": "invalid_stored_spec", "message": str(exc),
            }],
        }
    effective, params_valid = _legacy_effective_params(strategy)
    legacy_meta = STRATEGY_TEMPLATES.get(strategy.template, {})
    # 证据状态由服务端状态机管理(strategy/evidence.py);这里只做展示与允许操作
    evidence_status = (
        normalized.get("metadata", {}).get("evidence_status")
        if isinstance(normalized, dict) else None
    )
    return {
        "id": strategy.id,
        "name": strategy.name,
        "kind": strategy.kind,
        "kind_name": "股票组合" if strategy.kind == "portfolio" else "单只股票",
        "spec_schema_version": getattr(strategy, "spec_schema_version", 1),
        "spec": normalized,
        "spec_hash": calculated_hash,
        "evidence_status": evidence_status,
        "evidence_actions": manual_actions_for(evidence_status) if editable else [],
        "research_status": getattr(strategy, "research_status", "unverified"),
        "capability": capability,
        "enabled": bool(strategy.enabled),
        "is_system": bool(strategy.is_system),
        "owner_id": strategy.owner_id,
        "editable": editable,
        "backtest_count": usage,
        "evidence_backtest_count": evidence_usage,
        "created_at": strategy.created_at.isoformat(sep=" ") if strategy.created_at else None,
        "updated_at": (
            strategy.updated_at.isoformat(sep=" ")
            if getattr(strategy, "updated_at", None) else None
        ),
        # 迁移期只读兼容字段，不能作为执行依据。
        "template": strategy.template,
        "template_name": template_name(strategy.template),
        "params": strategy.params or {},
        "effective_params": effective,
        "params_valid": params_valid,
        "research_plan_capabilities": CAPABILITIES.get(strategy.template, {}),
        "plan_capabilities": legacy_meta.get("plan_capability", {}),
    }


def get_strategy_or_404(db: Session, strategy_id: int, user_id: str) -> Strategy:
    strategy = db.execute(
        select(Strategy).where(Strategy.id == strategy_id, visible_to(user_id))
    ).scalar_one_or_none()
    if strategy is None:
        raise HTTPException(404, f"策略 {strategy_id} 不存在")
    return strategy


def _writable_strategy(db: Session, strategy_id: int, user_id: str) -> Strategy:
    strategy = get_strategy_or_404(db, strategy_id, user_id)
    if not can_edit(strategy, user_id):
        raise HTTPException(
            403, f"「{strategy.name}」是公共策略，不能修改。可以先另存为我的策略",
        )
    return strategy


def _check_quota(db: Session, user_id: str, *, adding: bool,
                 enabling: bool) -> None:
    if adding and count_owned(db, user_id) >= MAX_STRATEGIES_PER_USER:
        raise HTTPException(400, f"策略数量已达上限 {MAX_STRATEGIES_PER_USER}")
    if enabling and count_owned(db, user_id, enabled_only=True) >= MAX_ENABLED_PER_USER:
        raise HTTPException(400, f"启用的策略已达上限 {MAX_ENABLED_PER_USER}")


def _usage_counts(db: Session, strategy_ids: list[int]) -> dict[int, int]:
    if not strategy_ids:
        return {}
    rows = db.execute(
        select(BacktestRun.strategy_id, func.count())
        .where(BacktestRun.strategy_id.in_(strategy_ids))
        .group_by(BacktestRun.strategy_id)
    ).all()
    return {strategy_id: int(count) for strategy_id, count in rows}


def _evidence_counts(db: Session, strategies: list[Strategy]) -> dict[int, int]:
    """可作为当前规格证据的回测数,按身份哈希匹配(忽略 evidence_status)。

    证据状态推进会改变策略 spec_hash(状态是规格的一部分);旧精确匹配会在
    每次推进后把刚刚完成的回测误判为"不同规格"。同一规则内容在五种状态下
    的哈希都算同一份规格。
    """
    if not strategies:
        return {}
    expected: dict[int, set[str | None]] = {}
    for strategy in strategies:
        try:
            expected[strategy.id] = candidate_spec_hashes(
                strategy_spec_for(strategy),
            )
        except Exception:  # noqa: BLE001 - 规格不可解析时退化为精确匹配
            expected[strategy.id] = {getattr(strategy, "spec_hash", None)}
    rows = db.execute(
        select(BacktestRun.strategy_id, BacktestRun.strategy_spec_hash)
        .where(BacktestRun.strategy_id.in_(expected))
    ).all()
    counts: dict[int, int] = {}
    for strategy_id, spec_hash in rows:
        if spec_hash is not None and spec_hash in expected.get(strategy_id, set()):
            counts[strategy_id] = counts.get(strategy_id, 0) + 1
    return counts


@router.get("/templates")
def list_templates():
    """迁移期旧模板元数据；新建策略应使用结构化 StrategySpec。"""
    items = []
    for name in sorted(REGISTRY):
        item = deepcopy(STRATEGY_TEMPLATES[name])
        item["research_plan_capabilities"] = deepcopy(CAPABILITIES.get(name, {}))
        items.append(item)
    return {"items": items}


@router.post("/validate")
def validate_strategy(body: StrategyValidateIn, db: Session = Depends(get_db)):
    return validation_out(
        body.spec, db=db, check_design_gate=body.check_design_gate,
    )


@router.get("")
def list_strategies(db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    items = list_visible(db, user_id)
    usage = _usage_counts(db, [item.id for item in items])
    evidence = _evidence_counts(db, items)
    # 字段可用性与策略无关,整表探测一次即可,避免 N×10 次 LIMIT 1
    available_fields = _available_fields(db)
    return {
        "count": len(items),
        "items": [
            strategy_out(
                item, editable=can_edit(item, user_id), usage=usage.get(item.id, 0),
                evidence_usage=evidence.get(item.id, 0),
                available_fields=available_fields,
            )
            for item in items
        ],
        "limits": {"max_total": MAX_STRATEGIES_PER_USER,
                   "max_enabled": MAX_ENABLED_PER_USER},
    }


@router.post("", status_code=201)
def create_strategy(body: StrategyCreateIn, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    spec, capability = _parse_definition(spec=body.spec, db=db)
    # evidence_status 由服务端状态机管理(见 strategy/evidence.py):
    # 新建一律 unverified,客户端传入的状态值无效
    spec = with_status(spec, "unverified")
    if body.enabled:
        _require_supported(capability)
    _check_quota(db, user_id, adding=True, enabling=body.enabled)
    normalized = spec.model_dump(mode="json")
    strategy = Strategy(
        owner_id=user_id,
        is_system=False,
        name=body.name.strip(),
        template="strategy_spec",
        kind=spec.kind,
        params={},
        spec_schema_version=spec.schema_version,
        spec=normalized,
        spec_hash=strategy_spec_hash(spec),
        research_status=body.research_status,
        enabled=body.enabled,
    )
    db.add(strategy)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{body.name}」已存在")
    db.refresh(strategy)
    return strategy_out(strategy, editable=True, usage=0, evidence_usage=0, db=db)


@router.get("/{strategy_id}")
def get_strategy(strategy_id: int, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, strategy_id, user_id)
    usage = _usage_counts(db, [strategy.id])
    evidence = _evidence_counts(db, [strategy])
    return strategy_out(
        strategy, editable=can_edit(strategy, user_id), usage=usage.get(strategy.id, 0),
        evidence_usage=evidence.get(strategy.id, 0), db=db,
    )


@router.post("/{strategy_id}/validate")
def validate_saved_strategy(strategy_id: int, db: Session = Depends(get_db),
                            claims: dict = Depends(require_client)):
    strategy = get_strategy_or_404(
        db, strategy_id, user_id_from_claims(claims),
    )
    return validation_out(strategy_spec_for(strategy), db=db)


@router.post("/{strategy_id}/evidence")
def update_evidence_status(strategy_id: int, body: StrategyEvidenceIn,
                           db: Session = Depends(get_db),
                           claims: dict = Depends(require_client)):
    """证据状态手动操作:标记设计完成 / 否决复位。其余状态由回测自动推进。"""
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    try:
        transition = apply_manual_action(
            db, strategy, body.action,
            available_fields=_available_fields(db),
        )
    except DesignCompleteChecklistError as exc:
        raise HTTPException(400, detail={
            "error": "design_complete_checklist_failed",
            "checks": exc.checks,
            "message": str(exc),
        }) from exc
    except ValueError as exc:
        raise HTTPException(400, str(exc)) from exc
    db.commit()
    db.refresh(strategy)
    usage = _usage_counts(db, [strategy.id])
    evidence = _evidence_counts(db, [strategy])
    out = strategy_out(
        strategy, editable=True, usage=usage.get(strategy.id, 0),
        evidence_usage=evidence.get(strategy.id, 0), db=db,
    )
    out["evidence_transition"] = transition
    return out


@router.post("/{strategy_id}/duplicate", status_code=201)
def duplicate_strategy(strategy_id: int, body: StrategyDuplicateIn,
                       db: Session = Depends(get_db),
                       claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    source = get_strategy_or_404(db, strategy_id, user_id)
    if body.spec is not None:
        spec, capability = _parse_definition(spec=body.spec, db=db)
    else:
        spec = strategy_spec_for(source)
        capability = resolve_capabilities(
            spec, available_fields=_available_fields(db),
        )
    # 副本是新策略:证据状态重置为 unverified,不继承来源策略的验证结论
    spec = with_status(spec, "unverified")
    _require_supported(capability)
    name = (body.name or f"{source.name} 副本").strip()[:64]
    _check_quota(db, user_id, adding=True, enabling=True)
    copy = Strategy(
        owner_id=user_id,
        is_system=False,
        name=name,
        template=source.template or "strategy_spec",
        kind=spec.kind,
        params={},
        spec_schema_version=spec.schema_version,
        spec=spec.model_dump(mode="json"),
        spec_hash=strategy_spec_hash(spec),
        research_status="unverified",
        enabled=True,
    )
    db.add(copy)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{name}」已存在")
    db.refresh(copy)
    return strategy_out(copy, editable=True, usage=0, evidence_usage=0, db=db)


@router.patch("/{strategy_id}")
def update_strategy(strategy_id: int, body: StrategyPatchIn,
                    db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    if body.name is not None:
        strategy.name = body.name.strip()
    if body.spec is not None:
        spec, capability = _parse_definition(spec=body.spec, db=db)
        if bool(body.enabled if body.enabled is not None else strategy.enabled):
            _require_supported(capability)
        # evidence_status 由状态机管理:客户端传入值无效;规格内容身份变化时,
        # 高于 design_complete 的状态(含 rejected)回落——旧回测证据与旧否决
        # 结论都是针对旧规格的(见 strategy/evidence.py)
        spec = with_status(spec, resolve_status_on_edit(strategy.spec, spec))
        strategy.kind = spec.kind
        strategy.spec_schema_version = spec.schema_version
        strategy.spec = spec.model_dump(mode="json")
        strategy.spec_hash = strategy_spec_hash(spec)
    if body.enabled is not None and body.enabled != strategy.enabled:
        if body.enabled:
            _require_supported(resolve_capabilities(
                strategy_spec_for(strategy),
                available_fields=_available_fields(db),
            ))
        _check_quota(db, user_id, adding=False, enabling=body.enabled)
        strategy.enabled = body.enabled
    if body.research_status is not None:
        strategy.research_status = body.research_status
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{body.name}」已存在")
    db.refresh(strategy)
    usage = _usage_counts(db, [strategy.id])
    evidence = _evidence_counts(db, [strategy])
    return strategy_out(
        strategy, editable=True, usage=usage.get(strategy.id, 0),
        evidence_usage=evidence.get(strategy.id, 0), db=db,
    )


@router.delete("/{strategy_id}")
def delete_strategy(strategy_id: int, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    used = _usage_counts(db, [strategy.id]).get(strategy.id, 0)
    if used:
        raise HTTPException(
            409,
            f"「{strategy.name}」已被 {used} 条回测记录引用，不能删除。可以改为停用",
        )
    db.delete(strategy)
    db.commit()
    return {"deleted": 1, "id": strategy_id}
