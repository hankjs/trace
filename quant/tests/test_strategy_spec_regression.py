"""六个系统预置策略的 legacy/StrategySpec 交易证据回归。"""
from __future__ import annotations

from datetime import date
from pathlib import Path
import sys

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import (
    DEFAULT_COSTS,
    _batch_single,
    _normalize_compiler_reasons,
    _portfolio_sim,
    _validate_params,
    opening_buy_tradable_mask,
)
from app.strategy.compiler import compile_portfolio, compile_single
from app.strategy.overlays import (
    apply_portfolio_overlays,
    apply_single_overlays,
    portfolio_base_exit_reasons,
    single_entry_price_ceiling,
    single_entry_price_floor,
)
from app.strategy.presets import get_preset_spec
from app.strategy.strategies import (
    breakout,
    ma_cross,
    mean_reversion,
    momentum_rotation,
    multifactor_hold,
    volume_breakout,
)


SINGLE_CASES = [
    ("ma_cross", ma_cross, {"fast": 4, "slow": 18}, 123),
    ("breakout", breakout, {"entry": 15, "exit": 7}, 124),
    (
        "mean_reversion",
        mean_reversion,
        {"rsi_buy": 42, "rsi_sell": 58, "ma": 25},
        125,
    ),
]

PORTFOLIO_CASES = [
    (
        "momentum_rotation",
        momentum_rotation,
        {"top_n": 3, "w_mom20": 0.7, "w_mom60": 0.3},
    ),
    ("multifactor_hold", multifactor_hold, {"top_n": 3}),
]

TRADE_FACT_FIELDS = (
    "code",
    "signal_date",
    "execution_date",
    "execution_price",
    "size",
    "fees",
    "side",
    "tradable",
    "execution_status",
    "closed_trades",
    "winning_trades",
    "realized_pnl",
)


def _random_bars(seed: int, periods: int = 260, base: float = 30.0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.bdate_range(date(2023, 1, 2), periods=periods)
    close = base * np.exp(np.cumsum(rng.normal(0.0003, 0.025, periods)))
    open_ = close * (1 + rng.normal(0, 0.004, periods))
    high = np.maximum(open_, close) * (1 + rng.uniform(0.001, 0.018, periods))
    low = np.minimum(open_, close) * (1 - rng.uniform(0.001, 0.018, periods))
    volume = rng.integers(100_000, 3_000_000, periods).astype(float)
    return pd.DataFrame({
        "date": dates.date,
        "open": open_,
        "high": high,
        "low": low,
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": volume * close,
    })


def _volume_breakout_bars(window: int = 20) -> pd.DataFrame:
    dates = pd.bdate_range(date(2024, 1, 2), periods=window + 8)
    close = np.array(
        [10.0] * window + [10.5, 10.4, 10.3, 9.5, 9.4, 9.3, 9.2, 9.1],
    )
    volume = np.array(
        [1_000.0] * (window - 5) + [100.0] * 5 + [4_000.0] + [800.0] * 7,
    )
    frame = pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": close * 1.01,
        "low": close * 0.99,
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": volume * close,
    })
    frame.loc[:window - 1, "high"] = 10.1
    frame.loc[:window - 1, "low"] = 9.9
    return frame


def _portfolio_pool(periods: int = 260) -> tuple[pd.DatetimeIndex, dict[str, pd.DataFrame]]:
    dates = pd.bdate_range(date(2023, 1, 2), periods=periods)
    regimes = np.repeat(np.array([0.004, -0.003, 0.006, -0.001, 0.003, -0.004]), 44)
    regimes = regimes[:periods]
    pool: dict[str, pd.DataFrame] = {}
    for number in range(7):
        rng = np.random.default_rng(400 + number)
        daily_returns = np.roll(regimes, number * 9) + rng.normal(0, 0.012, periods)
        close = (15 + number) * np.exp(np.cumsum(daily_returns))
        open_ = close * (1 + rng.normal(0, 0.003, periods))
        volume = rng.integers(500_000, 3_000_000, periods).astype(float)
        pool[f"S{number}"] = pd.DataFrame({
            "date": dates.date,
            "open": open_,
            "high": np.maximum(open_, close) * 1.01,
            "low": np.minimum(open_, close) * 0.99,
            "close": close,
            "raw_close": close,
            "volume": volume,
            "amount": volume * close,
        })
    return dates, pool


