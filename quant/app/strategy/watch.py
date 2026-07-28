"""通用「临近触发」(watch) 判定。

旧实现依赖模板私有的 watch() 函数,在动态策略框架改造中被一并删除,导致
watch 信号静默消失。这里用规格化的入场表达式重新实现:遍历
``entry.condition`` AST 中的所有比较节点,计算「距离触发的归一化间距」
并按逻辑结构聚合,判断空仓状态下条件是否已接近触发。

间距约定(所有比较算子统一):

- ``gap > 0``:该比较尚未成立,值越小越接近触发;
- ``gap <= 0``:该比较已经成立(或已越过);
- ``gap`` 为 ``None``:当日数据不足或分支保守不可评估,不视为临近。

聚合规则:

- ``all`` (AND):取各子条件 gap 的**最大值** —— 最远的条件都被拉近到容差内,
  才算整体临近(AND 要所有条件都接近);
- ``any`` (OR):取各子条件 gap 的**最小值** —— 任一可评估条件接近即可;
- ``not``:保守处理 —— 永远不会「临近」,只按当前布尔值参与聚合:成立时按
  已满足(不阻挡 AND、不成就 OR),不成立时按不可评估处理。
"""
from __future__ import annotations

import math
from typing import Any

import pandas as pd

from .components import evaluate_expression
from .spec import Expression

# 临近触发的归一化间距容差:入场条件中最具约束的比较与触发线的相对距离
# 不超过 2% 时,认为策略「临近触发」,产出 watch 信号供人工继续观察。
WATCH_TOLERANCE = 0.02

_EPS = 1e-12

_COMPARISON_OPS = frozenset({
    "gt", "gte", "lt", "lte", "cross_above", "cross_below",
})

_OP_SYMBOL = {
    "gt": ">", "gte": ">=", "lt": "<", "lte": "<=",
    "cross_above": "上穿", "cross_below": "下穿",
}


def assess_entry_watch(
    condition: Expression,
    df: pd.DataFrame,
    *,
    tolerance: float = WATCH_TOLERANCE,
) -> dict[str, Any]:
    """评估单标的入场条件在最后一根 bar 的触发状态与临近程度。

    返回字典包含:
      triggered: 入场条件当日是否已经成立(成立时不应再产 watch);
      near:      未触发且聚合间距在容差内;
      gap:       聚合归一化间距(无法评估时为 None);
      binding:   最具约束的比较节点的可读明细(op/两侧标签/两侧取值/gap);
      summary:   面向用户的中文说明。
    """
    fields = {
        column: pd.Series(df[column].to_numpy(), index=df.index)
        for column in df.columns
        if column != "date"
    }
    value = evaluate_expression(condition, fields)
    triggered = _bool_at_last(value)
    gap, binding = _node_gap(condition, fields)
    near = (
        not triggered
        and gap is not None
        and math.isfinite(gap)
        and gap <= tolerance
    )
    finite_gap = gap if gap is not None and math.isfinite(gap) else None
    return {
        "triggered": triggered,
        "near": near,
        "gap": round(finite_gap, 6) if finite_gap is not None else None,
        "tolerance": tolerance,
        "binding": binding,
        "summary": _summary(triggered, near, finite_gap, binding, tolerance),
    }


def _bool_at_last(value: Any) -> bool:
    if isinstance(value, pd.Series):
        if value.empty:
            return False
        last = value.iat[-1]
        return False if pd.isna(last) else bool(last)
    return bool(value)


def _node_gap(
    expr: Expression,
    fields: dict[str, pd.Series],
) -> tuple[float | None, dict[str, Any] | None]:
    """递归计算表达式节点的归一化间距与最具约束的比较明细。"""
    op = expr.op
    if op in _COMPARISON_OPS:
        return _comparison_gap(expr, fields)
    if op in {"all", "any"}:
        children = [
            _node_gap(item, fields) for item in expr.args or []
        ]
        if op == "all":
            # AND:任一子条件无法评估,整体保守视为不临近
            if any(gap is None for gap, _ in children):
                return None, None
            gaps = [gap for gap, _ in children]
            index = gaps.index(max(gaps))
            return gaps[index], children[index][1]
        # OR:忽略不可评估的分支,任一可评估分支接近即可
        assessed = [(gap, detail) for gap, detail in children if gap is not None]
        if not assessed:
            return None, None
        return min(assessed, key=lambda item: item[0])
    if op == "not":
        # not 分支保守处理:不提供距离;成立按已满足,不成立按不可评估
        assert expr.arg is not None
        value = evaluate_expression(expr, fields)
        return (-math.inf if _bool_at_last(value) else None), None
    if op == "literal":
        return (-math.inf if expr.value else None), None
    raise ValueError(f"watch 判定不支持的布尔节点: {op}")


