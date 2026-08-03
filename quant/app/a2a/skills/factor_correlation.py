"""factor.correlation skill:待检因子相对对照集的相关性与正交性。"""
from __future__ import annotations

from datetime import date
from typing import Any

from ...factors.correlation import (
    MAX_BENCHMARKS,
    build_correlation_artifact,
    compute_factor_correlation,
)
from ...factors.evaluation import normalize_neutralize
from ...tasks import TaskConflictError, register_handler, submit_task
from ._common import A2AContext, wait_for_task


def _factor_correlation_handler(db, task, *, cancel_event=None):
    """quant_task handler：执行相关性检验。"""
    p = task.params or {}
    codes = p.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]
    row = compute_factor_correlation(
        db,
        user_id=task.user_id,
        expression=p.get("expression"),
        factor_key=p.get("factor_key"),
        benchmark_keys=p.get("benchmark_keys"),
        start=date.fromisoformat(p["start"]),
        end=date.fromisoformat(p["end"]),
        pool_id=p.get("pool_id"),
        codes=codes,
        rebalance=p["rebalance"],
        neutralize=p.get("neutralize"),
        cancel_event=cancel_event,
    )
    return build_correlation_artifact(row)


register_handler(
    "factor_correlation", _factor_correlation_handler, supports_cancel=True,
)


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """提交 factor_correlation 类型 quant_task 并等待完成。"""
    expression = payload.get("expression")
    factor_key = payload.get("factor_key")
    if (expression is None) == (factor_key is None):
        raise ValueError("必须且只能提供 expression 或 factor_key 之一")

    start = date.fromisoformat(str(payload["start"]))
    end = date.fromisoformat(str(payload["end"]))
    rebalance = str(payload.get("rebalance") or "weekly")
    if rebalance not in {"weekly", "monthly"}:
        raise ValueError("rebalance 只支持 weekly 或 monthly")

    # 提交前校验:非法值立即报错,不要等 worker
    neutralize = normalize_neutralize(payload.get("neutralize"))
    benchmark_keys = payload.get("benchmark_keys")
    if benchmark_keys is not None:
        if not isinstance(benchmark_keys, list):
            raise ValueError("benchmark_keys 必须是字符串数组")
        if len(benchmark_keys) > MAX_BENCHMARKS:
            raise ValueError(
                f"对照因子最多 {MAX_BENCHMARKS} 个,收到 {len(benchmark_keys)} 个"
            )

    codes = payload.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="factor_correlation",
            title=f"factor correlation · {factor_key or 'expression'}",
            params={
                "expression": expression,
                "factor_key": factor_key,
                "benchmark_keys": benchmark_keys,
                "start": start.isoformat(),
                "end": end.isoformat(),
                "pool_id": payload.get("pool_id"),
                "codes": codes,
                "rebalance": rebalance,
                "neutralize": neutralize,
                "client_request_id": payload.get("client_request_id"),
            },
        )
    except TaskConflictError as exc:
        raise ValueError(str(exc)) from exc

    wait_for_task(ctx.db, task, cancel_event)
    if task.status != "done":
        err = task.error or "因子相关性检验执行失败"
        if "对照" in err or "样本" in err:
            err = f"{err}；可缩小区间或减少对照因子数后重试"
        raise ValueError(err)
    return task.result or {}


__all__ = ["handle"]
