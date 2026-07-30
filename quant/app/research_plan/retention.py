"""研究计划保留策略:限制 quant_research_plan 无界增长。

按 (strategy_id, plan_type, code, pool_id) 分链,每链保留最近
KEEP_LATEST_PER_CHAIN 版;超出部分若仍被 quant_signal.plan_id 引用则保留,
否则删除。
"""
from __future__ import annotations

import logging
from collections import defaultdict
from typing import TYPE_CHECKING

from sqlalchemy import delete, select

from ..models import ResearchPlan, ResearchPlanItem, Signal

if TYPE_CHECKING:
    from sqlalchemy.orm import Session

logger = logging.getLogger(__name__)

KEEP_LATEST_PER_CHAIN = 30


def prune_research_plans(
    db: Session, keep_latest: int = KEEP_LATEST_PER_CHAIN,
) -> dict:
    """每链按 id 倒序保留前 keep_latest 版;其余未被信号引用则删除。

    全程使用调用方传入的 Session 事务;异常由调用方 rollback。
    """
    rows = db.execute(
        select(
            ResearchPlan.id,
            ResearchPlan.strategy_id,
            ResearchPlan.plan_type,
            ResearchPlan.code,
            ResearchPlan.pool_id,
        ).order_by(ResearchPlan.id.desc()),
    ).all()

    chains: dict[tuple[int, str, str | None, int | None], list[int]] = defaultdict(list)
    for row in rows:
        key = (row.strategy_id, row.plan_type, row.code, row.pool_id)
        chains[key].append(row.id)

    protected = set(
        db.execute(
            select(Signal.plan_id).where(Signal.plan_id.isnot(None)),
        ).scalars().all()
    )

    candidates: list[int] = []
    to_delete: list[int] = []
    protected_kept = 0
    for plan_ids in chains.values():
        discard = plan_ids[keep_latest:]
        for plan_id in discard:
            candidates.append(plan_id)
            if plan_id in protected:
                protected_kept += 1
            else:
                to_delete.append(plan_id)

    deleted = len(to_delete)
    if to_delete:
        for i in range(0, len(to_delete), 500):
            batch = to_delete[i:i + 500]
            db.execute(
                delete(ResearchPlanItem).where(
                    ResearchPlanItem.plan_id.in_(batch),
                ),
            )
        db.execute(
            delete(ResearchPlan).where(ResearchPlan.id.in_(to_delete)),
        )

    result = {
        "chains": len(chains),
        "candidates": len(candidates),
        "deleted": deleted,
        "protected_kept": protected_kept,
    }
    logger.info("研究计划保留策略完成: %s", result)
    return result
