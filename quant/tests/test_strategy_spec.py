"""数据库 StrategySpec、受控算子与通用编译器测试。"""
from __future__ import annotations

from copy import deepcopy
from datetime import date
import json
from pathlib import Path
import sys

import numpy as np
import pandas as pd
import pytest
from pydantic import ValidationError

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.compiler import compile_portfolio, compile_single
from app.strategy.components import evaluate_expression
from app.strategy.presets import SYSTEM_STRATEGY_SPECS, get_preset_spec
from app.strategy.spec import (
    CapabilityStatus,
    Expression,
    MAX_AST_DEPTH,
    StrategySpec,
    canonical_spec_json,
    resolve_capabilities,
    strategy_spec_hash,
    validate_strategy_spec,
)
from app.strategy.strategies import (
    breakout,
    ma_cross,
    mean_reversion,
    momentum_rotation,
    multifactor_hold,
    volume_breakout,
)


def _bars(
    dates: pd.DatetimeIndex,
    close: np.ndarray | list[float],
    *,
    volume: np.ndarray | list[float] | None = None,
) -> pd.DataFrame:
    close = np.asarray(close, dtype=float)
    volume = (
        np.asarray(volume, dtype=float)
        if volume is not None
        else np.full(len(close), 1_000_000.0)
    )
    return pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": close * 1.01,
        "low": close * 0.99,
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": volume * close,
    })


def _random_bars(periods: int = 180) -> pd.DataFrame:
    rng = np.random.default_rng(20260727)
    dates = pd.bdate_range(date(2023, 1, 2), periods=periods)
    close = 100 * np.exp(np.cumsum(rng.normal(0, 0.025, periods)))
    frame = _bars(dates, close, volume=rng.integers(100_000, 3_000_000, periods))
    frame["open"] = close * (1 + rng.normal(0, 0.004, periods))
    frame["high"] = np.maximum(frame["open"], close) * (1 + rng.uniform(0, 0.02, periods))
    frame["low"] = np.minimum(frame["open"], close) * (1 - rng.uniform(0, 0.02, periods))
    return frame


def _volume_breakout_bars(window: int = 20) -> pd.DataFrame:
    dates = pd.bdate_range(date(2024, 1, 2), periods=window + 8)
    close = np.array([10.0] * window + [10.5, 10.4, 10.3, 9.5, 9.4, 9.3, 9.2, 9.1])
    volume = np.array(
        [1_000.0] * (window - 5) + [100.0] * 5
        + [4_000.0] + [800.0] * 7
    )
    frame = _bars(dates, close, volume=volume)
    frame.loc[:window - 1, "high"] = 10.1
    frame.loc[:window - 1, "low"] = 9.9
    return frame


def _portfolio_pool(periods: int = 120) -> tuple[pd.DatetimeIndex, dict[str, pd.DataFrame]]:
    dates = pd.bdate_range(date(2024, 1, 2), periods=periods)
    pool = {}
    for number, daily_return in enumerate((0.009, 0.007, 0.004, 0.001, -0.001)):
        prices = 10 * (1 + daily_return) ** np.arange(periods)
        prices *= 1 + 0.004 * np.sin(np.arange(periods) / 7 + number)
        pool[f"stock_{number}"] = _bars(dates, prices)
    return dates, pool


def test_six_system_presets_are_complete_strict_specs_with_stable_hashes():
    assert set(SYSTEM_STRATEGY_SPECS) == {
        "ma_cross", "breakout", "mean_reversion", "volume_breakout",
        "momentum_rotation", "multifactor_hold",
    }
    required_sections = {
        "metadata", "universe", "data_requirements", "entry", "positioning",
        "holding", "native_exit", "overlays", "portfolio_constraints",
        "execution", "validation",
    }
    for raw in SYSTEM_STRATEGY_SPECS.values():
        parsed = StrategySpec.model_validate(raw)
        assert required_sections <= set(raw)
        assert parsed.schema_version == 1
        assert len(strategy_spec_hash(parsed)) == 64
        assert strategy_spec_hash(raw) == strategy_spec_hash(parsed)
        assert validate_strategy_spec(raw).valid is True


