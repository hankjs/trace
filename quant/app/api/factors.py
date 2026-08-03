"""动态因子定义 CRUD、表达式校验、预览与回填 API。

所有响应形状与 web/src/factors.ts 的 TypeScript 类型保持一致。
"""
from __future__ import annotations

import math
import re
from datetime import date
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from ..auth import require_admin, require_client, user_id_from_claims
from ..catalog import FILTER_FIELDS
from ..data.ingest import BAR_FIELDS, SNAPSHOT_SPEC_FIELDS, load_bars_df
from ..db import get_db
from ..factors import build_reason_tree, evaluate_factor, invalidate_factor_cache
from ..factors.backfill import run_factor_backfill_task
from ..factors.listing import (
    EvaluationNotFoundError,
    evaluation_detail,
)
from ..factors.listing import get_evaluation as get_evaluation_detail
from ..models import FactorDef, FactorEvaluation, SelectionConfig
from ..selection.config import (
    SelectionConfigUpdateIn,
    get_active_selection_config_row,
    invalidate_selection_config_cache,
    load_selection_config,
    validate_selection_config,
)
from ..strategy.spec import (
    SUPPORTED_FIELDS,
    ExpressionValidationResult,
    validate_expression,
)
from ..tasks import submit_task, task_payload

router = APIRouter(prefix="/api/factors", tags=["factors"])

_KEY_RE = re.compile(r"^[a-z][a-z0-9_]*$")


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class FactorCreateIn(StrictModel):
    key: str = Field(..., min_length=1, max_length=64)
    name: str = Field(..., min_length=1, max_length=128)
    description: str | None = Field(None, max_length=512)
    category: str | None = Field(None, max_length=64)
    unit: str | None = Field(None, max_length=32)
    direction: str | None = Field(None, max_length=256)
    limits: str | None = Field(None, max_length=256)
    value_type: str | None = Field(None, max_length=16)
    input_scale: float | None = Field(None)
    expression: dict[str, Any]
    enabled: bool = True

    @field_validator("key")
    @classmethod
    def validate_key(cls, v: str) -> str:
        if not _KEY_RE.fullmatch(v):
            raise ValueError("key 必须以小写字母开头,只能包含小写字母、数字、下划线")
        return v


class FactorPatchIn(StrictModel):
    name: str | None = Field(None, min_length=1, max_length=128)
    description: str | None = Field(None, max_length=512)
    category: str | None = Field(None, max_length=64)
    unit: str | None = Field(None, max_length=32)
    direction: str | None = Field(None, max_length=256)
    limits: str | None = Field(None, max_length=256)
    value_type: str | None = Field(None, max_length=16)
    input_scale: float | None = Field(None)
    expression: dict[str, Any] | None = None
    enabled: bool | None = None


class FactorValidateIn(StrictModel):
    expression: dict[str, Any]


class FactorPreviewIn(StrictModel):
    expression: dict[str, Any] | None = None
    factor_key: str | None = None
    code: str = Field(..., min_length=1)
    days: int = Field(default=60, ge=1, le=500)

    @model_validator(mode="after")
    def exactly_one_source(self) -> "FactorPreviewIn":
        has_expr = self.expression is not None
        has_key = self.factor_key is not None
        if has_expr == has_key:
            raise ValueError("必须且只能提供 expression 或 factor_key 之一")
        return self


class FactorBackfillIn(StrictModel):
    factor_key: str | None = None
    start: date
    end: date
    codes: list[str] | None = None


