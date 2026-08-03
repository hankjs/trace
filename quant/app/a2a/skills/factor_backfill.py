"""factor.backfill skill:把因子草稿的历史值算进 quant_factor_daily。"""
from __future__ import annotations

from datetime import date
from typing import Any

from sqlalchemy import select

from ...backtest.validation import validate_backtest_window
from ...factors.defs import can_write_factor
from ...models import FactorDef
from ...tasks import TaskConflictError, submit_task
from ._common import A2AContext, wait_for_task


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """提交 factor_backfill 类型 quant_task 并等待完成。

    只允许回填自己的 disabled 草稿:回填是全市场逐日写库,放开给任意因子
    等于让普通用户覆盖系统因子的历史值。
    """
    factor_key = payload.get("factor_key")
    if not factor_key:
        raise ValueError("factor_key 必填;回填全部启用因子仅管理员可用,请指定单个因子")

    is_admin = bool(ctx.claims.get("can_admin"))
    def_ = ctx.db.execute(
        select(FactorDef).where(FactorDef.key == str(factor_key))
    ).scalar_one_or_none()
    if def_ is None:
        raise ValueError(f"因子 {factor_key} 不存在")
    if not can_write_factor(def_, user_id=ctx.user_id, is_admin=is_admin):
        raise ValueError(f"无权回填因子 {factor_key}:只能回填自己保存的草稿")
    if def_.enabled and not is_admin:
        raise ValueError(
            f"因子 {factor_key} 已启用,回填会改动夜间管道读取的因子值,仅管理员可操作"
        )

    start = date.fromisoformat(str(payload["start"]))
    end = date.fromisoformat(str(payload["end"]))
    validate_backtest_window(start, end)

    codes = payload.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="factor_backfill",
            title=f"factor backfill · {factor_key}",
            params={
                "factor_key": str(factor_key),
                "start": start.isoformat(),
                "end": end.isoformat(),
                "codes": codes,
                "owner_id": ctx.user_id,
                "is_admin": is_admin,
                "client_request_id": payload.get("client_request_id"),
            },
        )
    except TaskConflictError as exc:
        raise ValueError(str(exc)) from exc

    wait_for_task(ctx.db, task, cancel_event)
    if task.status != "done":
        raise ValueError(task.error or "因子回填执行失败")

    result = task.result or {}
    return {
        "factor_backfill": {
            "factor_key": str(factor_key),
            "window": {"start": start.isoformat(), "end": end.isoformat()},
            "days": result.get("days", 0),
            "rows_written": result.get("rows_written", 0),
            "skipped": result.get("skipped", 0),
            "detail_ref": {"task_id": task.id},
        }
    }


__all__ = ["handle"]
