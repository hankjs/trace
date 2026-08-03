"""因子评估列表与详情的 domain 查询。

REST(`app/api/factors.py`)与 A2A skill 共用本模块,避免两处复制 SQL 与
序列化口径 —— 口径漂移会让 agent 与看板看到不同的评估结论。
"""
from __future__ import annotations

from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import FactorEvaluation

MAX_LIST_LIMIT = 50


class EvaluationNotFoundError(ValueError):
    """评估不存在或不属于当前用户(两种情况统一按不存在处理,防探测)。"""


def evaluation_detail(row: FactorEvaluation) -> dict[str, Any]:
    """完整详情:含 result 全量。"""
    return {
        "id": row.id,
        "factor_key": row.factor_key,
        "expression": row.expression,
        "expression_hash": row.expression_hash,
        "start": str(row.start),
        "end": str(row.end),
        "pool_id": row.pool_id,
        "codes": row.codes,
        "layers": row.layers,
        "rebalance": row.rebalance,
        "neutralize": row.neutralize or [],
        "horizons": row.horizons or [],
        "universe": row.universe,
        "result": row.result,
        "status": row.status,
        "error": row.error,
        "created_at": (
            row.created_at.isoformat(sep=" ") if row.created_at else None
        ),
        "finished_at": (
            row.finished_at.isoformat(sep=" ") if row.finished_at else None
        ),
    }


def evaluation_summary(row: FactorEvaluation) -> dict[str, Any]:
    """列表用摘要:只带 IC 头部指标与口径,不带分层/衰减明细。

    列表是 agent 做「上一轮跑过什么」横向对比用的,全量 result 会迅速吃掉
    上下文;要明细请按 id 取详情。
    """
    result = row.result or {}
    ic = result.get("ic") or {}
    multiplicity = result.get("multiplicity") or {}
    return {
        "id": row.id,
        "factor_key": row.factor_key,
        "expression_hash": row.expression_hash,
        "start": str(row.start),
        "end": str(row.end),
        "pool_id": row.pool_id,
        "layers": row.layers,
        "rebalance": row.rebalance,
        "neutralize": row.neutralize or [],
        "horizons": row.horizons or [],
        "universe_size": (row.universe or {}).get("size"),
        "status": row.status,
        "error": row.error,
        "ic": {
            "ic_mean": ic.get("ic_mean"),
            "rank_ic_mean": ic.get("rank_ic_mean"),
            "icir": ic.get("icir"),
            "ic_t_stat": ic.get("ic_t_stat"),
            "ic_p_value": ic.get("ic_p_value"),
            "n_periods": ic.get("n_periods"),
        },
        "survives_bonferroni": multiplicity.get("survives_bonferroni"),
        "created_at": (
            row.created_at.isoformat(sep=" ") if row.created_at else None
        ),
    }


def list_evaluations(
    db: Session,
    *,
    user_id: str,
    factor_key: str | None = None,
    status: str | None = None,
    limit: int = 20,
    before_id: int | None = None,
) -> dict[str, Any]:
    """列出本人因子评估摘要,id 倒序游标分页。"""
    limit = max(1, min(int(limit), MAX_LIST_LIMIT))
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
        "items": [evaluation_summary(row) for row in rows[:limit]],
        "has_more": has_more,
        "note": "summary_only_fetch_detail_by_id",
    }


def get_evaluation(
    db: Session, *, user_id: str, evaluation_id: int,
) -> dict[str, Any]:
    """取本人单条评估详情;非本人按不存在处理。"""
    row = db.execute(
        select(FactorEvaluation).where(
            FactorEvaluation.id == evaluation_id,
            FactorEvaluation.user_id == user_id,
        )
    ).scalar_one_or_none()
    if row is None:
        raise EvaluationNotFoundError(f"评估 {evaluation_id} 不存在")
    return evaluation_detail(row)


__all__ = [
    "EvaluationNotFoundError",
    "MAX_LIST_LIMIT",
    "evaluation_detail",
    "evaluation_summary",
    "get_evaluation",
    "list_evaluations",
]
