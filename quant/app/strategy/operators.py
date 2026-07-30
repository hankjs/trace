"""StrategySpec 受控操作符注册表与元数据。

所有操作符的形状、类型、求值语义与最小数据窗长集中在此定义。
components.py 通过 OPERATORS 注册表调用求值逻辑,避免 if/elif 链扩散。
"""
from __future__ import annotations

import math
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import numpy as np
import pandas as pd

if TYPE_CHECKING:
    from .spec import Expression

COMPONENT_VERSION = "strategy-components-v1"
# rolling_std 固定 ddof=0(总体标准差,与波动率实务一致);zscore 复用同口径。
ROLLING_STD_DDOF = 0

Vector = pd.Series | pd.DataFrame


@dataclass(frozen=True)
class OperatorSpec:
    op: str
    fields: frozenset[str]          # exact allowed JSON keys incl "op"
    arg_types: dict[str, str]       # expression slot -> "number"|"bool"
    result_type: str                # "number" | "bool" | "literal"
    evaluate: Callable              # (expr: Expression, fields: Mapping, recurse: Callable) -> Any
    min_window: Callable            # (expr: Expression, child_windows: list[int]) -> int
    version: str = COMPONENT_VERSION


def _shift(value: Any, periods: int) -> Any:
    if not isinstance(value, (pd.Series, pd.DataFrame)):
        raise ValueError("shift/cross 操作符不能作用于字面量")
    return value.shift(periods)


def _previous(value: Any) -> Any:
    return value.shift(1) if isinstance(value, (pd.Series, pd.DataFrame)) else value


def _window_percentile_rank(arr: np.ndarray) -> float:
    """滚动窗口末值的百分位: (count of values <= last) / n, 输出 (0,1]。"""
    if arr is None or len(arr) == 0:
        return math.nan
    last = arr[-1]
    if last != last:  # NaN
        return math.nan
    valid = arr[~np.isnan(arr)]
    if len(valid) == 0:
        return math.nan
    return float(np.sum(valid <= last) / len(valid))


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