def _position_values(series: pd.Series) -> pd.Series:
    return pd.Series(series.to_numpy(), dtype=int)


def _reason_codes_by_day(reasons: dict) -> dict:
    return {
        key: [item["code"] for item in items]
        for key, items in reasons.items()
    }


def _trade_facts(details: list[dict]) -> list[dict]:
    return [
        {key: item[key] for key in TRADE_FACT_FIELDS}
        for item in details
    ]


def _assert_simulation_evidence_equal(legacy: dict, compiled: dict) -> None:
    assert _trade_facts(compiled["trade_details"]) == _trade_facts(
        legacy["trade_details"],
    )
    assert compiled["exit_reason_distribution"] == legacy["exit_reason_distribution"]
    assert compiled["metrics"] == legacy["metrics"]
    assert sum(item["fees"] for item in compiled["trade_details"]) == pytest.approx(
        sum(item["fees"] for item in legacy["trade_details"]), abs=1e-12,
    )
    pd.testing.assert_series_equal(compiled["equity"], legacy["equity"])


@pytest.mark.parametrize(("name", "module", "overrides", "seed"), SINGLE_CASES)
def test_single_presets_match_legacy_transaction_evidence(
    name,
    module,
    overrides,
    seed,
):
    frame = _random_bars(seed)
    params = _validate_params(name, overrides)
    spec = get_preset_spec(name, params)
    dates = pd.DatetimeIndex(frame["date"])
    entry_tradable = opening_buy_tradable_mask({"X": frame}, dates)["X"]

    legacy_native = module.positions(frame, params)
    compiled_native = compile_single(spec, frame).positions
    pd.testing.assert_series_equal(
        _position_values(compiled_native),
        _position_values(legacy_native),
    )

    legacy_positions, legacy_reasons = apply_single_overlays(
        frame,
        legacy_native,
        params,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
        entry_price_floor=single_entry_price_floor(module, frame, params),
        entry_price_ceiling=single_entry_price_ceiling(module, frame, params),
    )
    compilation = compile_single(
        spec,
        frame,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
    )
    compiled_reasons = {
        day: _normalize_compiler_reasons(items)
        for day, items in compilation.reasons.items()
        if any(
            item.get("code") in {"native_exit", "risk_overlay", "take_profit"}
            for item in items
        )
    }

    pd.testing.assert_series_equal(
        _position_values(compilation.positions),
        _position_values(legacy_positions),
    )
    assert _reason_codes_by_day(compiled_reasons) == _reason_codes_by_day(
        legacy_reasons,
    )

    legacy = _batch_single(
        {"X": frame},
        {"X": legacy_positions},
        DEFAULT_COSTS,
        frame["date"].iat[0],
        exit_reasons_by_code={"X": legacy_reasons},
    )["X"]
    compiled = _batch_single(
        {"X": frame},
        {"X": compilation.positions},
        DEFAULT_COSTS,
        frame["date"].iat[0],
        exit_reasons_by_code={"X": compiled_reasons},
    )["X"]

    assert any(item["side"] == "sell" for item in legacy["trade_details"])
    _assert_simulation_evidence_equal(legacy, compiled)


