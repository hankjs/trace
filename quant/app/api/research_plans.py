"""策略研究计划查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..models import ResearchPlan
from ..research_plan.service import plan_detail, plan_summary, visible_to

router = APIRouter(prefix="/api/research-plans", tags=["research-plans"])


@router.get("")
def list_research_plans(
    strategy_id: int | None = None,
    code: str | None = None,
    date_: date | None = Query(None, alias="date"),
    plan_type: str | None = None,
    limit: int = Query(100, ge=1, le=500),
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    user_id = user_id_from_claims(claims)
    query = (select(ResearchPlan).where(visible_to(user_id))
             .order_by(ResearchPlan.data_date.desc(), ResearchPlan.id.desc())
             .limit(limit))
    if strategy_id is not None:
        query = query.where(ResearchPlan.strategy_id == strategy_id)
    if code is not None:
        query = query.where(ResearchPlan.code == code.lower())
    if date_ is not None:
        query = query.where(ResearchPlan.data_date == date_)
    if plan_type is not None:
        query = query.where(ResearchPlan.plan_type == plan_type)
    plans = db.execute(query).scalars().all()
    evidence_cache: dict = {}
    plan_ids = [plan.id for plan in plans]
    superseded_plan_ids = set(db.execute(
        select(ResearchPlan.supersedes_plan_id).where(
            ResearchPlan.supersedes_plan_id.in_(plan_ids)
        )
    ).scalars()) if plan_ids else set()
    read_context = {"superseded_plan_ids": superseded_plan_ids}
    return {
        "count": len(plans),
        "items": [
            # 列表轻量:不做原生条件重算与证据扫库;详情接口仍实时评估
            plan_summary(
                plan, db=db, viewer_user_id=user_id,
                evidence_cache=evidence_cache,
                read_context=read_context,
                reevaluate=False,
                resolve_evidence=False,
            )
            for plan in plans
        ],
    }


@router.get("/{plan_id}")
def get_research_plan(
    plan_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    user_id = user_id_from_claims(claims)
    plan = db.execute(select(ResearchPlan).where(
        ResearchPlan.id == plan_id, visible_to(user_id))) \
        .scalar_one_or_none()
    if plan is None:
        raise HTTPException(404, f"研究计划 {plan_id} 不存在")
    return plan_detail(db, plan, viewer_user_id=user_id)