def test_canonical_json_ignores_object_key_order_but_not_rule_changes():
    original = deepcopy(SYSTEM_STRATEGY_SPECS["breakout"])

    def reverse(value):
        if isinstance(value, dict):
            return {key: reverse(value[key]) for key in reversed(value)}
        if isinstance(value, list):
            return [reverse(item) for item in value]
        return value

    reordered = reverse(original)
    assert canonical_spec_json(original) == canonical_spec_json(reordered)
    assert strategy_spec_hash(original) == strategy_spec_hash(reordered)

    changed = deepcopy(original)
    changed["entry"]["condition"]["right"]["window"] = 21
    assert strategy_spec_hash(changed) != strategy_spec_hash(original)


def test_strict_shape_rejects_unknown_sections_and_operator_fields():
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    raw["python"] = "pass"
    with pytest.raises(ValidationError, match="Extra inputs are not permitted"):
        StrategySpec.model_validate(raw)

    condition = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"]["entry"]["condition"])
    condition["window"] = 20
    with pytest.raises(ValidationError, match="形状错误"):
        Expression.model_validate(condition)


@pytest.mark.parametrize(
    ("mutation", "status", "path"),
    [
        (
            lambda raw: raw["entry"].update({
                "condition": {"op": "future_magic", "input": {"op": "field", "name": "close"}},
            }),
            CapabilityStatus.MISSING_ENGINE,
            "$.entry.condition.op",
        ),
        (
            lambda raw: raw["entry"].update({
                "condition": {"op": "gt", "left": {"op": "field", "name": "minute_price"},
                              "right": {"op": "literal", "value": 1}},
            }),
            CapabilityStatus.MISSING_DATA,
            "$.entry.condition.left.name",
        ),
        (
            lambda raw: raw["entry"].update({"condition": {"op": "subjective"}}),
            CapabilityStatus.SUBJECTIVE_ONLY,
            "$.entry.condition.op",
        ),
        (
            lambda raw: raw["execution"].update({"execution_time": "intraday"}),
            CapabilityStatus.BOUNDARY_DENIED,
            "$.execution.execution_time",
        ),
        (
            lambda raw: raw["entry"]["condition"].update({"code": "eval('1 + 1')"}),
            CapabilityStatus.BOUNDARY_DENIED,
            "$.entry.condition.code",
        ),
    ],
)
def test_capability_resolver_reports_precise_paths(mutation, status, path):
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    mutation(raw)
    report = resolve_capabilities(raw)
    assert report.status == status
    assert any(issue.status == status and issue.path == path for issue in report.issues)


def test_capability_resolver_checks_fields_present_in_the_data_snapshot():
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["breakout"])
    report = resolve_capabilities(raw, available_fields={"close", "high"})
    assert report.status == CapabilityStatus.MISSING_DATA
    assert any(
        issue.path == "$.data_requirements[2].field"
        and issue.code == "field_not_available"
        for issue in report.issues
    )


def test_holding_adjust_requires_rule_and_reports_precise_path():
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    raw["holding"]["allow_add"] = True
    report = resolve_capabilities(raw)
    assert report.status == CapabilityStatus.MISSING_ENGINE
    assert any(
        issue.path == "$.holding.add_rule" and issue.code == "holding_rule_missing"
        for issue in report.issues
    )


def test_ast_window_depth_node_and_parameter_scan_limits_are_enforced():
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    raw["entry"]["condition"] = {
        "op": "rolling_mean", "input": {"op": "field", "name": "close"},
        "window": 501, "shift": 0,
    }
    assert resolve_capabilities(raw).status == CapabilityStatus.MISSING_ENGINE

    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    condition: dict = {"op": "literal", "value": True}
    for _ in range(MAX_AST_DEPTH + 1):
        condition = {"op": "not", "arg": condition}
    raw["entry"]["condition"] = condition
    with pytest.raises(ValidationError, match="AST 深度"):
        StrategySpec.model_validate(raw)

    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    raw["entry"]["condition"] = {
        "op": "all",
        "args": [{"op": "literal", "value": True} for _ in range(257)],
    }
    with pytest.raises(ValidationError, match="AST 节点数"):
        StrategySpec.model_validate(raw)

    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    raw["validation"]["parameter_scans"] = [
        {"path": "$.entry.fast", "values": list(range(17))},
        {"path": "$.entry.slow", "values": list(range(17))},
    ]
    with pytest.raises(ValidationError, match="参数扫描组合数"):
        StrategySpec.model_validate(raw)


