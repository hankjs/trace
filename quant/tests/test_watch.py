"""通用「临近触发」(watch) 判定的行为断言。

覆盖 app/strategy/watch.py 的语义契约:
1. 各比较算子的归一化间距(gap > 0 未触发、越小越接近);
2. all/any 聚合规则(AND 取最大、OR 取最小)与 not 的保守处理;
3. cross_above/cross_below 事件型节点「差距在容差内且尚未交叉」才算临近;
4. 数据不足(NaN)保守视为不临近。
"""
from __future__ import annotations

import sys
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.spec import Expression
from app.strategy.watch import WATCH_TOLERANCE, assess_entry_watch


def _frame(close: list[float], *, high: list[float] | None = None,
           volume: list[float] | None = None) -> pd.DataFrame:
    dates = pd.bdate_range(end="2026-07-24", periods=len(close))
    return pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": high or [value + 0.1 for value in close],
        "low": [value - 0.1 for value in close],
        "close": close,
        "raw_close": close,
        "volume": volume or [1_000_000.0] * len(close),
        "amount": [1e7] * len(close),
    })


def _expr(raw: dict) -> Expression:
    return Expression.model_validate(raw)


def _compare(op: str, left: dict, right: dict) -> dict:
    return {"op": op, "left": left, "right": right}


CLOSE = {"op": "field", "name": "close"}


def test_gt_gap_normalization_and_tolerance():
    condition = _expr(_compare("gt", CLOSE, {"op": "literal", "value": 10.0}))
    near = assess_entry_watch(condition, _frame([9.0] * 5 + [9.95]))
    far = assess_entry_watch(condition, _frame([9.0] * 5 + [9.5]))

    # (10 - 9.95) / 10 = 0.5%,在 2% 容差内
    assert near["near"] is True
    assert near["triggered"] is False
    assert near["gap"] == pytest.approx(0.005, abs=1e-6)
    assert near["binding"]["op"] == "gt"
    # (10 - 9.5) / 10 = 5%,超出容差
    assert far["near"] is False
    assert far["gap"] == pytest.approx(0.05, abs=1e-6)


def test_lt_gap_uses_same_sign_convention():
    condition = _expr(_compare("lt", CLOSE, {"op": "literal", "value": 10.0}))
    result = assess_entry_watch(condition, _frame([11.0] * 5 + [10.1]))

    # lt 取 (left - right) / |right| = 1%,与 gt 同号约定:>0 未触发、越小越近
    assert result["near"] is True
    assert result["gap"] == pytest.approx(0.01, abs=1e-6)


def test_triggered_condition_is_not_near():
    condition = _expr(_compare("gte", CLOSE, {"op": "literal", "value": 10.0}))
    result = assess_entry_watch(condition, _frame([9.0] * 5 + [10.5]))

    assert result["triggered"] is True
    assert result["near"] is False


def test_all_aggregation_uses_max_gap():
    near_leg = _compare("gt", CLOSE, {"op": "literal", "value": 10.0})
    far_leg = _compare(
        "gt", {"op": "field", "name": "volume"}, {"op": "literal", "value": 2e6},
    )
    frame = _frame([9.95] * 5, volume=[1_000_000.0] * 5)

    only_near = assess_entry_watch(_expr({"op": "all", "args": [
        near_leg, _compare("gt", CLOSE, {"op": "literal", "value": 9.0}),
    ]}), frame)
    with_far = assess_entry_watch(
        _expr({"op": "all", "args": [near_leg, far_leg]}), frame,
    )

    # AND:已成立的腿(gap<=0)不拖后腿,但最远的腿决定整体是否临近
    assert only_near["near"] is True
    assert with_far["near"] is False
    assert with_far["gap"] == pytest.approx(0.5, abs=1e-6)


