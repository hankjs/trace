"""FW-3: rolling_std / rolling_rank / zscore 数值语义与无前视。"""
from __future__ import annotations

import sys
from copy import deepcopy
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.components import ROLLING_STD_DDOF, evaluate_expression
from app.strategy.presets import SYSTEM_STRATEGY_SPECS
from app.strategy.spec import (
    Expression,
    CapabilityStatus,
    parse_strategy_spec,
    resolve_capabilities,
)


def _series(values: list[float]) -> pd.Series:
    return pd.Series(values, index=pd.RangeIndex(len(values)), dtype=float)


def test_rolling_std_matches_pandas_ddof0_and_shift():
    close = _series([1, 2, 3, 4, 5, 6, 7, 8])
    expr = Expression.model_validate({
        "op": "rolling_std",
        "input": {"op": "field", "name": "close"},
        "window": 3,
        "shift": 1,
    })
    out = evaluate_expression(expr, {"close": close})
    history = close.shift(1)
    expected = history.rolling(3).std(ddof=ROLLING_STD_DDOF)
    pd.testing.assert_series_equal(out, expected, check_names=False)
    assert ROLLING_STD_DDOF == 0


def test_zscore_zero_variance_is_nan():
    close = _series([5.0] * 8)
    expr = Expression.model_validate({
        "op": "zscore",
        "input": {"op": "field", "name": "close"},
        "window": 3,
        "shift": 0,
    })
    out = evaluate_expression(expr, {"close": close})
    # 常数序列 std=0 -> NaN
    assert out.iloc[3:].isna().all()


def test_zscore_matches_mean_over_std():
    close = _series([1, 3, 2, 8, 4, 6, 5, 7], )
    expr = Expression.model_validate({
        "op": "zscore",
        "input": {"op": "field", "name": "close"},
        "window": 4,
        "shift": 1,
    })
    out = evaluate_expression(expr, {"close": close})
    history = close.shift(1)
    mean = history.rolling(4).mean()
    std = history.rolling(4).std(ddof=0)
    expected = (history - mean) / std.where(std != 0)
    pd.testing.assert_series_equal(out, expected, check_names=False)


def test_rolling_rank_percentile_in_unit_interval():
    close = _series([1, 2, 10, 3, 4, 5, 6, 7])
    expr = Expression.model_validate({
        "op": "rolling_rank",
        "input": {"op": "field", "name": "close"},
        "window": 3,
        "shift": 0,
    })
    out = evaluate_expression(expr, {"close": close})
    # window 满后: 末值在窗内的百分位
    # 索引 2: [1,2,10] -> 10 是最高 -> 1.0
    assert out.iloc[2] == pytest.approx(1.0)
    # 索引 3: [2,10,3] -> 3 排 2/3
    assert out.iloc[3] == pytest.approx(2 / 3)
    valid = out.dropna()
    assert (valid > 0).all() and (valid <= 1).all()


def test_shift1_no_same_bar_dependency():
    """shift=1 时第 t 日输出不依赖 t 日 input(改 t 日不应改 t 日结果)。"""
    base = _series([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0])
    for op in ("rolling_std", "rolling_rank", "zscore"):
        expr = Expression.model_validate({
            "op": op,
            "input": {"op": "field", "name": "close"},
            "window": 3,
            "shift": 1,
        })
        out_a = evaluate_expression(expr, {"close": base})
        mutated = base.copy()
        t = 5
        mutated.iloc[t] = 999.0
        out_b = evaluate_expression(expr, {"close": mutated})
        # 第 t 日结果应相同(只用到 shift 后的历史,不含当日)
        a_t, b_t = out_a.iloc[t], out_b.iloc[t]
        if pd.isna(a_t) and pd.isna(b_t):
            pass
        else:
            assert a_t == pytest.approx(b_t), op
        # 第 t+1 日会吃到被改写的 t 日(经 shift=1),对 std/zscore 一定变化
        if op in {"rolling_std", "zscore"}:
            assert out_a.iloc[t + 1] != pytest.approx(out_b.iloc[t + 1]), op


def test_new_ops_in_whitelist_and_spec_shape():
    for op in ("rolling_std", "rolling_rank", "zscore"):
        Expression.model_validate({
            "op": op,
            "input": {"op": "field", "name": "close"},
            "window": 20,
            "shift": 1,
        })
    with pytest.raises(Exception):
        Expression.model_validate({
            "op": "rolling_std",
            "input": {"op": "field", "name": "close"},
            "window": 20,
            # 缺 shift
        })


def test_spec_with_rolling_std_filter_supported():
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["breakout"])
    # 叠加波动过滤: rolling_std(close,20,1) > 0 (几乎总真,仅验证可编译)
    vol = {
        "op": "gt",
        "left": {
            "op": "rolling_std",
            "input": {"op": "field", "name": "close"},
            "window": 20,
            "shift": 1,
        },
        "right": {"op": "literal", "value": 0.0},
    }
    raw["entry"]["condition"] = {
        "op": "all",
        "args": [raw["entry"]["condition"], vol],
    }
    if "close" not in {d["field"] for d in raw["data_requirements"]}:
        raw["data_requirements"].append({
            "field": "close", "availability": "daily_close", "required": True,
        })
    report = resolve_capabilities(raw)
    assert report.status == CapabilityStatus.SUPPORTED
    parse_strategy_spec(raw)
