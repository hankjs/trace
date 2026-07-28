"""FW-1: 首批片段 build → 最小 Spec 壳 → 后端 validate capability.supported。"""
from __future__ import annotations

import sys
from copy import deepcopy
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.strategies import StrategyValidateIn, validate_strategy
from app.strategy.presets import SYSTEM_STRATEGY_SPECS
from app.strategy.spec import CapabilityStatus, resolve_capabilities


def _field(name: str) -> dict:
    return {"op": "field", "name": name}


def _lit(v: float | int | bool) -> dict:
    return {"op": "literal", "value": v}


def _window(op: str, input_node: dict, window: int, shift: int) -> dict:
    return {"op": op, "input": input_node, "window": window, "shift": shift}


def _ma(input_node: dict, window: int) -> dict:
    return {"op": "ma", "input": input_node, "window": window}


# 与 web/src/specSnippets.ts 首批片段语义对齐(参数默认值)
SNIPPET_ASTS: dict[str, dict] = {
    "entry_breakout_n": {
        "op": "gt",
        "left": _field("close"),
        "right": _window("rolling_max", _field("high"), 20, 1),
    },
    "entry_breakout_vol": {
        "op": "all",
        "args": [
            {
                "op": "gt",
                "left": _field("close"),
                "right": _window("rolling_max", _field("high"), 20, 1),
            },
            {
                "op": "gt",
                "left": _window("volume_ratio", _field("volume"), 20, 1),
                "right": _lit(1.5),
            },
        ],
    },
    "entry_ma_cross_up": {
        "op": "cross_above",
        "left": _ma(_field("close"), 10),
        "right": _ma(_field("close"), 60),
    },
    "entry_close_above_ma": {
        "op": "gt",
        "left": _field("close"),
        "right": _ma(_field("close"), 20),
    },
    "exit_channel_low": {
        "op": "lt",
        "left": _field("close"),
        "right": _window("rolling_min", _field("low"), 10, 1),
    },
    "exit_ma_cross_down": {
        "op": "cross_below",
        "left": _ma(_field("close"), 10),
        "right": _ma(_field("close"), 60),
    },
    "exit_close_below_ma": {
        "op": "lt",
        "left": _field("close"),
        "right": _ma(_field("close"), 20),
    },
    "score_momentum_n": {
        "op": "momentum",
        "input": _field("close"),
        "window": 20,
    },
    "filter_rsi_oversold_recover": {
        "op": "cross_above",
        "left": {"op": "rsi", "input": _field("close"), "window": 14},
        "right": _lit(30.0),
    },
}


def _shell_for_snippet(snippet_id: str, ast: dict) -> dict:
    """把片段 AST 塞进合法 Spec 壳(不自动写 validation 之外的假数据)。"""
    if snippet_id == "score_momentum_n":
        raw = deepcopy(SYSTEM_STRATEGY_SPECS["momentum_rotation"])
        raw["positioning"]["score"] = ast
        fields = {"close"}
    elif snippet_id.startswith("exit_"):
        raw = deepcopy(SYSTEM_STRATEGY_SPECS["breakout"])
        raw["native_exit"] = {"condition": ast, "reason_code": "snippet_exit"}
        fields = {"close", "high", "low", "volume"}
    else:
        raw = deepcopy(SYSTEM_STRATEGY_SPECS["breakout"])
        raw["entry"] = {"condition": ast, "reason_code": "snippet_entry"}
        fields = {"close", "high", "low", "volume"}
    # 合并片段建议字段
    have = {d["field"] for d in raw["data_requirements"]}
    for name in fields:
        if name not in have:
            raw["data_requirements"].append({
                "field": name, "availability": "daily_close", "required": True,
            })
    return raw


@pytest.mark.parametrize("snippet_id,ast", list(SNIPPET_ASTS.items()))
def test_snippet_builds_supported_spec(snippet_id: str, ast: dict):
    raw = _shell_for_snippet(snippet_id, ast)
    report = resolve_capabilities(raw)
    assert report.status == CapabilityStatus.SUPPORTED, (snippet_id, report)
    result = validate_strategy(StrategyValidateIn(spec=raw), db=None)
    assert result["valid"] is True, (snippet_id, result.get("errors"))
    assert result["capability"]["status"] == "supported"


def test_first_batch_has_at_least_eight_covering_slots():
    assert len(SNIPPET_ASTS) >= 8
    ids = set(SNIPPET_ASTS)
    assert any(i.startswith("entry_") for i in ids)
    assert any(i.startswith("exit_") for i in ids)
    assert any(i.startswith("score_") for i in ids)
