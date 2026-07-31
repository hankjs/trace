"""backtest.run skill。"""
from __future__ import annotations

from datetime import date, datetime
from typing import Any

from ...api.backtest import _prepare_backtest
from ...api.strategies import get_strategy_or_404
from ...backtest.engine import DEFAULT_COSTS
from ...backtest.jobs import execute_backtest_run, pending_payload
from ...backtest.listing import _run_summary
from ...backtest.validation import validate_backtest_window
from ...models import BacktestRun
from ...strategy.compiler import COMPILER_VERSION, component_versions_for_spec
from ...strategy.evidence import advance_after_backtest
from ...strategy.spec import strategy_spec_hash
from ...tasks import submit_task, TaskConflictError
from ._common import A2AContext, wait_for_task


_ALLOWED_KEYS = {
    "strategy_id", "start", "end", "codes", "pool_id", "costs",
    "confirmed", "client_request_id",
}


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """对已保存 strategy_id 发起回测，映射到 quant_task。"""
    unknown = set(payload) - _ALLOWED_KEYS
    if unknown:
        raise ValueError(
            f"backtest.run 不支持顶层字段: {', '.join(sorted(unknown))}。"
            "请只使用 strategy_id/start/end/codes/pool_id/costs，"
            "并通过 strategy.save_draft 固化参数。"
        )
    if "strategy_id" not in payload:
        raise ValueError("backtest.run 必须提供 strategy_id；请先调用 strategy.save_draft")

    # 禁止 historical 非法费用字段
    for bad in ("initial_cash", "fees", "slippage_bps", "params"):
        if bad in payload:
            raise ValueError(
                f"backtest.run 不支持 {bad}；费用请使用 costs.commission/costs.stamp_tax/costs.slippage"
            )

    from pydantic import BaseModel, Field

    class _BacktestIn(BaseModel):
        strategy_id: int
        codes: list[str] = Field(default_factory=list)
        start: date
        end: date
        pool_id: int | None = None
        costs: dict = Field(default_factory=dict)
        # _prepare_backtest 会读 body.params(现网 BacktestIn 的兼容字段);
        # A2A 契约禁止 payload 带 params(上方已拦),这里恒为 None 走冻结 spec 路径。
        params: dict | None = None

        def model_post_init(self, __context):
            validate_backtest_window(self.start, self.end)

    body = _BacktestIn(**payload)

    strategy, execution_spec, codes, use_pool, pool = _prepare_backtest(
        body, ctx.db, ctx.user_id,
    )
    pool_id = pool.id if use_pool and pool is not None else None

    versions = component_versions_for_spec(execution_spec)
    run = BacktestRun(
        user_id=ctx.user_id,
        strategy_id=strategy.id,
        params={},
        costs=body.costs or {},
        pool_id=pool_id,
        codes=codes,
        start=body.start,
        end=body.end,
        metrics=None,
        strategy_spec_snapshot=execution_spec.model_dump(mode="json"),
        strategy_spec_hash=strategy_spec_hash(execution_spec),
        compiler_version=COMPILER_VERSION,
        component_versions=dict(sorted(versions.items())),
        status="pending",
        request_snapshot={
            "codes": codes,
            "params": {},
            "costs": body.costs or {},
            "dynamic_universe": use_pool,
            "pool_id": pool_id,
            "client_request_id": payload.get("client_request_id"),
        },
        created_at=datetime.now(),
    )
    ctx.db.add(run)
    ctx.db.commit()
    ctx.db.refresh(run)

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="backtest",
            title=f"回测 · {strategy.name} · {body.start}~{body.end}",
            params={
                "run_id": run.id,
                "client_request_id": payload.get("client_request_id"),
            },
            ref_id=run.id,
        )
    except TaskConflictError as exc:
        run.status = "cancelled"
        run.error = "已有进行中的任务，可等待完成；若仍为排队中可先 Cancel"
        run.finished_at = datetime.now()
        ctx.db.commit()
        raise ValueError(str(exc)) from exc

    wait_for_task(ctx.db, task, cancel_event)
    ctx.db.refresh(run)

    if run.status == "cancelled":
        raise ValueError("任务已取消")
    if run.status != "done":
        raise ValueError(run.error or "回测执行失败")

    try:
        advance_after_backtest(ctx.db, strategy, {"metrics": run.metrics, "validation": (run.metrics or {}).get("validation")})
        ctx.db.commit()
    except Exception:  # noqa: BLE001
        ctx.db.rollback()

    summary = {"backtest_summary": _run_summary(run)}
    # backtest 任务类型的 execute_backtest_run 不写 task.result(返回 None);
    # 幂等重放的 artifact 取自 task.result(见 server.py 幂等分支),
    # 这里把 A2A 摘要补写进去。REST 任务 API 的 result 原本为 null,属纯增量。
    task.result = summary
    ctx.db.commit()
    return summary


__all__ = ["handle"]
