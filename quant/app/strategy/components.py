"""StrategySpec 受控操作符的确定性 pandas 实现。"""
from __future__ import annotations

import math
from collections.abc import Mapping
from typing import Any

import numpy as np
import pandas as pd

from .spec import Expression

COMPONENT_VERSION = "strategy-components-v1"
COMPONENT_VERSIONS = {
    op: COMPONENT_VERSION for op in (
        "field", "literal", "all", "any", "not",
        "gt", "gte", "lt", "lte", "cross_above", "cross_below",
        "add", "subtract", "multiply", "divide",
        "rolling_mean", "rolling_max", "rolling_min", "shift",
        "ma", "rsi", "atr", "momentum", "return", "volume_ratio",
        "rank", "top_n",
    )
}

Vector = pd.Series | pd.DataFrame


def evaluate_expression(expr: Expression, fields: Mapping[str, Vector]) -> Any:
    """计算一个已校验表达式；不解析字符串，也不调用任何外部资源。"""
    op = expr.op
    if op == "field":
        assert expr.name is not None
        try:
            return fields[expr.name]
        except KeyError as exc:
            raise ValueError(f"输入数据缺少字段 {expr.name}") from exc
    if op == "literal":
        return expr.value
    if op in {"all", "any"}:
        values = [evaluate_expression(item, fields) for item in expr.args or []]
        result = values[0]
        for value in values[1:]:
            result = result & value if op == "all" else result | value
        return result
    if op == "not":
        assert expr.arg is not None
        value = evaluate_expression(expr.arg, fields)
        return not value if isinstance(value, bool) else ~value

    if op in {
        "gt", "gte", "lt", "lte", "cross_above", "cross_below",
        "add", "subtract", "multiply", "divide",
    }:
        assert expr.left is not None and expr.right is not None
        left = evaluate_expression(expr.left, fields)
        right = evaluate_expression(expr.right, fields)
        if op == "gt":
            return left > right
        if op == "gte":
            return left >= right
        if op == "lt":
            return left < right
        if op == "lte":
            return left <= right
        if op == "cross_above":
            return (left > right) & (_previous(left) <= _previous(right))
        if op == "cross_below":
            return (left < right) & (_previous(left) >= _previous(right))
        if op == "add":
            return left + right
        if op == "subtract":
            return left - right
        if op == "multiply":
            return left * right
        denominator = right.where(right != 0) if isinstance(right, (pd.Series, pd.DataFrame)) else right
        if not isinstance(denominator, (pd.Series, pd.DataFrame)) and denominator == 0:
            return math.nan
        return left / denominator

    if op == "shift":
        assert expr.input is not None and expr.periods is not None
        return _shift(evaluate_expression(expr.input, fields), expr.periods)

    if op in {"rolling_mean", "rolling_max", "rolling_min", "volume_ratio"}:
        assert expr.input is not None and expr.window is not None and expr.shift is not None
        value = evaluate_expression(expr.input, fields)
        history = _shift(value, expr.shift)
        rolling = history.rolling(expr.window)
        if op == "rolling_mean":
            return rolling.mean()
        if op == "rolling_max":
            return rolling.max()
        if op == "rolling_min":
            return rolling.min()
        denominator = rolling.mean()
        return value / denominator.where(denominator > 0)

    if op in {"ma", "rsi", "momentum", "return"}:
        assert expr.input is not None and expr.window is not None
        value = evaluate_expression(expr.input, fields)
        if op == "ma":
            return value.rolling(expr.window).mean()
        if op == "rsi":
            delta = value.diff()
            gain = delta.clip(lower=0)
            loss = -delta.clip(upper=0)
            avg_gain = gain.ewm(alpha=1 / expr.window, adjust=False).mean()
            avg_loss = loss.ewm(alpha=1 / expr.window, adjust=False).mean()
            rs = avg_gain / avg_loss
            return 100 - 100 / (1 + rs)
        return value / value.shift(expr.window) - 1

    if op == "atr":
        assert (
            expr.high is not None and expr.low is not None
            and expr.close is not None and expr.window is not None
        )
        high = evaluate_expression(expr.high, fields)
        low = evaluate_expression(expr.low, fields)
        close = evaluate_expression(expr.close, fields)
        previous = close.shift(1)
        true_range = _elementwise_nanmax(
            high - low,
            (high - previous).abs(),
            (low - previous).abs(),
        )
        return true_range.rolling(expr.window).mean()

    if op == "rank":
        assert expr.input is not None and expr.ascending is not None
        value = evaluate_expression(expr.input, fields)
        if not isinstance(value, pd.DataFrame):
            raise ValueError("rank 只能用于组合横截面")
        return value.rank(axis=1, ascending=expr.ascending, method="first")

    if op == "top_n":
        assert expr.input is not None and expr.n is not None
        value = evaluate_expression(expr.input, fields)
        if not isinstance(value, pd.DataFrame):
            raise ValueError("top_n 只能用于组合横截面")
        return value.rank(axis=1, ascending=False, method="first") <= expr.n

    # Expression 已在 Pydantic 层限制操作符，这一分支只保护手工构造的异常对象。
    raise ValueError(f"不支持的操作符 {op}")


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


def _shift(value: Any, periods: int) -> Any:
    if not isinstance(value, (pd.Series, pd.DataFrame)):
        raise ValueError("shift/cross 操作符不能作用于字面量")
    return value.shift(periods)


def _previous(value: Any) -> Any:
    return value.shift(1) if isinstance(value, (pd.Series, pd.DataFrame)) else value


def _elementwise_nanmax(first: Vector, second: Vector, third: Vector) -> Vector:
    values = np.stack([
        first.to_numpy(dtype=float),
        second.to_numpy(dtype=float),
        third.to_numpy(dtype=float),
    ])
    with np.errstate(all="ignore"):
        result = np.nanmax(values, axis=0)
    if isinstance(first, pd.DataFrame):
        return pd.DataFrame(result, index=first.index, columns=first.columns)
    return pd.Series(result, index=first.index)


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
    "COMPONENT_VERSION", "COMPONENT_VERSIONS", "build_reason_tree",
    "evaluate_expression", "used_component_versions",
]
