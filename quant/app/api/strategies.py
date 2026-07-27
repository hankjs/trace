"""策略 CRUD。

策略 = 算法模板 + 一组参数 + 用户起的名字,存 `quant_strategy`(见 alembic 0012)。
分两类:

- **公共策略**(`is_system=true`):全用户可读、**不可改不可删**。现有 6 个算法各
  一条,参数为模板默认值。
- **自定义策略**:按 `owner_id` 归属,只有属主可见可改。

改别人的策略统一返回 404 而不是 403,与 `api/pools.py` 同口径:否则可以靠状态码
枚举出别人建了哪些策略。

`POST /{id}/duplicate` 是「另存为我的策略」——公共策略只读,用户要调参就先复制
一份。这与池的「另存为自定义池」是同一套交互。
"""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..backtest.engine import validate_params
from ..catalog import STRATEGY_TEMPLATES, template_name
from ..db import get_db
from ..models import BacktestRun, Strategy
from ..strategy.store import (MAX_ENABLED_PER_USER, MAX_STRATEGIES_PER_USER,
                              can_edit, count_owned, list_visible, visible_to)
from ..strategy.strategies import REGISTRY
from ..research_plan.domain import CAPABILITIES

router = APIRouter(prefix="/api/strategies", tags=["strategies"])


class StrategyCreateIn(BaseModel):
    name: str = Field(..., min_length=1, max_length=64)
    template: str = Field(..., min_length=1, max_length=32)
    # 只需给要覆盖的键,其余走模板默认值
    params: dict = Field(default_factory=dict)
    enabled: bool = True


class StrategyPatchIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    params: dict | None = None
    enabled: bool | None = None


class StrategyDuplicateIn(BaseModel):
    """另存为我的策略。name 留空则在原名后加「副本」。"""

    name: str | None = Field(None, min_length=1, max_length=64)
    params: dict | None = None


def strategy_out(strategy: Strategy, *, editable: bool = False,
                 usage: int | None = None) -> dict:
    """策略的对外形状(与前端 `Strategy` 类型一致)。

    带上 template 的元数据:前端渲染参数表单要参数定义,少一次请求。
    `effective_params` 是合并默认值后的实际生效参数,避免前端重复实现合并逻辑。
    """
    template = STRATEGY_TEMPLATES.get(strategy.template, {})
    try:
        effective = validate_params(strategy.template, strategy.params)
    except ValueError:
        # 模板参数改名后库里可能残留无效键;不要让整个列表接口 500,
        # 前端据 params_valid=false 提示用户修正
        effective, valid = dict(strategy.params or {}), False
    else:
        valid = True
    return {
        "id": strategy.id,
        "name": strategy.name,
        "template": strategy.template,
        "template_name": template_name(strategy.template),
        "kind": strategy.kind,
        "kind_name": template.get("kind_name", strategy.kind),
        "research_plan_capabilities": CAPABILITIES.get(strategy.template, {}),
        "params": strategy.params or {},
        "effective_params": effective,
        "params_valid": valid,
        "enabled": bool(strategy.enabled),
        "is_system": bool(strategy.is_system),
        "owner_id": strategy.owner_id,
        "editable": editable,
        # 被多少条回测引用:>0 时不能删(外键 RESTRICT),前端据此禁用删除按钮
        "backtest_count": usage,
        "created_at": strategy.created_at.isoformat(sep=" ") if strategy.created_at else None,
    }


def get_strategy_or_404(db: Session, strategy_id: int, user_id: str) -> Strategy:
    """按可见性取策略;不可见的按不存在处理(见模块文档字符串)。"""
    strategy = db.execute(
        select(Strategy).where(Strategy.id == strategy_id, visible_to(user_id))
    ).scalar_one_or_none()
    if strategy is None:
        raise HTTPException(404, f"策略 {strategy_id} 不存在")
    return strategy


def _writable_strategy(db: Session, strategy_id: int, user_id: str) -> Strategy:
    """取策略并要求可写:公共策略一律拒绝。"""
    strategy = get_strategy_or_404(db, strategy_id, user_id)
    if not can_edit(strategy, user_id):
        raise HTTPException(
            403, f"「{strategy.name}」是公共策略，不能修改。可以先「另存为我的策略」再调参")
    return strategy


def _validated_template(template: str) -> str:
    if template not in REGISTRY or template not in STRATEGY_TEMPLATES:
        raise HTTPException(
            400, f"未知算法模板 {template}，可选: {', '.join(sorted(REGISTRY))}")
    return template


def _validated_params(template: str, params: dict | None) -> dict:
    """校验参数并**只存用户显式给的键**。

    刻意不存合并后的全量参数:模板默认值调整后,用户没碰过的参数应当跟着变。
    实际生效值由 `validate_params` 在跑的时候合并(回测落库时才固化快照)。
    """
    try:
        validate_params(template, params)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return dict(params or {})


def _check_quota(db: Session, user_id: str, *, adding: bool,
                 enabling: bool) -> None:
    """配额检查。启用数单独限:夜间信号引擎的成本是股票数 × 启用策略数。"""
    if adding and count_owned(db, user_id) >= MAX_STRATEGIES_PER_USER:
        raise HTTPException(
            400, f"策略数量已达上限 {MAX_STRATEGIES_PER_USER}，请先删除不用的策略")
    if enabling and count_owned(db, user_id, enabled_only=True) >= MAX_ENABLED_PER_USER:
        raise HTTPException(
            400,
            f"启用的策略已达上限 {MAX_ENABLED_PER_USER}。"
            "启用的策略每天都要参与信号计算，请先停用一些",
        )


