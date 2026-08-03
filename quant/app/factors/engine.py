"""动态因子引擎:把 DSL 表达式求值到日线序列。

所有计算都是向量化 pandas 操作,无未来函数。
"""
from __future__ import annotations

import math
from typing import Any

import numpy as np
import pandas as pd

from ..strategy.components import build_reason_tree, evaluate_expression
from ..strategy.operators import INDUSTRY_FIELD_KEY
from ..strategy.spec import Expression, parse_expression


# 从 DataFrame 构造字段映射,与 strategy.compiler._single_fields 等价
def bars_fields(df: pd.DataFrame) -> dict[str, pd.Series]:
    """把日线 DataFrame 转为表达式求值所需的 {字段名: Series} 映射。

    去掉 date 列,其它列全部暴露给表达式。
    """
    return {
        col: df[col]
        for col in df.columns
        if col != "date"
    }


def evaluate_factor(expr: Expression | dict[str, Any] | str,
                    df: pd.DataFrame) -> pd.Series:
    """计算因子表达式,返回与 df 等长的 Series(前导窗口不足为 NaN)。"""
    parsed = parse_expression(expr)
    fields = bars_fields(df)
    return evaluate_expression(parsed, fields)


def evaluate_factor_cross_section(
    expr: Expression | dict[str, Any] | str,
    pool_dfs: dict[str, pd.DataFrame],
    *,
    industries: dict[str, str] | None = None,
) -> pd.DataFrame:
    """在横截面上求值因子,返回 date×code 帧。

    复用组合策略侧既有的帧构造路径(compiler._portfolio_fields),不另建一套
    横截面引擎 —— 两套实现会在字段对齐与 reindex 语义上悄悄分叉。
    """
    from ..strategy.compiler import _portfolio_fields

    parsed = parse_expression(expr)
    if not pool_dfs:
        raise ValueError("pool_dfs 不能为空")
    # 统一日期轴:取全池日期并集并排序,与 compile_portfolio 的 index 同口径
    all_dates: list = []
    for frame in pool_dfs.values():
        if "date" in frame.columns:
            all_dates.extend(pd.DatetimeIndex(frame["date"]).tolist())
    if not all_dates:
        raise ValueError("pool_dfs 中没有可用日期")
    index = pd.DatetimeIndex(sorted(set(all_dates)))
    fields = _portfolio_fields(index, pool_dfs)
    if industries is not None:
        fields[INDUSTRY_FIELD_KEY] = pd.Series(industries)
    result = evaluate_expression(parsed, fields)
    if not isinstance(result, pd.DataFrame):
        raise ValueError("横截面求值必须返回 date×code 帧;该表达式可能是时序的")
    return result


def evaluate_def_last(def_: Any, df: pd.DataFrame) -> float | None:
    """对单个 FactorDef 求最后一个交易日的因子值。

    数据不足、结果 NaN 或无穷时返回 None;否则返回保留 6 位小数的 float。
    """
    series = evaluate_factor(def_.expression, df)
    if len(series) == 0:
        return None
    value = series.iloc[-1]
    if value is None or pd.isna(value):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return round(float(value), 6)


__all__ = [
    "bars_fields",
    "build_reason_tree",
    "evaluate_def_last",
    "evaluate_factor",
    "evaluate_factor_cross_section",
]
