"""数据库 StrategySpec 的校验、CRUD 与可见性接口。

`quant_strategy.spec` 是当前完整定义的唯一事实来源。用户编辑时原地更新；
`kind`、规范化哈希和能力状态全部由服务端计算，不接受客户端伪造。
"""
from __future__ import annotations

from copy import deepcopy
from typing import Literal

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field, model_validator
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..backtest.engine import validate_params as validate_legacy_params
from ..catalog import STRATEGY_TEMPLATES, template_name
from ..db import get_db
from ..models import BacktestRun, Strategy
from ..research_plan.domain import CAPABILITIES
from ..strategy.runtime import strategy_spec_for
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
    spec: dict | None = None
    enabled: bool = True
    research_status: ResearchStatus = "unverified"
    # 迁移期兼容旧客户端；服务端立即转换成完整规格，执行路径不会读取这两项。
    template: str | None = Field(None, min_length=1, max_length=32)
    params: dict = Field(default_factory=dict)

    @model_validator(mode="after")
    def require_definition(self) -> StrategyCreateIn:
        if self.spec is None and self.template is None:
            raise ValueError("spec 不能为空")
        return self


class StrategyPatchIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    spec: dict | None = None
    enabled: bool | None = None
    research_status: ResearchStatus | None = None
    # 旧客户端只改 params 时转换为新的完整规格。
    params: dict | None = None


class StrategyDuplicateIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    spec: dict | None = None
    params: dict | None = None


class StrategyValidateIn(BaseModel):
    spec: dict


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


def _parse_definition(*, spec: dict | None, template: str | None = None,
                      params: dict | None = None) -> tuple[StrategySpec, CapabilityReport]:
    raw = spec if spec is not None else _legacy_spec(template or "", params)
    capability = resolve_capabilities(raw)
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


def validation_out(raw: object) -> dict:
    capability = resolve_capabilities(raw)
    parsed: StrategySpec | None = None
    error: str | None = None
    try:
        parsed = parse_strategy_spec(raw)  # type: ignore[arg-type]
    except Exception as exc:
        error = str(exc)
    normalized = parsed.model_dump(mode="json") if parsed is not None else None
    return {
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


def _legacy_effective_params(strategy: Strategy) -> tuple[dict, bool]:
    if strategy.template not in STRATEGY_TEMPLATES:
        return dict(strategy.params or {}), True
    try:
        return validate_legacy_params(strategy.template, strategy.params), True
    except ValueError:
        return dict(strategy.params or {}), False


def strategy_out(strategy: Strategy, *, editable: bool = False,
                 usage: int | None = None,
                 evidence_usage: int | None = None) -> dict:
    try:
        spec = strategy_spec_for(strategy)
        normalized = spec.model_dump(mode="json")
        calculated_hash = strategy_spec_hash(spec)
        capability = resolve_capabilities(normalized).model_dump(mode="json")
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
    return {
        "id": strategy.id,
        "name": strategy.name,
        "kind": strategy.kind,
        "kind_name": "股票组合" if strategy.kind == "portfolio" else "单只股票",
        "spec_schema_version": getattr(strategy, "spec_schema_version", 1),
        "spec": normalized,
        "spec_hash": calculated_hash,
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
    if not strategies:
        return {}
    expected = {strategy.id: getattr(strategy, "spec_hash", None) for strategy in strategies}
    rows = db.execute(
        select(BacktestRun.strategy_id, BacktestRun.strategy_spec_hash)
        .where(BacktestRun.strategy_id.in_(expected))
    ).all()
    counts: dict[int, int] = {}
    for strategy_id, spec_hash in rows:
        if spec_hash is not None and spec_hash == expected.get(strategy_id):
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
def validate_strategy(body: StrategyValidateIn):
    return validation_out(body.spec)


@router.get("")
def list_strategies(db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    items = list_visible(db, user_id)
    usage = _usage_counts(db, [item.id for item in items])
    evidence = _evidence_counts(db, items)
    return {
        "count": len(items),
        "items": [
            strategy_out(
                item, editable=can_edit(item, user_id), usage=usage.get(item.id, 0),
                evidence_usage=evidence.get(item.id, 0),
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
    spec, capability = _parse_definition(
        spec=body.spec, template=body.template, params=body.params,
    )
    if body.enabled:
        _require_supported(capability)
    _check_quota(db, user_id, adding=True, enabling=body.enabled)
    normalized = spec.model_dump(mode="json")
    strategy = Strategy(
        owner_id=user_id,
        is_system=False,
        name=body.name.strip(),
        template=body.template or "strategy_spec",
        kind=spec.kind,
        params=dict(body.params or {}) if body.template else {},
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
    return strategy_out(strategy, editable=True, usage=0, evidence_usage=0)


@router.get("/{strategy_id}")
def get_strategy(strategy_id: int, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, strategy_id, user_id)
    usage = _usage_counts(db, [strategy.id])
    evidence = _evidence_counts(db, [strategy])
    return strategy_out(
        strategy, editable=can_edit(strategy, user_id), usage=usage.get(strategy.id, 0),
        evidence_usage=evidence.get(strategy.id, 0),
    )


@router.post("/{strategy_id}/validate")
def validate_saved_strategy(strategy_id: int, db: Session = Depends(get_db),
                            claims: dict = Depends(require_client)):
    strategy = get_strategy_or_404(
        db, strategy_id, user_id_from_claims(claims),
    )
    return validation_out(strategy_spec_for(strategy))


@router.post("/{strategy_id}/duplicate", status_code=201)
def duplicate_strategy(strategy_id: int, body: StrategyDuplicateIn,
                       db: Session = Depends(get_db),
                       claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    source = get_strategy_or_404(db, strategy_id, user_id)
    if body.spec is not None:
        spec, capability = _parse_definition(spec=body.spec)
    elif body.params is not None:
        spec, capability = _parse_definition(
            spec=None, template=source.template, params=body.params,
        )
    else:
        spec = strategy_spec_for(source)
        capability = resolve_capabilities(spec)
    _require_supported(capability)
    name = (body.name or f"{source.name} 副本").strip()[:64]
    _check_quota(db, user_id, adding=True, enabling=True)
    copy = Strategy(
        owner_id=user_id,
        is_system=False,
        name=name,
        template=source.template,
        kind=spec.kind,
        params=dict(body.params or source.params or {}) if source.template in REGISTRY else {},
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
    return strategy_out(copy, editable=True, usage=0, evidence_usage=0)


@router.patch("/{strategy_id}")
def update_strategy(strategy_id: int, body: StrategyPatchIn,
                    db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    if body.name is not None:
        strategy.name = body.name.strip()
    if body.spec is not None or body.params is not None:
        spec, capability = _parse_definition(
            spec=body.spec,
            template=strategy.template if body.spec is None else None,
            params=body.params,
        )
        if bool(body.enabled if body.enabled is not None else strategy.enabled):
            _require_supported(capability)
        strategy.kind = spec.kind
        strategy.spec_schema_version = spec.schema_version
        strategy.spec = spec.model_dump(mode="json")
        strategy.spec_hash = strategy_spec_hash(spec)
        if body.params is not None and strategy.template in REGISTRY:
            strategy.params = dict(body.params)
    if body.enabled is not None and body.enabled != strategy.enabled:
        if body.enabled:
            _require_supported(resolve_capabilities(strategy_spec_for(strategy)))
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
        evidence_usage=evidence.get(strategy.id, 0),
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
