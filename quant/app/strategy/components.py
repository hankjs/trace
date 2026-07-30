"""StrategySpec 受控操作符的确定性 pandas 实现。"""
from __future__ import annotations

from collections.abc import Mapping
from typing import Any

import numpy as np
import pandas as pd

from .operators import OPERATORS, ROLLING_STD_DDOF
from .spec import Expression

COMPONENT_VERSION = "strategy-components-v1"
# rolling_std 固定 ddof=0(总体标准差,与波动率实务一致);zscore 复用同口径。
COMPONENT_VERSIONS = {op: spec.version for op, spec in OPERATORS.items()}

Vector = pd.Series | pd.DataFrame


def evaluate_expression(expr: Expression, fields: Mapping[str, Vector]) -> Any:
    """计算一个已校验表达式；不解析字符串，也不调用任何外部资源。"""
    spec = OPERATORS.get(expr.op)
    if spec is None:
        raise ValueError(f"不支持的操作符 {expr.op}")
    return spec.evaluate(expr, fields, evaluate_expression)


def build_reason_tree(
    expr: Expression,
    fields: Mapping[str, Vector],
    position: int,
    *,
    column: str | None = None,
) -> dict[str, Any]:
    """返回某日、某标的的表达式贡献树，供状态变化证据使用。"""
    value = evaluate_expression(expr, fields)
    item: dict[str, Any] = {
        "op": expr.op,
        "value": _json_value(_value_at(value, position, column)),
    }
    if expr.name is not None:
        item["field"] = expr.name
    if expr.op == "literal":
        item["literal"] = expr.value
    for key in ("window", "shift", "periods", "ascending", "n"):
        value = getattr(expr, key)
        if value is not None:
            item[key] = value
    children = [
        child for child in (
            expr.arg, expr.left, expr.right, expr.input, expr.high, expr.low, expr.close,
        ) if child is not None
    ] + list(expr.args or [])
    if children:
        item["children"] = [
            build_reason_tree(child, fields, position, column=column)
            for child in children
        ]
    return item


def used_component_versions(expr: Expression) -> dict[str, str]:
    versions: dict[str, str] = {}

    def visit(node: Expression) -> None:
        versions[node.op] = COMPONENT_VERSIONS[node.op]
        for child in (
            node.arg, node.left, node.right, node.input, node.high, node.low, node.close,
        ):
            if child is not None:
                visit(child)
        for child in node.args or []:
            visit(child)

    visit(expr)
    return dict(sorted(versions.items()))


def _value_at(value: Any, position: int, column: str | None) -> Any:
    if isinstance(value, pd.DataFrame):
        if column is None:
            raise ValueError("DataFrame 原因树必须指定 column")
        return value.iloc[position][column]
    if isinstance(value, pd.Series):
        return value.iloc[position]
    return value


def _json_value(value: Any) -> Any:
    if value is None or pd.isna(value):
        return None
    if isinstance(value, (np.bool_, bool)):
        return bool(value)
    if isinstance(value, (np.integer, int)):
        return int(value)
    if isinstance(value, (np.floating, float)):
        return round(float(value), 12)
    return value


__all__ = [
    "COMPONENT_VERSION", "COMPONENT_VERSIONS", "ROLLING_STD_DDOF",
    "build_reason_tree", "evaluate_expression", "used_component_versions",
]