class SelectionConfigPutIn(StrictModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    score_weights: dict[str, float]
    vol_confirm: dict[str, Any]
    hard_filters: list[dict[str, Any]]
    top_n: int = Field(..., ge=1, le=100)


def _available_fields(db: Session) -> frozenset[str]:
    from ..data.ingest import snapshot_available_fields

    return BAR_FIELDS | snapshot_available_fields(db)


def _used_fields(expr: dict) -> set[str]:
    from ..strategy.spec import _walk_expression, parse_expression

    return {
        node.name
        for node in _walk_expression(parse_expression(expr))
        if node.op == "field" and node.name is not None
    }


def _factor_out(def_: FactorDef) -> dict[str, Any]:
    return {
        "id": def_.id,
        "key": def_.key,
        "name": def_.name,
        "description": def_.description or None,
        "category": def_.category or None,
        "unit": def_.unit,
        "direction": def_.direction or None,
        "limits": def_.limits or None,
        "value_type": def_.value_type or None,
        "input_scale": def_.input_scale,
        "expression": def_.expression,
        "expression_hash": def_.expression_hash,
        "min_bars": def_.min_bars,
        "enabled": bool(def_.enabled),
        "is_system": bool(def_.is_system),
        "created_at": (
            def_.created_at.isoformat(sep=" ") if def_.created_at else None
        ),
        "updated_at": (
            def_.updated_at.isoformat(sep=" ") if def_.updated_at else None
        ),
    }


def _get_factor_or_404(db: Session, key: str) -> FactorDef:
    row = db.execute(
        select(FactorDef).where(FactorDef.key == key)
    ).scalar_one_or_none()
    if row is None:
        raise HTTPException(404, f"因子 {key} 不存在")
    return row


def _validate_factor_expression(db: Session, expr: dict) -> ExpressionValidationResult:
    result = validate_expression(
        expr, require_type="number", available_fields=_available_fields(db),
    )
    if not result.valid:
        raise HTTPException(
            422,
            {
                "message": "表达式校验失败",
                "capability": result.capability.model_dump(mode="json"),
                "errors": [issue.message for issue in result.capability.issues],
            },
        )
    return result


def _check_key_collision(key: str) -> None:
    if key in FILTER_FIELDS or key in SUPPORTED_FIELDS:
        raise HTTPException(
            409, f"key {key} 与系统保留字段冲突,请更换"
        )


@router.get("")
def list_factors(db: Session = Depends(get_db),
                 _claims: dict = Depends(require_client)):
    """列出全部因子定义(含禁用),供管理页展示。"""
    from ..factors import load_all_defs

    items = load_all_defs(db)
    return {"items": [_factor_out(item) for item in items]}


@router.get("/{key}")
def get_factor(key: str, db: Session = Depends(get_db),
               _claims: dict = Depends(require_client)):
    """获取单个因子定义。"""
    return _factor_out(_get_factor_or_404(db, key))


@router.post("", status_code=201)
def create_factor(body: FactorCreateIn,
                  db: Session = Depends(get_db),
                  _claims: dict = Depends(require_admin)):
    """创建因子定义;表达式哈希与 min_bars 由服务端计算。"""
    _check_key_collision(body.key)
    result = _validate_factor_expression(db, body.expression)
    def_ = FactorDef(
        key=body.key,
        name=body.name,
        description=body.description or "",
        category=body.category or "",
        unit=body.unit,
        direction=body.direction or "",
        limits=body.limits or "",
        value_type=body.value_type or "number",
        input_scale=body.input_scale,
        expression=body.expression,
        expression_hash=result.expression_hash or "",
        min_bars=result.min_bars or 1,
        enabled=body.enabled,
    )
    db.add(def_)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(409, f"因子 key {body.key} 已存在")
    db.refresh(def_)
    invalidate_factor_cache()
    return _factor_out(def_)


@router.patch("/{key}")
def update_factor(key: str, body: FactorPatchIn,
                  db: Session = Depends(get_db),
                  _claims: dict = Depends(require_admin)):
    """部分更新因子定义;修改表达式会触发重新校验与哈希。"""
    def_ = _get_factor_or_404(db, key)
    if body.name is not None:
        def_.name = body.name
    if body.description is not None:
        def_.description = body.description
    if body.category is not None:
        def_.category = body.category
    if body.unit is not None:
        def_.unit = body.unit
    if body.direction is not None:
        def_.direction = body.direction
    if body.limits is not None:
        def_.limits = body.limits
    if body.value_type is not None:
        def_.value_type = body.value_type
    if body.input_scale is not None:
        def_.input_scale = body.input_scale
    if body.enabled is not None:
        def_.enabled = body.enabled
    if body.expression is not None:
        result = _validate_factor_expression(db, body.expression)
        def_.expression = body.expression
        def_.expression_hash = result.expression_hash or ""
        def_.min_bars = result.min_bars or 1
    db.commit()
    db.refresh(def_)
    invalidate_factor_cache()
    return _factor_out(def_)


@router.delete("/{key}", status_code=204)
def delete_factor(key: str, db: Session = Depends(get_db),
                  _claims: dict = Depends(require_admin)):
    """删除因子定义;系统因子只能禁用,不能删除。"""
    def_ = _get_factor_or_404(db, key)
    if def_.is_system:
        raise HTTPException(409, "系统因子只能禁用")
    db.delete(def_)
    db.commit()
    invalidate_factor_cache()
    return None


@router.post("/validate")
def validate_factor_expression(body: FactorValidateIn,
                               db: Session = Depends(get_db),
                               _claims: dict = Depends(require_client)):
    """校验表达式并返回规范化哈希、min_bars 等信息。"""
    result = _validate_factor_expression(db, body.expression)
    return result.model_dump(mode="json")


@router.post("/preview")
def preview_factor(body: FactorPreviewIn,
                   db: Session = Depends(get_db),
                   _claims: dict = Depends(require_client)):
    """预览表达式/因子在指定股票上的最近 N 日序列与原因树。"""
    if body.factor_key is not None:
        def_ = _get_factor_or_404(db, body.factor_key)
        expr = def_.expression
    else:
        expr = body.expression

    result = validate_expression(
        expr, require_type="number", available_fields=_available_fields(db),
    )
    if not result.valid:
        raise HTTPException(
            422,
            {
                "message": "表达式校验失败",
                "capability": result.capability.model_dump(mode="json"),
                "errors": [issue.message for issue in result.capability.issues],
            },
        )

    needed = _used_fields(expr)
    extra_fields = sorted(needed & set(SNAPSHOT_SPEC_FIELDS))
    df = load_bars_df(db, body.code, extra_fields=extra_fields or None)
    if df.empty:
        raise HTTPException(422, f"未找到 {body.code} 的日线数据")

    series = evaluate_factor(expr, df)

    from ..strategy.spec import parse_expression

    # 原因树必须在完整窗口上构建:截尾到 N 日后窗口类操作符(ma20 等)
    # 得不到足够历史,整棵树会退化成 NaN。
    full_fields = {col: df[col] for col in df.columns if col != "date"}
    reason_tree = build_reason_tree(
        parse_expression(expr),
        full_fields,
        position=-1,
    )

    # 取最后 days 条有效日线对齐
    df = df.tail(body.days).reset_index(drop=True)
    series = series.tail(body.days).reset_index(drop=True)
    dates = [str(d) for d in df["date"]]
    values: list[float | None] = []
    for v in series:
        if v is None or (isinstance(v, float) and (math.isnan(v) or not math.isfinite(v))):
            values.append(None)
        else:
            values.append(round(float(v), 12))

    return {
        "code": body.code,
        "dates": dates,
        "values": values,
        "reason_tree": reason_tree,
    }


@router.get("/selection-config")
def get_selection_config(db: Session = Depends(get_db),
                         _claims: dict = Depends(require_client)):
    """获取当前 active 的选股配置。"""
    config = load_selection_config(db)
    return _selection_config_out(config)


def _selection_config_out(config: SelectionConfig) -> dict[str, Any]:
    return {
        "id": config.id,
        "name": config.name,
        "is_active": bool(config.is_active),
        "score_weights": config.score_weights or {},
        "vol_confirm": config.vol_confirm or {},
        "hard_filters": config.hard_filters or [],
        "top_n": config.top_n,
        "updated_at": (
            config.updated_at.isoformat(sep=" ") if config.updated_at else None
        ),
    }


@router.put("/selection-config")
def update_selection_config(body: SelectionConfigPutIn,
                            db: Session = Depends(get_db),
                            _claims: dict = Depends(require_admin)):
    """全量替换 active 选股配置;校验失败返回 422 与错误列表。"""
    payload = body.model_dump()
    errors = validate_selection_config(db, payload)
    if errors:
        raise HTTPException(422, {"message": "选股配置校验失败", "errors": errors})

    config = get_active_selection_config_row(db)
    if body.name is not None:
        config.name = body.name
    config.score_weights = body.score_weights
    config.vol_confirm = body.vol_confirm
    config.hard_filters = body.hard_filters
    config.top_n = body.top_n
    db.commit()
    db.refresh(config)
    invalidate_selection_config_cache()
    return _selection_config_out(config)


@router.post("/backfill")
def backfill_factors(body: FactorBackfillIn,
                     db: Session = Depends(get_db),
                     claims: dict = Depends(require_admin)):
    """提交因子回填任务;返回 202 与任务摘要。"""
    user_id = user_id_from_claims(claims)
    task = submit_task(
        db,
        user_id=user_id,
        type="factor_backfill",
        title="因子回填",
        params={
            "factor_key": body.factor_key,
            "start": str(body.start),
            "end": str(body.end),
            "codes": body.codes,
        },
    )
    return {"task": task_payload(task)}


@router.get("/evaluations")
def list_evaluations(
    limit: int = Query(default=20, ge=1, le=50),
    before_id: int | None = Query(default=None),
    factor_key: str | None = Query(default=None),
    status: str | None = Query(default=None),
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """列出本人因子评估结果,limit 上限 50,before_id 游标分页。

    看板历史依赖 items[].result 全量,故这里保留详情形状;A2A 侧走摘要口径
    (`factor.evaluation_list`)以省 agent 上下文。
    """
    user_id = user_id_from_claims(claims)
    q = select(FactorEvaluation).where(FactorEvaluation.user_id == user_id)
    if factor_key:
        q = q.where(FactorEvaluation.factor_key == factor_key)
    if status:
        q = q.where(FactorEvaluation.status == status)
    if before_id is not None:
        q = q.where(FactorEvaluation.id < before_id)
    q = q.order_by(FactorEvaluation.id.desc()).limit(limit + 1)
    rows = list(db.execute(q).scalars().all())
    has_more = len(rows) > limit
    return {
        "items": [evaluation_detail(row) for row in rows[:limit]],
        "has_more": has_more,
    }


@router.get("/evaluations/{evaluation_id}")
def get_evaluation(
    evaluation_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """获取本人单条因子评估详情;非本人按 404 防探测。"""
    user_id = user_id_from_claims(claims)
    try:
        return get_evaluation_detail(
            db, user_id=user_id, evaluation_id=evaluation_id,
        )
    except EvaluationNotFoundError as exc:
        raise HTTPException(404, str(exc)) from exc


__all__ = ["router"]
