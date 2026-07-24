"""回测引擎回归测试(纯合成数据,不依赖数据库)。

覆盖评审修复:
1. 组合换仓 call_seq="auto":买单列先于卖单列时,A->B 轮换不会变成全现金;
2. 起点前已有持仓不丢失(单标的合成首日建仓 / 组合先 shift 再截断);
3. 印花税进入撮合:卖出单费率 = 佣金 + 印花税,净值口径一致。

运行: cd quant && uv run pytest tests/
"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import DEFAULT_COSTS, _batch_single, _portfolio_sim


def _mk_df(start: date, prices: list[float]) -> pd.DataFrame:
    dates = [start + timedelta(days=i) for i in range(len(prices))]
    return pd.DataFrame({
        "date": dates, "open": prices, "high": prices, "low": prices,
        "close": prices, "raw_close": prices,
        "volume": 1e6, "amount": 1e7,
    })


def test_rotation_buy_column_first_not_all_cash():
    """组合换仓:买单列(B)排在卖单列(A)之前,换仓后必须持有 B。"""
    start = date(2024, 1, 1)
    # 前 20 天两只都走平;第 20 天起 B 每天涨 2%(买到 B 才有收益)
    a_prices = [10.0] * 40
    b_prices = [10.0] * 20 + [10.0 * 1.02 ** i for i in range(1, 21)]
    # 刻意把 B 放在 A 前面:默认列序下买单先于卖单执行,修复前买单被拒
    dfs = {"B": _mk_df(start, b_prices), "A": _mk_df(start, a_prices)}
    idx = pd.DatetimeIndex(dfs["B"]["date"])
    w = pd.DataFrame(0.0, index=idx, columns=["B", "A"])
    w.loc[idx[:20], "A"] = 1.0
    w.loc[idx[20:], "B"] = 1.0

    sim = _portfolio_sim(w, dfs, idx, DEFAULT_COSTS)
    # 修复前:买单现金不足被拒 -> 全现金 -> 换仓后收益为 0
    assert sim["metrics"]["total_return"] > 0.1


def test_prestart_position_single():
    """单标的:回测起点前仓位已为 1,首日合成建仓,收益不为 0。"""
    start = date(2024, 1, 1)
    df = _mk_df(start, list(np.linspace(10.0, 12.0, 60)))
    pos = pd.Series(1, index=df.index)  # 全程满仓
    bt_start = df["date"].iloc[30]

    res = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS, bt_start)
    m = res["X"]["metrics"]
    # 修复前:截断后 diff 无跳变 -> 无入场 -> 收益恒 0
    assert m["total_return"] > 0.01
    assert m["trade_count"] == 1  # 仅首日一笔买入


def test_prestart_position_portfolio():
    """组合:起点前已有 50/50 权重,首日即建仓而不是空仓等下个调仓点。"""
    start = date(2024, 1, 1)
    dfs = {
        "A": _mk_df(start, list(np.linspace(10.0, 11.0, 60))),
        "B": _mk_df(start, list(np.linspace(20.0, 23.0, 60))),
    }
    full_idx = pd.DatetimeIndex(dfs["A"]["date"])
    w = pd.DataFrame(0.5, index=full_idx, columns=["A", "B"])  # 全程 50/50
    bt_idx = full_idx[30:]

    sim = _portfolio_sim(w, dfs, bt_idx, DEFAULT_COSTS)
    assert sim["metrics"]["total_return"] > 0.02
    assert sim["metrics"]["trade_count"] >= 2  # 首日两只都建仓


def test_stamp_tax_in_simulation():
    """印花税按卖出单进入撮合:有税/无税收益差 ≈ 印花税率。"""
    start = date(2024, 1, 1)
    df = _mk_df(start, [10.0] * 40)
    pos = pd.Series([1] * 20 + [0] * 20, index=df.index)  # 一买一卖
    bt_start = df["date"].iloc[0]

    taxed = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS, bt_start)
    free = _batch_single({"X": df}, {"X": pos},
                         {**DEFAULT_COSTS, "stamp_tax": 0.0}, bt_start)
    diff = (free["X"]["metrics"]["total_return"]
            - taxed["X"]["metrics"]["total_return"])
    assert diff == pytest.approx(DEFAULT_COSTS["stamp_tax"], abs=2e-4)
    assert taxed["X"]["metrics"]["trade_count"] == 2
    # 平价一买一卖必亏(双边佣金+滑点+印花税),且胜率口径来自同一模拟
    assert taxed["X"]["metrics"]["total_return"] < 0
    assert taxed["X"]["metrics"]["win_rate"] == 0.0
