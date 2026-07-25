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

from app.backtest.engine import (
    DEFAULT_COSTS,
    MIN_STAT_BARS,
    TRADING_DAYS,
    _batch_single,
    _combo_metrics,
    _equity_statistics,
    _held_before,
    _limit_pct,
    _portfolio_sim,
    _sharpe_ratio,
    _validate_costs,
    _validate_params,
)
from app.strategy.strategies import momentum_rotation


def _mk_df(start: date, prices: list[float]) -> pd.DataFrame:
    dates = [start + timedelta(days=i) for i in range(len(prices))]
    return pd.DataFrame({
        "date": dates, "open": prices, "high": prices, "low": prices,
        "close": prices, "raw_close": prices,
        "volume": 1e6, "amount": 1e7,
    })


def _mk_df_ohlc(start: date, opens: list[float],
                closes: list[float]) -> pd.DataFrame:
    """open 与 close 可不同的合成行情(用于暴露"当日开盘成交当日收盘信号")。"""
    dates = [start + timedelta(days=i) for i in range(len(closes))]
    return pd.DataFrame({
        "date": dates, "open": opens,
        "high": [max(o, c) for o, c in zip(opens, closes)],
        "low": [min(o, c) for o, c in zip(opens, closes)],
        "close": closes, "raw_close": closes,
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
    # 起点取 bar 1:bar 0 已持仓 -> 属"起点前已持仓",首日合成建仓。
    # (起点若取 bar 0 则起点前无历史,不存在已持仓的证据,不该成交)
    bt_start = df["date"].iloc[1]

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


def test_initial_entry_cost_is_in_total_return_and_drawdown():
    start = date(2024, 1, 1)
    df = _mk_df(start, [10.0] * 40)
    # 同上:起点取 bar 1,bar 0 的持仓构成"起点前已持仓"的证据
    res = _batch_single(
        {"X": df}, {"X": pd.Series([1] * 40)}, DEFAULT_COSTS,
        df["date"].iloc[1],
    )["X"]

    assert res["metrics"]["total_return"] == pytest.approx(-0.00035, abs=1e-4)
    # D1 新口径:回撤是净值自身峰谷。首日建仓成本是一次性台阶下移,
    # 之后再无从峰值回落,故回撤≈0(旧口径以初始资金 1.0 播种峰值报 -0.00035)
    assert res["metrics"]["max_drawdown"] == pytest.approx(0.0, abs=1e-9)
    assert res["equity"].iat[0] < 1.0


def test_no_synthetic_entry_without_prior_bar():
    """起点前完全无历史时不合成建仓(与 _portfolio_sim 的 shift 口径一致)。

    _portfolio_sim 在 bt_idx[0] == weights.index[0] 时 shift 出 NaN,当日不下单;
    单标的路径同理:没有"起点前已持仓"的证据就不该凭空成交。
    """
    start = date(2024, 1, 1)
    df = _mk_df(start, [10.0] * 40)
    res = _batch_single(
        {"X": df}, {"X": pd.Series([1] * 40)}, DEFAULT_COSTS, start,
    )["X"]

    assert res["metrics"]["trade_count"] == 0
    assert res["metrics"]["total_return"] == pytest.approx(0.0, abs=1e-12)


def test_costs_and_strategy_params_are_validated():
    with pytest.raises(ValueError, match="commission"):
        _validate_costs({"commission": -0.01})
    with pytest.raises(ValueError, match="未知费用"):
        _validate_costs({"rebate": 0.01})
    with pytest.raises(ValueError, match="fast 必须小于 slow"):
        _validate_params("ma_cross", {"fast": 20, "slow": 5})
    with pytest.raises(ValueError, match="不支持参数"):
        _validate_params("ma_cross", {"future_window": 3})


def test_portfolio_stamp_tax_is_applied_to_actual_sell_order():
    start = date(2024, 1, 1)
    dfs = {
        "A": _mk_df(start, [10.0] * 40),
        "B": _mk_df(start, [10.0] * 40),
    }
    idx = pd.DatetimeIndex(dfs["A"]["date"])
    weights = pd.DataFrame(0.0, index=idx, columns=["A", "B"])
    weights.loc[idx[:20], "A"] = 1.0
    weights.loc[idx[20:], "B"] = 1.0

    sim = _portfolio_sim(weights, dfs, idx, DEFAULT_COSTS)
    orders = sim["pf"].orders.records_readable
    sell = orders[orders["Side"] == "Sell"].iloc[0]

    assert sell["Fees"] / (sell["Size"] * sell["Price"]) == pytest.approx(
        DEFAULT_COSTS["commission"] + DEFAULT_COSTS["stamp_tax"], rel=1e-6,
    )


def test_dynamic_eligibility_excludes_inactive_stock_from_ranking():
    start = date(2024, 1, 1)
    n = 80
    dfs = {
        "inactive": _mk_df(start, [10 + i * 0.2 for i in range(n)]),
        "active": _mk_df(start, [10 + i * 0.1 for i in range(n)]),
    }
    idx = pd.DatetimeIndex(dfs["active"]["date"])
    eligibility = pd.DataFrame(
        {"inactive": [False] * n, "active": [True] * n}, index=idx,
    )

    weights = momentum_rotation.target_weights(
        list(dfs["active"]["date"]), dfs, {"top_n": 1},
        eligibility=eligibility,
    )

    assert weights.iloc[-1].to_dict() == {"inactive": 0.0, "active": 1.0}


# ---------------------------------------------------------------------------
# §3.3 P0:提前一天建仓(lookahead / entry_before_start)
#
# 复现场景(brief §3.3 给的权威证据):
#   bar 29 之前仓位 0、bar 30 起为 1;bar 30 open=10 close=20。
#   窗口起点正好落在 bar 30 时,旧实现看"窗口首日仓位==1"就用 bar 30 的
#   **开盘价 10** 建仓,而这个 1 是 bar 30 **收盘价 20** 算出来的信号 ——
#   等于当天开盘就知道了当天要涨 100%,total_return 虚增到 ≈0.9993。
#   修好后 bar 30 不该成交,建仓要等 T+1(bar 31),total_return≈-0.0003。
# ---------------------------------------------------------------------------

_LOOKAHEAD_BARS = 60
_FLIP_AT = 30  # 仓位由 0 翻 1 的那根 bar


def _lookahead_fixture() -> tuple[pd.DataFrame, pd.Series, date]:
    """构造上述场景:翻仓那根 bar 当天暴涨(open=10 close=20),其余走平。"""
    start = date(2024, 1, 1)
    opens = [10.0] * _LOOKAHEAD_BARS
    closes = [10.0] * _LOOKAHEAD_BARS
    closes[_FLIP_AT] = 20.0          # 翻仓日收盘暴涨(信号就是它算出来的)
    for i in range(_FLIP_AT + 1, _LOOKAHEAD_BARS):
        opens[i] = 20.0              # 之后价格停在 20,T+1 建仓买不到便宜货
        closes[i] = 20.0
    df = _mk_df_ohlc(start, opens, closes)
    pos = pd.Series([0] * _FLIP_AT + [1] * (_LOOKAHEAD_BARS - _FLIP_AT),
                    index=df.index)
    return df, pos, df["date"].iloc[_FLIP_AT]


def test_no_entry_before_start_on_same_day_flip():
    """窗口首日恰好由 0 翻 1:不得用当日开盘价成交当日收盘才产生的信号。"""
    df, pos, bt_start = _lookahead_fixture()

    m = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS, bt_start)["X"]["metrics"]

    # 修复前:0.9993(open=10 买入 -> close=20,凭空一倍)
    assert m["total_return"] != pytest.approx(0.9993, abs=1e-3)
    # 修复后:首日不成交,T+1 以 open=20 建仓后走平,只剩建仓成本
    assert m["total_return"] == pytest.approx(-0.00035, abs=1e-4)
    assert m["trade_count"] == 1


def test_entry_before_start_still_synthesised_when_truly_held():
    """起点前一根 bar 已持仓:仍要合成首日建仓(修 P0 不能把真持仓丢掉)。"""
    df, pos, _ = _lookahead_fixture()
    # 起点推后一根:bar 30 已持仓,bar 31 起点 -> 属于"起点前已持仓"
    bt_start = df["date"].iloc[_FLIP_AT + 1]

    res = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS, bt_start)["X"]
    # 首日 open=20 合成建仓,之后走平 -> 只有建仓成本,但必须真的建了仓
    assert res["metrics"]["trade_count"] == 1
    assert res["equity"].iat[0] < 1.0