def _eval_field(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.name is not None
    try:
        return fields[expr.name]
    except KeyError as exc:
        raise ValueError(f"输入数据缺少字段 {expr.name}") from exc


def _eval_literal(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    return expr.value


def _eval_all(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    values = [recurse(item, fields) for item in expr.args or []]
    result = values[0]
    for value in values[1:]:
        result = result & value
    return result


def _eval_any(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    values = [recurse(item, fields) for item in expr.args or []]
    result = values[0]
    for value in values[1:]:
        result = result | value
    return result


def _eval_not(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.arg is not None
    value = recurse(expr.arg, fields)
    return not value if isinstance(value, bool) else ~value


def _eval_comparison_or_arithmetic(
    expr: "Expression", fields: Mapping[str, Vector], recurse: Callable,
) -> Any:
    assert expr.left is not None and expr.right is not None
    left = recurse(expr.left, fields)
    right = recurse(expr.right, fields)
    op = expr.op
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


def _eval_shift(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.input is not None and expr.periods is not None
    return _shift(recurse(expr.input, fields), expr.periods)


def _eval_rolling_family(
    expr: "Expression", fields: Mapping[str, Vector], recurse: Callable,
) -> Any:
    assert expr.input is not None and expr.window is not None and expr.shift is not None
    value = recurse(expr.input, fields)
    history = _shift(value, expr.shift)
    rolling = history.rolling(expr.window)
    op = expr.op
    if op == "rolling_mean":
        return rolling.mean()
    if op == "rolling_max":
        return rolling.max()
    if op == "rolling_min":
        return rolling.min()
    if op == "rolling_std":
        return rolling.std(ddof=ROLLING_STD_DDOF)
    if op == "rolling_rank":
        # 窗口末值在窗内的百分位排名 ∈ (0,1];不足 window 根为 NaN
        return history.rolling(expr.window).apply(
            _window_percentile_rank, raw=True,
        )
    if op == "zscore":
        mean = rolling.mean()
        std = rolling.std(ddof=ROLLING_STD_DDOF)
        return (history - mean) / std.where(std != 0)
    denominator = rolling.mean()
    return value / denominator.where(denominator > 0)


def _eval_ma_family(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.input is not None and expr.window is not None
    value = recurse(expr.input, fields)
    op = expr.op
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


def _eval_atr(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert (
        expr.high is not None and expr.low is not None
        and expr.close is not None and expr.window is not None
    )
    high = recurse(expr.high, fields)
    low = recurse(expr.low, fields)
    close = recurse(expr.close, fields)
    previous = close.shift(1)
    true_range = _elementwise_nanmax(
        high - low,
        (high - previous).abs(),
        (low - previous).abs(),
    )
    return true_range.rolling(expr.window).mean()


def _eval_rank(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.input is not None and expr.ascending is not None
    value = recurse(expr.input, fields)
    if not isinstance(value, pd.DataFrame):
        raise ValueError("rank 只能用于组合横截面")
    return value.rank(axis=1, ascending=expr.ascending, method="first")


def _eval_top_n(expr: "Expression", fields: Mapping[str, Vector], recurse: Callable) -> Any:
    assert expr.input is not None and expr.n is not None
    value = recurse(expr.input, fields)
    if not isinstance(value, pd.DataFrame):
        raise ValueError("top_n 只能用于组合横截面")
    return value.rank(axis=1, ascending=False, method="first") <= expr.n


def _max_children(_expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) if child_windows else 0


def _child_plus_window(expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) + expr.window - 1


def _child_plus_window_for_momentum(expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) + expr.window


def _shift_min_window(expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) + expr.periods


def _rolling_family_min_window(expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) + expr.shift + expr.window - 1


def _atr_min_window(expr: "Expression", child_windows: list[int]) -> int:
    return max(child_windows) + expr.window


_OPERATORS: list[OperatorSpec] = [
    OperatorSpec(
        op="field", fields=frozenset({"op", "name"}),
        arg_types={}, result_type="number",
        evaluate=_eval_field,
        min_window=lambda _e, _cw: 1,
    ),
    OperatorSpec(
        op="literal", fields=frozenset({"op", "value"}),
        arg_types={}, result_type="literal",
        evaluate=_eval_literal,
        min_window=lambda _e, _cw: 0,
    ),
    OperatorSpec(
        op="all", fields=frozenset({"op", "args"}),
        arg_types={"args": "bool"}, result_type="bool",
        evaluate=_eval_all,
        min_window=_max_children,
    ),
    OperatorSpec(
        op="any", fields=frozenset({"op", "args"}),
        arg_types={"args": "bool"}, result_type="bool",
        evaluate=_eval_any,
        min_window=_max_children,
    ),
    OperatorSpec(
        op="not", fields=frozenset({"op", "arg"}),
        arg_types={"arg": "bool"}, result_type="bool",
        evaluate=_eval_not,
        min_window=_max_children,
    ),
    *[
        OperatorSpec(
            op=op, fields=frozenset({"op", "left", "right"}),
            arg_types={"left": "number", "right": "number"},
            result_type="bool" if op in {
                "gt", "gte", "lt", "lte", "cross_above", "cross_below",
            } else "number",
            evaluate=_eval_comparison_or_arithmetic,
            min_window=_max_children,
        )
        for op in (
            "gt", "gte", "lt", "lte", "cross_above", "cross_below",
            "add", "subtract", "multiply", "divide",
        )
    ],
    *[
        OperatorSpec(
            op=op, fields=frozenset({"op", "input", "window", "shift"}),
            arg_types={"input": "number"}, result_type="number",
            evaluate=_eval_rolling_family,
            min_window=_rolling_family_min_window,
        )
        for op in (
            "rolling_mean", "rolling_max", "rolling_min", "rolling_std",
            "rolling_rank", "zscore", "volume_ratio",
        )
    ],
    OperatorSpec(
        op="shift", fields=frozenset({"op", "input", "periods"}),
        arg_types={"input": "number"}, result_type="number",
        evaluate=_eval_shift,
        min_window=_shift_min_window,
    ),
    *[
        OperatorSpec(
            op=op, fields=frozenset({"op", "input", "window"}),
            arg_types={"input": "number"}, result_type="number",
            evaluate=_eval_ma_family,
            # momentum/return 需要完整 window 的 shift;rsi 按 ewm 预热窗口计
            # (与 quant_factor_def 种子 min_bars 口径一致),ma 首值只需 window-1。
            min_window=(
                _child_plus_window_for_momentum
                if op in {"momentum", "return", "rsi"} else _child_plus_window
            ),
        )
        for op in ("ma", "rsi", "momentum", "return")
    ],
    OperatorSpec(
        op="atr", fields=frozenset({"op", "high", "low", "close", "window"}),
        arg_types={"high": "number", "low": "number", "close": "number"},
        result_type="number",
        evaluate=_eval_atr,
        min_window=_atr_min_window,
    ),
    OperatorSpec(
        op="rank", fields=frozenset({"op", "input", "ascending"}),
        arg_types={"input": "number"}, result_type="number",
        evaluate=_eval_rank,
        min_window=_max_children,
    ),
    OperatorSpec(
        op="top_n", fields=frozenset({"op", "input", "n"}),
        arg_types={"input": "number"}, result_type="bool",
        evaluate=_eval_top_n,
        min_window=_max_children,
    ),
]

OPERATORS: dict[str, OperatorSpec] = {spec.op: spec for spec in _OPERATORS}


def compute_min_bars(expr: "Expression") -> int:
    """计算表达式产生首个非 NaN 值所需的最少 bar 数(含当前 bar,≥1)。"""
    def walk(node: "Expression") -> int:
        spec = OPERATORS.get(node.op)
        if spec is None:
            raise ValueError(f"不支持的操作符 {node.op}")
        child_windows: list[int] = []
        for slot in spec.arg_types:
            child = getattr(node, slot)
            if isinstance(child, list):
                child_windows.extend(walk(item) for item in child)
            elif child is not None:
                child_windows.append(walk(child))
        return spec.min_window(node, child_windows)

    return max(1, walk(expr))


__all__ = [
    "COMPONENT_VERSION", "OPERATORS", "OperatorSpec", "ROLLING_STD_DDOF",
    "Vector", "compute_min_bars",
]
