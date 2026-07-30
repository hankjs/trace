"""操作符注册表元数据与一致性测试。"""
from __future__ import annotations

import re
import sys
from collections.abc import Callable
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.operators import OPERATORS, OperatorSpec, compute_min_bars
from app.strategy.spec import SUPPORTED_OPERATORS, _OP_FIELDS, Expression


def test_registry_has_31_ops():
    """注册表包含全部受控操作符(与历史 _OP_FIELDS 条目数一致)。"""
    assert len(OPERATORS) == len(_OP_FIELDS)
    assert SUPPORTED_OPERATORS == frozenset(OPERATORS)


def test_every_op_has_complete_spec():
    for op, spec in OPERATORS.items():
        assert spec.op == op
        assert isinstance(spec.fields, frozenset)
        assert "op" in spec.fields
        assert isinstance(spec.arg_types, dict)
        assert spec.result_type in {"number", "bool", "literal"}
        assert callable(spec.evaluate)
        assert callable(spec.min_window)
        assert isinstance(spec.version, str)


def test_op_fields_match_derived_snapshot():
    """_OP_FIELDS 由注册表 fields 派生,与 baseline 一致。"""
    assert _OP_FIELDS == {op: spec.fields for op, spec in OPERATORS.items()}


@pytest.mark.parametrize(
    "expr_dict,expected",
    [
        # momentum close w20: field(1) + window(20) = 21
        (
            {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 20},
            21,
        ),
        # rsi14: field(1) + window(14) = 15(ewm 预热口径,与因子种子一致)
        (
            {"op": "rsi", "input": {"op": "field", "name": "close"}, "window": 14},
            15,
        ),
        # ma20_slope-style: ma(momentum(close,20), 5) -> 21 + 5 - 1 = 25
        (
            {
                "op": "ma",
                "input": {
                    "op": "momentum",
                    "input": {"op": "field", "name": "close"},
                    "window": 20,
                },
                "window": 5,
            },
            25,
        ),
        # atr14: max(high,low,close)=1 + window=14 = 15
        (
            {
                "op": "atr",
                "high": {"op": "field", "name": "high"},
                "low": {"op": "field", "name": "low"},
                "close": {"op": "field", "name": "close"},
                "window": 14,
            },
            15,
        ),
        # volume_ratio w5 s1: field(1) + shift(1) + window(5) - 1 = 6
        (
            {
                "op": "volume_ratio",
                "input": {"op": "field", "name": "volume"},
                "window": 5,
                "shift": 1,
            },
            6,
        ),
        # rolling_mean w20 s0: field(1) + shift(0) + window(20) - 1 = 20
        (
            {
                "op": "rolling_mean",
                "input": {"op": "field", "name": "close"},
                "window": 20,
                "shift": 0,
            },
            20,
        ),
    ],
)
def test_compute_min_bars(expr_dict, expected):
    expr = Expression.model_validate(expr_dict)
    assert compute_min_bars(expr) == expected


def test_min_bars_clamped_to_at_least_one():
    expr = Expression.model_validate({"op": "literal", "value": 1})
    assert compute_min_bars(expr) == 1


def test_frontend_op_registry_drift():
    """前端 EXPRESSION_OPS 必须覆盖后端所有操作符键名。"""
    web_src = Path(__file__).resolve().parent.parent / "web" / "src" / "specExpression.ts"
    text = web_src.read_text(encoding="utf-8")

    # 容错提取器:匹配 EXPRESSION_OPS 数组内 { op: '...' } 的键名
    ops_in_file = set(re.findall(r"\{\s*op:\s*['\"]([a-z_]+)['\"]", text))
    backend_ops = set(OPERATORS)

    missing = backend_ops - ops_in_file
    if missing:
        # 如果宽松提取器失败,退化为简单文本包含检查并给出明确诊断
        for op in sorted(backend_ops):
            assert (
                f"op: '{op}'" in text or f'op: "{op}"' in text
            ), f"前端未声明操作符 {op}"
    assert ops_in_file == backend_ops