def test_batch_single_window_start_matches_earlier_start_prefix():
    """同一策略同一数据,窗口起点前移不应改变"起点当日是否成交"的判定。

    起点落在翻仓日(bar 30)与起点更早(bar 20)时,bar 30 都不该有成交:
    修复前前者 total_return≈0.9993、后者≈-0.0003,两者矛盾正是 P0 的指纹。
    """
    df, pos, flip_day = _lookahead_fixture()

    at_flip = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS, flip_day)["X"]
    earlier = _batch_single(
        {"X": df}, {"X": pos}, DEFAULT_COSTS, df["date"].iloc[20],
    )["X"]

    assert at_flip["metrics"]["total_return"] == pytest.approx(
        earlier["metrics"]["total_return"], abs=1e-6,
    )
    assert at_flip["metrics"]["trade_count"] == earlier["metrics"]["trade_count"]


def test_held_before_reads_the_bar_preceding_start():
    """_held_before 直接单测:看的必须是 start 之前那根 bar。"""
    idx = pd.DatetimeIndex([date(2024, 1, 1) + timedelta(days=i) for i in range(4)])
    # 前两天空仓,后两天持仓
    pos = pd.Series([0.0, 0.0, 1.0, 1.0], index=idx)

    assert _held_before(pos, idx[2]) is False   # 翻仓当日:起点前是 0
    assert _held_before(pos, idx[3]) is True    # 起点前一根已是 1
    assert _held_before(pos, idx[0]) is False   # 无起点前历史


