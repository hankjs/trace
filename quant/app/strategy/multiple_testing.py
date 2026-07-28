"""多重检验的最小可用提示(非完整 Reality Check)。

在参数扫描/实验 trial 上报告试验次数与 Bonferroni 校正提示,
明确标注「未校正前的探索结果」,避免把最优夏普误读为已证实 Alpha。
"""
from __future__ import annotations

import math
from typing import Any


def multiplicity_report(
    rows: list[dict[str, Any]],
    *,
    metric_key: str = "annual_return_median",
    alpha: float = 0.05,
) -> dict[str, Any]:
    """从扫描/trial 行构造多重检验提示。

    rows 元素期望含 metrics 字典;metric_key 默认年化中位数(与 sweep 对齐)。
    """
    n = len(rows)
    values: list[tuple[float, dict[str, Any]]] = []
    for row in rows:
        metrics = row.get("metrics") or {}
        value = metrics.get(metric_key)
        if value is None:
            value = metrics.get("annual_return")
        if value is None:
            value = metrics.get("sharpe")
        if isinstance(value, (int, float)) and math.isfinite(float(value)):
            values.append((float(value), row))

    if not values:
        return {
            "n_trials": n,
            "n_evaluable": 0,
            "alpha": alpha,
            "bonferroni_alpha": None,
            "best_metric": None,
            "best_params": None,
            "disclaimer": (
                "探索性扫描结果,未做正式多重检验校正;"
                "试验数不足或指标不可用。"
            ),
        }

    best_value, best_row = max(values, key=lambda item: item[0])
    n_eval = len(values)
    bonferroni = alpha / n_eval if n_eval else None
    return {
        "n_trials": n,
        "n_evaluable": n_eval,
        "alpha": alpha,
        "bonferroni_alpha": None if bonferroni is None else round(bonferroni, 6),
        "best_metric": round(best_value, 4),
        "best_params": dict(best_row.get("params") or {}),
        "disclaimer": (
            f"共 {n} 组试验(可评估 {n_eval} 组)。"
            f"若按 Bonferroni 粗校正,名义显著性水平 {alpha} 对应约 "
            f"{bonferroni:.4f} 的单次阈值。"
            "这是未校正前的探索结果,不能当作策略已通过严格 Reality Check。"
        ),
    }


__all__ = ["multiplicity_report"]
