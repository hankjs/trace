"""system.report_finding skill。"""
from __future__ import annotations

from datetime import date, datetime
from typing import Any

from sqlalchemy import and_, select

from ...models import ResearchFinding
from ._common import A2AContext


ALLOWED_KINDS = frozenset({
    "missing_engine",
    "missing_data",
    "low_coverage",
    "product_gap",
    "ux_friction",
    "hypothesis_rejected",
})
MAX_DETAIL_LEN = 512
MAX_BATCH_SIZE = 20


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """批量写入 research findings，单日内 session_ref+kind+detail 去重。"""
    findings = payload.get("findings", [])
    if not isinstance(findings, list):
        raise ValueError("findings 必须是列表")
    if len(findings) > MAX_BATCH_SIZE:
        raise ValueError(f"单次 findings 不能超过 {MAX_BATCH_SIZE} 条")

    session_ref = payload.get("session_ref")
    source = ctx.source or "a2a"
    today = datetime.combine(date.today(), datetime.min.time())

    inserted = 0
    skipped = 0
    for item in findings:
        kind = item.get("kind")
        if kind not in ALLOWED_KINDS:
            raise ValueError(f"不支持的 finding kind: {kind}")
        detail = str(item.get("detail", ""))
        if len(detail) > MAX_DETAIL_LEN:
            raise ValueError(f"detail 超过 {MAX_DETAIL_LEN} 字符")

        dup = ctx.db.execute(
            select(ResearchFinding).where(
                and_(
                    ResearchFinding.user_id == ctx.user_id,
                    ResearchFinding.kind == kind,
                    ResearchFinding.detail == detail,
                    ResearchFinding.session_ref == session_ref,
                    ResearchFinding.created_at >= today,
                )
            )
        ).scalar_one_or_none()
        if dup is not None:
            skipped += 1
            continue

        ctx.db.add(
            ResearchFinding(
                user_id=ctx.user_id,
                kind=kind,
                detail=detail,
                evidence=item.get("evidence"),
                suggested_system_work=item.get("suggested_system_work"),
                experiment_id=item.get("experiment_id"),
                run_id=item.get("run_id"),
                session_ref=session_ref,
                source=source,
                created_at=datetime.now(),
            )
        )
        inserted += 1
    ctx.db.commit()

    return {
        "report_finding": {
            "inserted": inserted,
            "skipped": skipped,
        }
    }


__all__ = ["handle"]
