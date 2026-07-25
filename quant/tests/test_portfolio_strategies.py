"""组合策略的公共契约测试，保护后续 Top-N 调仓函数抽取。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.strategies import momentum_rotation, multifactor_hold
from app.strategy.rebalance import top_n_rebalance_weights


PORTFOLIO_STRATEGIES = [momentum_rotation, multifactor_hold]


def _bars(dates: pd.DatetimeIndex, daily_return: float) -> pd.DataFrame:
    prices = [10.0 * (1 + daily_return) ** i for i in range(len(dates))]
    return pd.DataFrame({
        "date": dates.date,
        "open": prices,
        "high": prices,
        "low": prices,
        "close": prices,
        "raw_close": prices,
        "volume": 1_000_000.0,
        "amount": 10_000_000.0,
    })


@pytest.mark.parametrize("strategy", PORTFOLIO_STRATEGIES,
                         ids=lambda strategy: strategy.NAME)
def test_portfolio_strategy_respects_daily_eligibility(strategy):
    """持仓标的退出动态池后必须当日清零，不能等到下个调仓日。"""
    dates = pd.bdate_range(date(2024, 1, 1), periods=110)
    pool = {
        "fast": _bars(dates, 0.010),
        "slow": _bars(dates, 0.004),
    }
    eligibility = pd.DataFrame(True, index=dates, columns=pool)
    eligibility.loc[dates[80]:, "fast"] = False

    weights = strategy.target_weights(
        dates.date, pool, {"top_n": 1}, eligibility=eligibility,
    )

    assert weights.loc[dates[75], "fast"] == pytest.approx(1.0)
    assert weights.loc[dates[80], "fast"] == pytest.approx(0.0)
    assert (weights.loc[dates[80]:, "fast"] == 0.0).all()


@pytest.mark.parametrize("strategy", PORTFOLIO_STRATEGIES,
                         ids=lambda strategy: strategy.NAME)
def test_portfolio_strategy_is_long_only_and_never_exceeds_budget(strategy):
    dates = pd.bdate_range(date(2024, 1, 1), periods=110)
    pool = {
        "a": _bars(dates, 0.010),
        "b": _bars(dates, 0.008),
        "c": _bars(dates, 0.004),
    }

    weights = strategy.target_weights(dates.date, pool, {"top_n": 2})

    assert list(weights.index) == list(dates)
    assert list(weights.columns) == list(pool)
    assert (weights >= 0.0).all().all()
    assert (weights.sum(axis=1) <= 1.0 + 1e-12).all()
    assert weights.iloc[-1].gt(0).sum() == 2


def test_momentum_rotation_applies_daily_ma20_risk_filter():
    dates = pd.bdate_range(date(2024, 1, 1), periods=100)
    prices = [10.0 * 1.01 ** i for i in range(90)]
    prices.extend([prices[-1] * 0.5] * 10)
    bars = _bars(dates, 0.0)
    for column in ("open", "high", "low", "close", "raw_close"):
        bars[column] = prices

    weights = momentum_rotation.target_weights(
        dates.date, {"falling": bars}, {"top_n": 1},
    )

    assert weights.loc[dates[85], "falling"] == pytest.approx(1.0)
    assert weights.loc[dates[90], "falling"] == pytest.approx(0.0)


def test_risk_filter_is_temporary_but_eligibility_exit_persists():
    dates = pd.bdate_range(date(2024, 1, 1), periods=5)
    scores = pd.DataFrame({"a": [2.0] * 5, "b": [1.0] * 5}, index=dates)
    eligibility = pd.DataFrame(True, index=dates, columns=scores.columns)
    eligibility.loc[dates[3], "a"] = False
    blocked = pd.DataFrame(False, index=dates, columns=scores.columns)
    blocked.loc[dates[1], "a"] = True

    weights = top_n_rebalance_weights(
        scores,
        [True, False, False, False, False],
        1,
        eligibility=eligibility,
        risk_blocked=blocked,
    )

    assert weights.loc[dates[0], "a"] == pytest.approx(1.0)
    assert weights.loc[dates[1], "a"] == pytest.approx(0.0)
    assert weights.loc[dates[2], "a"] == pytest.approx(1.0)
    assert weights.loc[dates[3], "a"] == pytest.approx(0.0)
    assert weights.loc[dates[4], "a"] == pytest.approx(0.0)