def test_volume_breakout_preset_matches_legacy_transaction_evidence():
    name = "volume_breakout"
    module = volume_breakout
    frame = _volume_breakout_bars()
    params = _validate_params(name, {
        "window": 20,
        "range_max": 0.15,
        "vol_mult": 2.0,
        "atr_mult": 2.0,
    })
    spec = get_preset_spec(name, params)
    dates = pd.DatetimeIndex(frame["date"])
    entry_tradable = opening_buy_tradable_mask({"X": frame}, dates)["X"]

    legacy_native = module.positions(frame, params)
    compiled_native = compile_single(spec, frame).positions
    pd.testing.assert_series_equal(
        _position_values(compiled_native),
        _position_values(legacy_native),
    )

    legacy_positions, legacy_reasons = apply_single_overlays(
        frame,
        legacy_native,
        params,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
    )
    compilation = compile_single(
        spec,
        frame,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
    )
    compiled_reasons = {
        day: _normalize_compiler_reasons(items)
        for day, items in compilation.reasons.items()
        if any(
            item.get("code") in {"native_exit", "risk_overlay", "take_profit"}
            for item in items
        )
    }

    pd.testing.assert_series_equal(
        _position_values(compilation.positions),
        _position_values(legacy_positions),
    )
    assert _reason_codes_by_day(compiled_reasons) == _reason_codes_by_day(
        legacy_reasons,
    )

    legacy = _batch_single(
        {"X": frame},
        {"X": legacy_positions},
        DEFAULT_COSTS,
        frame["date"].iat[0],
        exit_reasons_by_code={"X": legacy_reasons},
    )["X"]
    compiled = _batch_single(
        {"X": frame},
        {"X": compilation.positions},
        DEFAULT_COSTS,
        frame["date"].iat[0],
        exit_reasons_by_code={"X": compiled_reasons},
    )["X"]

    assert legacy["exit_reason_distribution"]["by_primary"] == {"native": 1}
    _assert_simulation_evidence_equal(legacy, compiled)


@pytest.mark.parametrize(("name", "module", "overrides"), PORTFOLIO_CASES)
def test_portfolio_presets_match_legacy_transaction_evidence(
    name,
    module,
    overrides,
):
    dates, pool = _portfolio_pool()
    params = _validate_params(name, overrides)
    spec = get_preset_spec(name, params)
    entry_tradable = opening_buy_tradable_mask(pool, dates)

    legacy_native = module.target_weights(dates.date, pool, params)
    compiled_native = compile_portfolio(spec, dates.date, pool).weights
    pd.testing.assert_frame_equal(compiled_native, legacy_native)

    rebalance = module.rebalance_mask(dates)
    base_reasons = portfolio_base_exit_reasons(
        name,
        legacy_native,
        pool,
        rebalance,
    )
    legacy_weights, legacy_reasons, _ = apply_portfolio_overlays(
        legacy_native,
        pool,
        params,
        rebalance,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
        base_exit_reasons=base_reasons,
    )
    compilation = compile_portfolio(
        spec,
        dates.date,
        pool,
        slippage=DEFAULT_COSTS["slippage"],
        entry_tradable=entry_tradable,
    )
    compiled_reasons = {
        key: _normalize_compiler_reasons(items)
        for key, items in compilation.reasons.items()
        if any(item.get("type") in {"exit", "reduce"} for item in items)
    }

    pd.testing.assert_frame_equal(compilation.weights, legacy_weights)
    assert _reason_codes_by_day(compiled_reasons) == _reason_codes_by_day(
        legacy_reasons,
    )

    legacy = _portfolio_sim(
        legacy_weights,
        pool,
        dates,
        DEFAULT_COSTS,
        exit_reasons=legacy_reasons,
    )
    compiled = _portfolio_sim(
        compilation.weights,
        pool,
        dates,
        DEFAULT_COSTS,
        exit_reasons=compiled_reasons,
    )

    assert any(item["side"] == "sell" for item in legacy["trade_details"])
    pd.testing.assert_frame_equal(
        compiled["pf"].orders.records_readable,
        legacy["pf"].orders.records_readable,
    )
    _assert_simulation_evidence_equal(legacy, compiled)
