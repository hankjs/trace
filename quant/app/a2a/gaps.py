"""A2A 缺口聚合:审计缺口列与 research findings 双源合并排行。

本模块供 A2A skill 与 REST 管理端共同调用,避免复制 SQL。
"""
from __future__ import annotations

from collections import Counter
from datetime import date, datetime, timedelta
from typing import TYPE_CHECKING

from sqlalchemy import func, select

from ..models import A2aAudit, ResearchFinding

if TYPE_CHECKING:
    from sqlalchemy.orm import Session


def aggregate_gaps(
    db: "Session",
    *,
    user_id: str | None,
    scope: str,
    limit: int,
    since_days: int,
) -> dict:
    """按 missing_capability / failure_kind 聚合审计缺口与 findings。

    参数:
        db: SQLAlchemy Session
        user_id: scope='me' 时必填,用于过滤两张表;scope='global' 时忽略
        scope: 'me' 或 'global'
        limit: 返回条数上限(调用方已 clamp)
        since_days: 回溯天数(调用方已 clamp)

    返回:
        {
            "audit_items": [...],
            "finding_items": [...],
            "merged": [...],
            "note": "aggregate_of_a2a_audit_not_llm_advice",
        }
    """
    since = datetime.combine(date.today() - timedelta(days=since_days), datetime.min.time())

    # 两张表各自过滤,不能把 A2aAudit.user_id 带进 ResearchFinding 查询,
    # 否则会隐式把 quant_a2a_audit 加进 FROM 形成笛卡尔积。
    audit_filter = True if scope == "global" else (A2aAudit.user_id == user_id)
    finding_filter = True if scope == "global" else (ResearchFinding.user_id == user_id)

    audit_rows = db.execute(
        select(
            A2aAudit.missing_capability,
            A2aAudit.failure_kind,
            func.count(A2aAudit.id),
            func.max(A2aAudit.created_at),
        )
        .where(A2aAudit.created_at >= since)
        .where(A2aAudit.missing_capability.isnot(None) | A2aAudit.failure_kind.isnot(None))
        .where(audit_filter)
        .group_by(A2aAudit.missing_capability, A2aAudit.failure_kind)
        .order_by(func.count(A2aAudit.id).desc())
        .limit(limit)
    ).all()

    finding_rows = db.execute(
        select(
            ResearchFinding.kind,
            ResearchFinding.detail,
            func.count(ResearchFinding.id),
            func.max(ResearchFinding.created_at),
        )
        .where(ResearchFinding.created_at >= since)
        .where(finding_filter)
        .group_by(ResearchFinding.kind, ResearchFinding.detail)
        .order_by(func.count(ResearchFinding.id).desc())
        .limit(limit)
    ).all()

    audit_items = [
        {
            "missing_capability": missing or "",
            "failure_kind": failure or "",
            "count": int(cnt),
            "last_seen": ts.isoformat(sep=" ") if ts else None,
            "source": "audit",
        }
        for missing, failure, cnt, ts in audit_rows
    ]
    finding_items = [
        {
            "missing_capability": detail or "",
            "failure_kind": kind,
            "count": int(cnt),
            "last_seen": ts.isoformat(sep=" ") if ts else None,
            "source": "finding",
        }
        for kind, detail, cnt, ts in finding_rows
    ]

    merged_counter: Counter[tuple[str, str]] = Counter()
    for item in audit_items + finding_items:
        key = (item["missing_capability"], item["failure_kind"])
        merged_counter[key] += item["count"]

    merged = [
        {
            "missing_capability": mc,
            "failure_kind": fk,
            "count": cnt,
        }
        for (mc, fk), cnt in merged_counter.most_common(limit)
    ]

    return {
        "audit_items": audit_items,
        "finding_items": finding_items,
        "merged": merged,
        "note": "aggregate_of_a2a_audit_not_llm_advice",
    }


__all__ = ["aggregate_gaps"]