def test_controlled_operator_registry_covers_series_and_cross_sectional_ops():
    index = pd.RangeIndex(8)
    close = pd.Series([1, 2, 3, 2, 4, 8, 4, 2], index=index, dtype=float)
    volume = pd.Series([1, 1, 1, 2, 4, 8, 4, 2], index=index, dtype=float)
    fields = {
        "close": close,
        "high": close + 1,
        "low": close - 1,
        "volume": volume,
    }

    expressions = [
        {"op": "rolling_mean", "input": {"op": "field", "name": "close"},
         "window": 3, "shift": 1},
        {"op": "rolling_max", "input": {"op": "field", "name": "close"},
         "window": 3, "shift": 0},
        {"op": "rolling_min", "input": {"op": "field", "name": "close"},
         "window": 3, "shift": 0},
        {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 3},
        {"op": "rsi", "input": {"op": "field", "name": "close"}, "window": 3},
        {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 3},
        {"op": "return", "input": {"op": "field", "name": "close"}, "window": 3},
        {"op": "volume_ratio", "input": {"op": "field", "name": "volume"},
         "window": 3, "shift": 1},
        {"op": "atr", "high": {"op": "field", "name": "high"},
         "low": {"op": "field", "name": "low"},
         "close": {"op": "field", "name": "close"}, "window": 3},
        {"op": "cross_above", "left": {"op": "field", "name": "close"},
         "right": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 3}},
        {"op": "cross_below", "left": {"op": "field", "name": "close"},
         "right": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 3}},
        {"op": "cross_above", "left": {"op": "field", "name": "close"},
         "right": {"op": "literal", "value": 3}},
    ]
    for raw in expressions:
        result = evaluate_expression(Expression.model_validate(raw), fields)
        assert isinstance(result, pd.Series)
        assert len(result) == len(close)

    arithmetic = Expression.model_validate({
        "op": "all", "args": [
            {"op": "gte", "left": {"op": "add", "left": {"op": "literal", "value": 2},
                                     "right": {"op": "literal", "value": 3}},
             "right": {"op": "literal", "value": 5}},
            {"op": "not", "arg": {"op": "lt", "left": {"op": "literal", "value": 2},
                                     "right": {"op": "literal", "value": 1}}},
        ],
    })
    assert evaluate_expression(arithmetic, fields) is True

    matrix = pd.DataFrame({"a": [3.0, 1.0], "b": [2.0, 4.0], "c": [1.0, 2.0]})
    rank = Expression.model_validate({
        "op": "rank", "input": {"op": "field", "name": "close"}, "ascending": False,
    })
    top_n = Expression.model_validate({
        "op": "top_n", "input": {"op": "field", "name": "close"}, "n": 2,
    })
    assert evaluate_expression(rank, {"close": matrix}).loc[0].tolist() == [1, 2, 3]
    assert evaluate_expression(top_n, {"close": matrix}).loc[1].tolist() == [False, True, True]


@pytest.mark.parametrize(
    ("name", "legacy", "params"),
    [
        ("ma_cross", ma_cross, {"fast": 3, "slow": 15}),
        ("breakout", breakout, {"entry": 10, "exit": 5}),
        ("mean_reversion", mean_reversion, {"rsi_buy": 40, "rsi_sell": 60, "ma": 30}),
    ],
)
def test_single_presets_regress_to_legacy_positions(name, legacy, params):
    frame = _random_bars()
    expected = legacy.positions(frame, params)
    actual = compile_single(get_preset_spec(name, params), frame).positions
    pd.testing.assert_series_equal(actual, expected, check_names=False)


def test_volume_breakout_preset_regresses_entry_native_exit_and_atr_overlay():
    params = {"window": 20, "range_max": 0.15, "vol_mult": 2.0, "atr_mult": 2.0}
    frame = _volume_breakout_bars()
    expected = volume_breakout.positions(frame, params)
    result = compile_single(get_preset_spec("volume_breakout", params), frame)

    pd.testing.assert_series_equal(result.positions, expected, check_names=False)
    assert result.positions.max() == 1
    assert any(item.state == "entry_pending" for item in result.transitions)
    assert any(item.state == "exit_pending" for item in result.transitions)
    assert any(
        reason["code"] in {"risk_overlay", "native_exit"}
        for day_reasons in result.reasons.values()
        for reason in day_reasons
        if "code" in reason
    )
    json.dumps(result.reason_events(), ensure_ascii=False)
    json.dumps([item.as_dict() for item in result.transitions], ensure_ascii=False)


