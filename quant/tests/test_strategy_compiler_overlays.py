"""StrategySpec 编译器与既有风险覆盖层的等价性测试。"""
from __future__ import annotations

from copy import deepcopy
from datetime import date
from pathlib import Path
import sys

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.compiler import compile_portfolio, compile_single
from app.strategy.overlays import apply_portfolio_overlays, apply_single_overlays
from app.strategy.presets import get_preset_spec


def _bars(
    periods: int,
    *,
    signal_position: int,
    execution_open: float,
    execution_close: float,
) -> pd.DataFrame:
    dates = pd.bdate_range(date(2024, 1, 2), periods=periods)
    opens = np.full(periods, 10.0)
    closes = np.full(periods, 10.0)
    opens[signal_position + 1] = execution_open
    closes[signal_position + 1] = execution_close
    return pd.DataFrame({
        "date": dates.date,
        "open": opens,
        "high": np.maximum(opens, closes) + 0.2,
        "low": np.minimum(opens, closes) - 0.2,
        "close": closes,
        "raw_close": closes,
        "volume": [0.0] * signal_position + [1_000_000.0] * (periods - signal_position),
        "amount": 10_000_000.0,
    })


def _single_spec(overlay_name: str, overlay: dict) -> dict:
    raw = get_preset_spec("ma_cross").model_dump(mode="json")
    raw["data_requirements"] = [
        {"field": field, "availability": "daily_close", "required": True}
        for field in ("close", "high", "low", "volume")
    ]
    raw["entry"] = {
        "condition": {
            "op": "gt",
            "left": {"op": "field", "name": "volume"},
            "right": {"op": "literal", "value": 0},
        },
        "reason_code": "volume_available",
    }
    raw["native_exit"] = {
        "condition": {"op": "literal", "value": False},
        "reason_code": "never",
    }
    raw["overlays"][overlay_name] = overlay
    return raw


def _legacy_overlay_params(raw: dict) -> dict:
    return {
        "risk_overlay": {
            key: value
            for key, value in raw["overlays"]["risk"].items()
            if key != "trailing"
        },
        "take_profit": {
            key: value
            for key, value in raw["overlays"]["take_profit"].items()
            if key != "trailing"
        },
    }


def _without_overlays(raw: dict) -> dict:
    native = deepcopy(raw)
    native["overlays"]["risk"]["enabled"] = False
    native["overlays"]["take_profit"]["enabled"] = False
    return native


def _persisted_reasons(items: list[dict]) -> list[dict]:
    return [
        {key: value for key, value in item.items() if key in {"code", "price_line"}}
        for item in items
    ]


@pytest.mark.parametrize(
    ("overlay_name", "overlay", "execution_open", "execution_close", "reason_code"),
    [
        (
            "risk",
            {"enabled": True, "type": "fixed_pct", "value": 0.10,
             "atr_period": 14, "trailing": False},
            12.0,
            10.8,
            "risk_overlay",
        ),
        (
            "risk",
            {"enabled": True, "type": "atr_multiple", "value": 2.0,
             "atr_period": 14, "trailing": False},
            12.0,
            11.0,
            "risk_overlay",
        ),
        (
            "take_profit",
            {"enabled": True, "type": "fixed_pct", "value": 0.10,
             "atr_period": 14, "trailing": False},
            8.0,
            9.0,
            "take_profit",
        ),
    ],
)
def test_single_compiler_matches_legacy_overlay_state_machine(
    overlay_name,
    overlay,
    execution_open,
    execution_close,
    reason_code,
):
    signal_position = 15
    frame = _bars(
        22,
        signal_position=signal_position,
        execution_open=execution_open,
        execution_close=execution_close,
    )
    raw = _single_spec(overlay_name, overlay)
    slippage = 0.01
    native = compile_single(_without_overlays(raw), frame).positions
    expected_state: dict = {}
    expected_positions, expected_reasons = apply_single_overlays(
        frame,
        native,
        _legacy_overlay_params(raw),
        slippage=slippage,
        state_out=expected_state,
    )

    actual = compile_single(raw, frame, slippage=slippage)

    assert actual.positions.tolist() == expected_positions.tolist()
    assert actual.state == expected_state
    exit_day = pd.Timestamp(frame["date"].iat[signal_position + 1])
    assert _persisted_reasons(expected_reasons[exit_day]) == actual.reasons[exit_day]
    assert actual.reasons[exit_day][0]["code"] == reason_code
    # 风险线必须基于 T+1 模拟开盘价，而不是信号日 10 元收盘价。
    assert actual.reasons[exit_day][0]["price_line"] != pytest.approx(
        10.0 * (0.9 if reason_code == "risk_overlay" else 1.1)
    )


def _portfolio_spec(overlay: dict) -> dict:
    raw = get_preset_spec("momentum_rotation", {"top_n": 1}).model_dump(mode="json")
    raw["data_requirements"] = [
        {"field": field, "availability": "daily_close", "required": True}
        for field in ("close", "high", "low", "volume")
    ]
    raw["entry"] = {
        "condition": {
            "op": "gt",
            "left": {"op": "field", "name": "volume"},
            "right": {"op": "literal", "value": 0},
        },
        "reason_code": "volume_available",
    }
    raw["positioning"]["score"] = {"op": "field", "name": "close"}
    raw["positioning"]["risk_filter"] = None
    raw["positioning"]["rebalance"] = {
        "frequency": "fixed",
        "interval_days": 16,
    }
    raw["overlays"]["risk"] = overlay
    return raw


@pytest.mark.parametrize(
    ("overlay", "execution_close"),
    [
        (
            {"enabled": True, "type": "fixed_pct", "value": 0.10,
             "atr_period": 14, "trailing": False},
            10.8,
        ),
        (
            {"enabled": True, "type": "atr_multiple", "value": 2.0,
             "atr_period": 14, "trailing": False},
            11.0,
        ),
    ],
)
def test_portfolio_compiler_matches_legacy_overlay_with_t1_entry_price(
    overlay,
    execution_close,
):
    signal_position = 16
    frame = _bars(
        24,
        signal_position=signal_position,
        execution_open=12.0,
        execution_close=execution_close,
    )
    dates = pd.DatetimeIndex(frame["date"])
    pool = {"only": frame}
    raw = _portfolio_spec(overlay)
    slippage = 0.01
    native = compile_portfolio(
        _without_overlays(raw), dates, pool,
    ).weights
    rebalance = pd.Series(
        [position % 16 == 0 for position in range(len(dates))],
        index=dates,
    )
    expected_weights, expected_reasons, _ = apply_portfolio_overlays(
        native,
        pool,
        _legacy_overlay_params(raw),
        rebalance,
        slippage=slippage,
    )

    actual = compile_portfolio(raw, dates, pool, slippage=slippage)

    pd.testing.assert_frame_equal(actual.weights, expected_weights)
    exit_day = dates[signal_position + 1]
    transition_reason = actual.reasons[(exit_day, "only")][0]
    assert transition_reason["all_reasons"] == _persisted_reasons(
        expected_reasons[(exit_day, "only")]
    )
    assert transition_reason["all_reasons"][0]["code"] == "risk_overlay"
    assert transition_reason["all_reasons"][0]["price_line"] != pytest.approx(9.0)