# ---------------------------------------------------------------------------
# §3.4 指标口径:242 年化基数 / 真峰谷回撤 / 无风险利率 / 短序列
# ---------------------------------------------------------------------------


def test_annual_return_uses_242_trading_days():
    """242 bar 涨 20% -> 年化 ≈0.20(252 基数下报 0.2091)。"""
    eq = pd.Series(np.linspace(1.0, 1.2, TRADING_DAYS),
                   index=pd.date_range("2024-01-01", periods=TRADING_DAYS))

    _, annual, _, _ = _equity_statistics(eq)

    assert annual == pytest.approx(0.20, abs=1e-6)
    # 旧的 252 基数会把同一序列报成 0.2091
    assert annual != pytest.approx(0.2091, abs=1e-4)


def test_max_drawdown_is_true_peak_to_trough():
    """净值 [0.5,0.4] 的真实峰谷是 -0.20,旧口径从初始资金起算报 -0.6。"""
    eq = pd.Series([0.5, 0.4], index=pd.date_range("2024-01-01", periods=2))

    _, _, max_dd, _ = _equity_statistics(eq)

    assert max_dd == pytest.approx(-0.20, abs=1e-9)
    assert max_dd != pytest.approx(-0.6, abs=1e-3)


def test_max_drawdown_measures_from_the_highest_peak():
    """涨到 2.0 再跌到 1.5:回撤 -0.25(从峰值 2.0 起算,不是从初始 1.0)。"""
    eq = pd.Series([1.0, 2.0, 1.5], index=pd.date_range("2024-01-01", periods=3))

    _, _, max_dd, _ = _equity_statistics(eq)

    assert max_dd == pytest.approx(-0.25, abs=1e-9)


def test_short_series_returns_none_for_annualised_metrics():
    """短于 20 bar 的序列:年化与夏普返回 None,不再报 252/3 次方的天文数字。"""
    eq = pd.Series([1.0, 1.05, 1.1], index=pd.date_range("2024-01-01", periods=3))

    total, annual, max_dd, rets = _equity_statistics(eq)

    assert annual is None
    assert total == pytest.approx(0.10, abs=1e-9)   # 总收益仍然给
    assert max_dd == pytest.approx(0.0, abs=1e-9)
    assert _sharpe_ratio(rets) is None
    # 边界:恰好 20 bar 时开始给年化
    eq20 = pd.Series(np.linspace(1.0, 1.1, MIN_STAT_BARS),
                     index=pd.date_range("2024-01-01", periods=MIN_STAT_BARS))
    assert _equity_statistics(eq20)[1] is not None