def test_any_aggregation_uses_min_gap():
    legs = [
        _compare("gt", CLOSE, {"op": "literal", "value": 20.0}),   # 很远
        _compare("gt", CLOSE, {"op": "literal", "value": 10.0}),   # 临近
    ]
    result = assess_entry_watch(
        _expr({"op": "any", "args": legs}), _frame([9.95] * 5),
    )

    assert result["near"] is True
    assert result["gap"] == pytest.approx(0.005, abs=1e-6)


def test_not_branch_never_contributes_proximity():
    # not 成立时按「已满足」参与 AND,不阻挡其他腿的临近判定
    condition = _expr({"op": "all", "args": [
        {"op": "not", "arg": _compare("gt", CLOSE, {"op": "literal", "value": 100.0})},
        _compare("gt", CLOSE, {"op": "literal", "value": 10.0}),
    ]})
    result = assess_entry_watch(condition, _frame([9.95] * 5))
    assert result["near"] is True

    # 条件只有 not 且当前成立 -> 已经 triggered,不是「临近」
    only_not = _expr(
        {"op": "not", "arg": _compare("gt", CLOSE, {"op": "literal", "value": 10.0})},
    )
    result = assess_entry_watch(only_not, _frame([9.95] * 5))
    assert result["triggered"] is True
    assert result["near"] is False

    # not 不成立时保守视为不可评估,不产生临近
    failing_not = _expr(
        {"op": "not", "arg": _compare("lt", CLOSE, {"op": "literal", "value": 10.0})},
    )
    result = assess_entry_watch(failing_not, _frame([9.5] * 5))
    assert result["triggered"] is False
    assert result["near"] is False
    assert result["gap"] is None


def test_cross_above_near_only_when_close_and_not_crossed():
    condition = _expr(_compare(
        "cross_above", CLOSE, {"op": "literal", "value": 10.0},
    ))
    # 两侧差距在容差内且尚未交叉(当日与前一日都在下方)-> 临近
    near = assess_entry_watch(condition, _frame([9.8, 9.9, 9.95]))
    assert near["near"] is True
    assert near["gap"] == pytest.approx(0.005, abs=1e-6)

    # 当日已上穿 -> 事件已触发,不是 watch
    crossed_today = assess_entry_watch(condition, _frame([9.8, 9.9, 10.05]))
    assert crossed_today["triggered"] is True
    assert crossed_today["near"] is False

    # 更早已经交叉(事件已过)-> 不视为「即将上穿」
    crossed_earlier = assess_entry_watch(condition, _frame([9.8, 10.2, 10.3]))
    assert crossed_earlier["triggered"] is False
    assert crossed_earlier["near"] is False


def test_cross_below_mirror_semantics():
    condition = _expr(_compare(
        "cross_below", CLOSE, {"op": "literal", "value": 10.0},
    ))
    near = assess_entry_watch(condition, _frame([10.2, 10.1, 10.05]))
    assert near["near"] is True

    already_below = assess_entry_watch(condition, _frame([10.2, 9.8, 9.7]))
    assert already_below["near"] is False


def test_insufficient_history_is_conservatively_not_near():
    condition = _expr(_compare(
        "gt",
        {"op": "ma", "input": CLOSE, "window": 60},
        {"op": "literal", "value": 10.0},
    ))
    result = assess_entry_watch(condition, _frame([9.9] * 10))

    assert result["triggered"] is False
    assert result["near"] is False
    assert result["gap"] is None
    assert "不可评估" in result["summary"] or "不足" in result["summary"]


def test_summary_is_human_readable_with_binding_detail():
    condition = _expr(_compare("gt", CLOSE, {"op": "literal", "value": 10.0}))
    result = assess_entry_watch(condition, _frame([9.95] * 5))

    assert "临近触发" in result["summary"]
    assert "close" in result["summary"]
    assert f"{WATCH_TOLERANCE:.0%}" in result["summary"]
    assert result["binding"]["left"] == "close"
    assert result["binding"]["right_value"] == pytest.approx(10.0)