def _usage_counts(db: Session, strategy_ids: list[int]) -> dict[int, int]:
    """各策略被回测记录引用的条数(外键 RESTRICT,>0 则不可删)。"""
    if not strategy_ids:
        return {}
    rows = db.execute(
        select(BacktestRun.strategy_id, func.count())
        .where(BacktestRun.strategy_id.in_(strategy_ids))
        .group_by(BacktestRun.strategy_id)
    ).all()
    return {sid: int(n) for sid, n in rows}


@router.get("/templates")
def list_templates():
    """算法模板元数据:参数定义、限制说明、单标的/组合。

    与 `GET /api/catalog` 的 `strategy_templates` 同源,这里单独出一份是为了
    新建策略页不必拉整份目录。
    """
    from copy import deepcopy
    items = []
    for name in sorted(REGISTRY):
        item = deepcopy(STRATEGY_TEMPLATES[name])
        item["research_plan_capabilities"] = deepcopy(CAPABILITIES.get(name, {}))
        items.append(item)
    return {"items": items}


@router.get("")
def list_strategies(db: Session = Depends(get_db),
                   claims: dict = Depends(require_client)):
    """我能看到的策略:公共的在前,然后是我自建的。"""
    user_id = user_id_from_claims(claims)
    items = list_visible(db, user_id)
    usage = _usage_counts(db, [s.id for s in items])
    return {
        "count": len(items),
        "items": [
            strategy_out(s, editable=can_edit(s, user_id),
                         usage=usage.get(s.id, 0))
            for s in items
        ],
        "limits": {"max_total": MAX_STRATEGIES_PER_USER,
                   "max_enabled": MAX_ENABLED_PER_USER},
    }


@router.get("/{strategy_id}")
def get_strategy(strategy_id: int, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, strategy_id, user_id)
    usage = _usage_counts(db, [strategy.id])
    return strategy_out(strategy, editable=can_edit(strategy, user_id),
                        usage=usage.get(strategy.id, 0))


@router.post("", status_code=201)
def create_strategy(body: StrategyCreateIn, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    template = _validated_template(body.template)
    params = _validated_params(template, body.params)
    _check_quota(db, user_id, adding=True, enabling=body.enabled)
    strategy = Strategy(
        owner_id=user_id, is_system=False, name=body.name.strip(),
        template=template,
        # kind 由模板决定,不接受客户端传入:否则组合策略可能被标成单标的
        # 而进到按个股跑的信号引擎里
        kind=REGISTRY[template].KIND,
        params=params, enabled=body.enabled,
    )
    db.add(strategy)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{body.name}」已存在")
    db.refresh(strategy)
    return strategy_out(strategy, editable=True, usage=0)


@router.post("/{strategy_id}/duplicate", status_code=201)
def duplicate_strategy(strategy_id: int, body: StrategyDuplicateIn,
                       db: Session = Depends(get_db),
                       claims: dict = Depends(require_client)):
    """另存为我的策略。公共策略只读,调参前先复制一份。"""
    user_id = user_id_from_claims(claims)
    source = get_strategy_or_404(db, strategy_id, user_id)
    params = _validated_params(
        source.template,
        source.params if body.params is None else body.params)
    name = (body.name or f"{source.name} 副本").strip()[:64]
    _check_quota(db, user_id, adding=True, enabling=True)
    copy = Strategy(
        owner_id=user_id, is_system=False, name=name,
        template=source.template, kind=source.kind,
        params=params, enabled=True,
    )
    db.add(copy)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{name}」已存在")
    db.refresh(copy)
    return strategy_out(copy, editable=True, usage=0)


@router.patch("/{strategy_id}")
def update_strategy(strategy_id: int, body: StrategyPatchIn,
                    db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    """改名 / 改参数 / 启停。模板不可改 —— 换算法就是另一个策略,请新建。"""
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    if body.name is not None:
        strategy.name = body.name.strip()
    if body.params is not None:
        strategy.params = _validated_params(strategy.template, body.params)
    if body.enabled is not None and body.enabled != strategy.enabled:
        _check_quota(db, user_id, adding=False, enabling=body.enabled)
        strategy.enabled = body.enabled
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"策略名「{body.name}」已存在")
    db.refresh(strategy)
    usage = _usage_counts(db, [strategy.id])
    return strategy_out(strategy, editable=True, usage=usage.get(strategy.id, 0))


@router.delete("/{strategy_id}")
def delete_strategy(strategy_id: int, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    """删除策略。被回测记录引用时拒绝(外键 RESTRICT),引导改用停用。

    信号与评估结果会随之级联删除 —— 它们是定时任务的派生数据。回测不是,
    见 alembic 0012 对两种 ON DELETE 的说明。
    """
    user_id = user_id_from_claims(claims)
    strategy = _writable_strategy(db, strategy_id, user_id)
    used = _usage_counts(db, [strategy.id]).get(strategy.id, 0)
    if used:
        raise HTTPException(
            409,
            f"「{strategy.name}」已被 {used} 条回测记录引用，不能删除。"
            "可以改为「停用」，历史回测将保持可查",
        )
    db.delete(strategy)
    db.commit()
    return {"deleted": 1, "id": strategy_id}