def test_empty_equity_series_raises_instead_of_indexerror():
    """空序列给出明确错误,而不是裸 IndexError。"""
    with pytest.raises(ValueError, match="净值序列为空"):
        _equity_statistics(pd.Series(dtype=float))


def test_sharpe_subtracts_risk_free_rate():
    """夏普引入无风险利率:rf>0 时必须低于 rf=0 的信息比率。"""
    rng = np.random.default_rng(42)
    rets = pd.Series(rng.normal(0.001, 0.01, 120),
                     index=pd.date_range("2024-01-01", periods=120))

    zero_rf = _sharpe_ratio(rets, 0.0)
    with_rf = _sharpe_ratio(rets, 0.03)

    assert zero_rf is not None and with_rf is not None
    assert with_rf < zero_rf
    # 差额 = (rf/242)/std*sqrt(242),可解析验证
    expected_gap = (0.03 / TRADING_DAYS) / float(rets.std()) * np.sqrt(TRADING_DAYS)
    assert zero_rf - with_rf == pytest.approx(expected_gap, abs=1e-4)


def test_combo_win_rate_is_weighted_by_trade_count():
    """组合胜率按回合交易数加权:2 笔全胜不该把 100 笔 40% 的整体拉到 70%。"""
    per_code = {
        "few":  {"win_rate": 1.0, "round_trips": 2, "trade_count": 4},
        "many": {"win_rate": 0.4, "round_trips": 98, "trade_count": 196},
    }
    combo = pd.Series(np.linspace(1.0, 1.1, 30),
                      index=pd.date_range("2024-01-01", periods=30))

    m = _combo_metrics(combo, per_code)

    # 加权:(1.0*2 + 0.4*98)/100 = 0.412;算术平均会报 0.70
    assert m["win_rate"] == pytest.approx(0.412, abs=1e-4)
    assert m["win_rate"] != pytest.approx(0.70, abs=1e-3)


# ---------------------------------------------------------------------------
# §3.9 涨跌停可成交性
# ---------------------------------------------------------------------------


def test_entry_blocked_when_opening_at_limit_up():
    """入场当日开盘一字涨停:该笔不成交(丢弃不顺延)。"""
    start = date(2024, 1, 1)
    n = 40
    opens = [10.0] * n
    closes = [10.0] * n
    # bar 19 收盘暴涨触发信号 -> bar 20 入场,而 bar 20 开盘一字涨停
    closes[19] = 11.0
    for i in range(20, n):
        opens[i] = 12.1   # 相对前收 11.0 涨 10%,一字板
        closes[i] = 12.1
    df = _mk_df_ohlc(start, opens, closes)
    pos = pd.Series([0] * 19 + [1] * (n - 19), index=df.index)

    res = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS,
                        df["date"].iloc[1])["X"]

    assert res["metrics"]["trade_count"] == 0  # 买不进
    assert res["metrics"]["total_return"] == pytest.approx(0.0, abs=1e-12)


def test_entry_allowed_when_open_below_limit_up():
    """同样的信号,开盘只涨 5%(未涨停)时必须正常成交——不能一律拦掉。"""
    start = date(2024, 1, 1)
    n = 40
    opens = [10.0] * n
    closes = [10.0] * n
    closes[19] = 11.0
    for i in range(20, n):
        opens[i] = 11.55   # 相对前收 11.0 涨 5%
        closes[i] = 11.55
    df = _mk_df_ohlc(start, opens, closes)
    pos = pd.Series([0] * 19 + [1] * (n - 19), index=df.index)

    res = _batch_single({"X": df}, {"X": pos}, DEFAULT_COSTS,
                        df["date"].iloc[1])["X"]

    assert res["metrics"]["trade_count"] == 1


def test_limit_pct_is_20_for_chinext_and_star_board():
    """创业板/科创板 20%,主板 10%:同一 15% 开盘缺口只拦主板。"""
    assert _limit_pct("sz.300750") == 0.20
    assert _limit_pct("sh.688111") == 0.20
    assert _limit_pct("sh.600519") == 0.10
    assert _limit_pct("sz.000001") == 0.10
