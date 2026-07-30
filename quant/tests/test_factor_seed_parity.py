"""种子因子表达式与旧 pandas 公式数值一致性回归测试。"""
from __future__ import annotations

import math
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.factors import evaluate_factor
from app.strategy.operators import compute_min_bars
from app.strategy.spec import parse_expression


def _make_synthetic_bars(seed: int = 42, days: int = 300) -> pd.DataFrame:
    """生成确定性随机游走日线,含一段零成交量(模拟停牌)。"""
    rng = np.random.default_rng(seed)
    n = days
    dates = pd.date_range("2023-01-01", periods=n, freq="B")
    # 随机游走收盘价
    log_returns = rng.normal(0.0005, 0.02, size=n)
    close = 10 * np.exp(np.cumsum(log_returns))
    # 日内高低价基于收盘价随机展开
    high = close * (1 + rng.uniform(0.00, 0.03, size=n))
    low = close * (1 - rng.uniform(0.00, 0.03, size=n))
    open_ = low + rng.uniform(0, 1, size=n) * (high - low)
    # 成交量随机,中间 20 天停牌
    volume = rng.integers(1_000_000, 10_000_000, size=n).astype(float)
    volume[n // 2 - 10:n // 2 + 10] = 0.0
    amount = volume * close * rng.uniform(0.9, 1.1, size=n)

    df = pd.DataFrame({
        "date": dates,
        "open": open_,
        "high": high,
        "low": low,
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": amount,
        "is_st": False,
    })
    return df


def _expected_mom20(close: pd.Series) -> pd.Series:
    return close / close.shift(20) - 1


def _expected_mom60(close: pd.Series) -> pd.Series:
    return close / close.shift(60) - 1


def _expected_rsi14(close: pd.Series) -> pd.Series:
    delta = close.diff()
    gain = delta.clip(lower=0)
    loss = -delta.clip(upper=0)
    avg_gain = gain.ewm(alpha=1 / 14, adjust=False).mean()
    avg_loss = loss.ewm(alpha=1 / 14, adjust=False).mean()
    rs = avg_gain / avg_loss
    return 100 - 100 / (1 + rs)


def _expected_atr_pct(high: pd.Series, low: pd.Series, close: pd.Series) -> pd.Series:
    prev_close = close.shift(1)
    tr1 = high - low
    tr2 = (high - prev_close).abs()
    tr3 = (low - prev_close).abs()
    tr = pd.concat([tr1, tr2, tr3], axis=1).max(axis=1)
    atr14 = tr.rolling(14).mean()
    return atr14 / close


def _expected_vol_ratio5(volume: pd.Series) -> pd.Series:
    denom = volume.shift(1).rolling(5).mean()
    return volume / denom.where(denom > 0)


def _expected_ma20_slope(close: pd.Series) -> pd.Series:
    ma20 = close.rolling(20).mean()
    return ma20 / ma20.shift(5) - 1


def _expected_amount_avg20(amount: pd.Series) -> pd.Series:
    return amount.rolling(20).mean()


SEED_EXPRESSIONS = {
    "mom20": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 20},
    "mom60": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 60},
    "rsi14": {"op": "rsi", "input": {"op": "field", "name": "close"}, "window": 14},
    "atr_pct": {
        "op": "divide",
        "left": {
            "op": "atr",
            "high": {"op": "field", "name": "high"},
            "low": {"op": "field", "name": "low"},
            "close": {"op": "field", "name": "close"},
            "window": 14,
        },
        "right": {"op": "field", "name": "close"},
    },
    "vol_ratio5": {
        "op": "volume_ratio",
        "input": {"op": "field", "name": "volume"},
        "window": 5,
        "shift": 1,
    },
    "ma20_slope": {
        "op": "subtract",
        "left": {
            "op": "divide",
            "left": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
            "right": {
                "op": "shift",
                "input": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
                "periods": 5,
            },
        },
        "right": {"op": "literal", "value": 1},
    },
    "amount_avg20": {
        "op": "rolling_mean",
        "input": {"op": "field", "name": "amount"},
        "window": 20,
        "shift": 0,
    },
}


@pytest.mark.parametrize("key,expected_fn", [
    ("mom20", lambda df: _expected_mom20(df["close"])),
    ("mom60", lambda df: _expected_mom60(df["close"])),
    ("rsi14", lambda df: _expected_rsi14(df["close"])),
    ("atr_pct", lambda df: _expected_atr_pct(df["high"], df["low"], df["close"])),
    ("vol_ratio5", lambda df: _expected_vol_ratio5(df["volume"])),
    ("ma20_slope", lambda df: _expected_ma20_slope(df["close"])),
    ("amount_avg20", lambda df: _expected_amount_avg20(df["amount"])),
])
def test_seed_factor_parity(key: str, expected_fn) -> None:
    df = _make_synthetic_bars()
    expr = SEED_EXPRESSIONS[key]
    actual = evaluate_factor(expr, df)
    expected = expected_fn(df)

    assert len(actual) == len(expected)
    actual_vals = actual.to_numpy(dtype=float)
    expected_vals = expected.to_numpy(dtype=float)

    for i, (a, e) in enumerate(zip(actual_vals, expected_vals)):
        if math.isnan(a) and math.isnan(e):
            continue
        assert a == pytest.approx(e, rel=1e-9, abs=1e-12), (
            f"{key} 位置 {i} 不一致: actual={a}, expected={e}"
        )


def test_seed_expression_min_bars_match_compute_min_bars() -> None:
    for key, expr in SEED_EXPRESSIONS.items():
        parsed = parse_expression(expr)
        computed = compute_min_bars(parsed)
        migration_min_bars = {
            "mom20": 21, "mom60": 61, "rsi14": 15, "atr_pct": 15,
            "vol_ratio5": 6, "ma20_slope": 25, "amount_avg20": 20,
        }[key]
        assert computed == migration_min_bars, (
            f"{key} 的 min_bars {computed} 与迁移种子 {migration_min_bars} 不一致"
        )