@pytest.mark.parametrize(
    ("name", "legacy", "params"),
    [
        ("momentum_rotation", momentum_rotation,
         {"top_n": 2, "w_mom20": 0.7, "w_mom60": 0.3}),
        ("multifactor_hold", multifactor_hold, {"top_n": 2}),
    ],
)
def test_portfolio_presets_regress_to_legacy_weights(name, legacy, params):
    dates, pool = _portfolio_pool()
    expected = legacy.target_weights(dates.date, pool, params)
    result = compile_portfolio(get_preset_spec(name, params), dates.date, pool)

    pd.testing.assert_frame_equal(result.weights, expected)
    assert result.transitions
    assert result.reasons
    assert (result.weights >= 0).all().all()
    assert (result.weights.sum(axis=1) <= 1 + 1e-12).all()
    json.dumps(result.reason_events(), ensure_ascii=False)


def test_fixed_and_rank_weighting_are_deterministic_and_respect_constraints():
    dates, pool = _portfolio_pool(periods=90)
    raw = get_preset_spec("momentum_rotation", {"top_n": 3}).model_dump(mode="json")
    raw["positioning"]["rebalance"] = {"frequency": "fixed", "interval_days": 5}
    raw["positioning"]["weighting"] = {"type": "rank"}
    raw["positioning"]["risk_filter"] = None
    raw["portfolio_constraints"]["max_single_weight"] = 0.5
    raw["portfolio_constraints"]["max_total_weight"] = 0.8

    first = compile_portfolio(raw, dates.date, pool).weights
    second = compile_portfolio(raw, dates.date, pool).weights
    pd.testing.assert_frame_equal(first, second)
    assert (first.max(axis=1) <= 0.5 + 1e-12).all()
    assert (first.sum(axis=1) <= 0.8 + 1e-12).all()


def test_portfolio_fixed_pct_overlay_emits_exit_reason_and_locks_until_rebalance():
    dates = pd.bdate_range(date(2024, 1, 2), periods=12)
    pool = {"only": _bars(dates, [10, 10, 10, 8, 8, 8, 8, 8, 8, 8, 8, 8])}
    raw = get_preset_spec("momentum_rotation", {"top_n": 1}).model_dump(mode="json")
    raw["positioning"]["score"] = {"op": "field", "name": "close"}
    raw["positioning"]["risk_filter"] = None
    raw["positioning"]["rebalance"] = {"frequency": "fixed", "interval_days": 10}
    raw["overlays"]["risk"] = {
        "enabled": True, "type": "fixed_pct", "value": 0.1,
        "atr_period": 14, "trailing": False,
    }
    result = compile_portfolio(raw, dates.date, pool)

    assert result.weights["only"].tolist()[:5] == [1.0, 1.0, 1.0, 0.0, 0.0]
    assert any(
        item.get("all_reasons", [{}])[0].get("code") == "risk_overlay"
        for items in result.reasons.values()
        for item in items
    )


def test_fixed_pct_overlay_exits_and_requires_native_reset_before_reentry():
    dates = pd.bdate_range(date(2024, 1, 2), periods=6)
    frame = _bars(dates, [10, 10, 8, 10, 10, 10])
    raw = get_preset_spec("ma_cross").model_dump(mode="json")
    raw["data_requirements"] = [
        {"field": name, "availability": "daily_close", "required": True}
        for name in ("close", "high", "low")
    ]
    raw["entry"] = {"condition": {"op": "literal", "value": True}, "reason_code": "always"}
    raw["native_exit"] = {
        "condition": {"op": "literal", "value": False}, "reason_code": "never",
    }
    raw["overlays"]["risk"] = {
        "enabled": True, "type": "fixed_pct", "value": 0.1,
        "atr_period": 14, "trailing": False,
    }
    result = compile_single(raw, frame)

    assert result.positions.tolist() == [1, 1, 0, 0, 0, 0]
    exit_day = pd.Timestamp(dates[2])
    assert result.reasons[exit_day][0]["code"] == "risk_overlay"
    assert any(item.state == "cooldown" for item in result.transitions)


def test_preset_adapter_rejects_unknown_parameters():
    with pytest.raises(ValueError, match="未知字段"):
        get_preset_spec("ma_cross", {"expression": "close > ma"})


def test_disabled_generic_overlay_does_not_remove_volume_strategy_native_atr_exit():
    spec = get_preset_spec("volume_breakout", {
        "risk_overlay": {
            "enabled": False, "type": "fixed_pct", "value": 0.08,
            "atr_period": 14,
        },
    })
    assert spec.overlays.risk.enabled is True
    assert spec.overlays.risk.type == "atr_multiple"
    assert spec.overlays.risk.value == 2.0