def _comparison_gap(
    expr: Expression,
    fields: dict[str, pd.Series],
) -> tuple[float | None, dict[str, Any] | None]:
    """单个比较节点的归一化间距。

    gt/gte 取 (right - left) / max(|right|, eps),lt/lte 取相反数,使
    gap > 0 恒表示「尚未成立,越小越接近」。cross_above/cross_below 是
    事件型节点:只有两侧差距在容差内且尚未交叉(当日与前一日的左右关系
    都还未越过)才算临近。
    """
    assert expr.left is not None and expr.right is not None
    left = evaluate_expression(expr.left, fields)
    right = evaluate_expression(expr.right, fields)
    left_now, left_prev = _last_two(left)
    right_now, right_prev = _last_two(right)
    detail = {
        "op": expr.op,
        "left": _expr_label(expr.left),
        "right": _expr_label(expr.right),
        "left_value": _round(left_now),
        "right_value": _round(right_now),
    }
    if left_now is None or right_now is None:
        return None, None
    scale = max(abs(right_now), _EPS)
    if expr.op in {"gt", "gte"}:
        gap = (right_now - left_now) / scale
    elif expr.op in {"lt", "lte"}:
        gap = (left_now - right_now) / scale
    elif expr.op == "cross_above":
        if left_prev is None or right_prev is None:
            return None, None
        if left_now > right_now or left_prev > right_prev:
            # 已交叉(事件已过)或从未在下方,不是「即将上穿」
            return None, None
        gap = (right_now - left_now) / scale
    else:  # cross_below
        if left_prev is None or right_prev is None:
            return None, None
        if left_now < right_now or left_prev < right_prev:
            return None, None
        gap = (left_now - right_now) / scale
    detail["gap"] = round(gap, 6)
    return gap, detail


def _last_two(value: Any) -> tuple[float | None, float | None]:
    """序列的最后两个取值(不足两个时前值为 None);常量前值等于当期值。"""
    if isinstance(value, pd.Series):
        now = _finite(value.iat[-1]) if len(value) >= 1 else None
        prev = _finite(value.iat[-2]) if len(value) >= 2 else None
        return now, prev
    constant = _finite(value)
    return constant, constant


def _finite(value: Any) -> float | None:
    if value is None or isinstance(value, bool):
        return None
    try:
        if pd.isna(value):
            return None
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def _round(value: float | None) -> float | None:
    return round(value, 6) if value is not None else None


def _expr_label(expr: Expression | None) -> str:
    """比较一侧的紧凑标签,用于给人看的临近说明。"""
    if expr is None:
        return "?"
    if expr.op == "field":
        return expr.name or "field"
    if expr.op == "literal":
        return f"{expr.value:g}"
    if expr.window is not None:
        suffix = f"{expr.window}"
        if expr.shift:
            suffix += f",shift{expr.shift}"
        return f"{expr.op}({suffix})"
    return expr.op


def _summary(
    triggered: bool,
    near: bool,
    gap: float | None,
    binding: dict[str, Any] | None,
    tolerance: float,
) -> str:
    if triggered:
        return "入场条件当日已成立,等待仓位跳变产生入场信号。"
    if near and binding is not None and gap is not None:
        return (
            f"临近触发:{binding['left']} {_OP_SYMBOL.get(binding['op'], binding['op'])}"
            f" {binding['right']} 尚未成立(当前 {binding['left_value']:g} vs"
            f" {binding['right_value']:g}),归一化间距 {gap:.2%} 在容差"
            f" {tolerance:.0%} 内。"
        )
    if gap is None:
        return "入场条件未成立,且部分比较数据不足或不可评估,暂不属于临近触发。"
    return f"入场条件未成立,归一化间距 {gap:.2%} 超出临近容差 {tolerance:.0%}。"


__all__ = ["WATCH_TOLERANCE", "assess_entry_watch"]
