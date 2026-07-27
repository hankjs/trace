"""统一风险/止盈覆盖层与回测证据测试。"""
from __future__ import annotations

from datetime import date, timedelta
from pathlib import Path
from types import SimpleNamespace
import sys

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import (
    DEFAULT_COSTS,
    _batch_single,
    _fee_assumptions,
    _portfolio_sim,
    run_backtest,
    run_sweep,
    validate_strategy_params,
)
from app.catalog import STRATEGY_TEMPLATES
from app.strategy.overlays import (apply_portfolio_overlays,
                                   apply_single_overlays,
                                   portfolio_base_exit_reasons,
                                   single_entry_price_ceiling,
                                   single_entry_price_floor)


def _bars(opens: list[float], closes: list[float] | None = None) -> pd.DataFrame:
    closes = closes or opens
    days = [date(2024, 1, 1) + timedelta(days=i) for i in range(len(opens))]
    return pd.DataFrame({
        "date": days,
        "open": opens,
        "high": [max(o, c) + 0.2 for o, c in zip(opens, closes)],
        "low": [min(o, c) - 0.2 for o, c in zip(opens, closes)],
        "close": closes,
        "raw_close": closes,
        "volume": [1_000_000.0] * len(opens),
        "amount": [10_000_000.0] * len(opens),
    })


def _params(**overrides) -> dict:
    params = validate_strategy_params("ma_cross", {})
    params.update(overrides)
    return params


def test_all_templates_publish_disabled_strict_overlay_contract():
    for template, metadata in STRATEGY_TEMPLATES.items():
        params = validate_strategy_params(template, {})
        assert params["risk_overlay"]["enabled"] is False
        assert params["take_profit"]["enabled"] is False
        assert metadata["plan_capability"]["overlays"]["risk_overlay"][
            "confirmation"
        ] == "T 日收盘确认，T+1 日开盘模拟退出"
        assert metadata["plan_capability"]["execution"]["real_execution"] == "外部手工确认"
        expected_type = "single" if metadata["kind"] == "single" else "portfolio_rebalance"
        assert metadata["plan_capability"]["plan_type"] == expected_type

    with pytest.raises(ValueError, match="未知字段"):
        validate_strategy_params("ma_cross", {
            "risk_overlay": {"enabled": True, "future": 1},
        })
    with pytest.raises(ValueError, match="enabled 必须是布尔值"):
        validate_strategy_params("ma_cross", {
            "risk_overlay": {"enabled": 1},
        })
    with pytest.raises(ValueError, match="fixed_pct 或 atr_multiple"):
        validate_strategy_params("ma_cross", {
            "take_profit": {"type": "price"},
        })
    with pytest.raises(ValueError, match="小数比例"):
        validate_strategy_params("ma_cross", {
            "risk_overlay": {"enabled": True, "value": 2.0},
        })
    with pytest.raises(ValueError, match="小数比例"):
        validate_strategy_params("ma_cross", {
            "take_profit": {"enabled": True, "value": 1.01},
        })


def test_single_overlay_uses_simulated_entry_and_preserves_all_exit_reasons():
    df = _bars([10, 10, 10, 10, 10], [10, 10, 8, 8, 8])
    native = pd.Series([0, 1, 1, 0, 0])
    params = _params(risk_overlay={
        "enabled": True, "type": "fixed_pct", "value": 0.10, "atr_period": 14,
    })

    positions, exits = apply_single_overlays(df, native, params, slippage=0.01)

    signal_day = pd.Timestamp(df["date"].iat[2])
    assert positions.tolist() == [0, 1, 0, 0, 0]
    assert [item["code"] for item in exits[signal_day]] == ["risk_overlay"]
    # 10 元 T+1 开盘模拟入场，加入 1% 买入滑点后，10% 风险线为 9.09。
    assert exits[signal_day][0]["price_line"] == pytest.approx(9.09)

    # 原生退出也在同一收盘日命中时，保存全部原因且风险优先。
    native.iloc[2] = 0
    simultaneous_positions, simultaneous = apply_single_overlays(
        df, native, params, slippage=0.01,
    )
    assert [item["code"] for item in simultaneous[signal_day]] == [
        "risk_overlay", "native",
    ]
    evidence = _batch_single(
        {"X": df}, {"X": simultaneous_positions}, DEFAULT_COSTS,
        df["date"].iat[0], exit_reasons_by_code={"X": simultaneous},
    )["X"]
    assert evidence["exit_reason_distribution"] == {
        "by_primary": {"risk_overlay": 1},
        "all_hits": {"risk_overlay": 1, "native": 1},
    }


def test_single_overlay_exposes_entry_based_lines_on_native_exit_day():
    df = _bars([10, 10, 10, 10])
    native = pd.Series([0, 1, 1, 0])
    params = _params(
        risk_overlay={
            "enabled": True, "type": "fixed_pct", "value": 0.1,
            "atr_period": 14,
        },
        take_profit={
            "enabled": True, "type": "fixed_pct", "value": 0.2,
            "atr_period": 14,
        },
    )
    state: dict = {}

    apply_single_overlays(df, native, params, state_out=state)

    assert state["simulated_entry_price"] == 10
    by_source = {rule["source"]: rule for rule in state["rules"]}
    assert by_source["risk_overlay"]["reference_line"] == 9
    assert by_source["take_profit"]["reference_line"] == 12


def test_single_overlay_requires_native_condition_reset_before_reentry():
    df = _bars([10] * 7, [10, 10, 8, 10, 10, 10, 11])
    native = pd.Series([0, 1, 1, 1, 0, 1, 1])
    params = _params(risk_overlay={
        "enabled": True, "type": "fixed_pct", "value": 0.10, "atr_period": 14,
    })

    positions, _ = apply_single_overlays(df, native, params)

    # 风险退出后原生条件仍为真期间保持空仓；先在 bar 4 失效，再由 bar 5
    # 的新上升沿重新产生 T+1 入场信号。
    assert positions.tolist() == [0, 1, 0, 0, 0, 1, 1]


def test_disabled_overlays_still_honor_blocked_simulated_entry():
    df = _bars([10] * 7)
    native = pd.Series([0, 1, 1, 1, 0, 1, 1])
    tradable = pd.Series(
        [True, True, False, True, True, True, True],
        index=pd.DatetimeIndex(df["date"]),
    )

    state: dict = {}
    positions, reasons = apply_single_overlays(
        df, native, _params(), entry_tradable=tradable, state_out=state,
    )

    # 首次 T+1 开盘受阻后不顺延；原生条件先归零再出现新上升沿才重新进入。
    assert positions.tolist() == [0, 1, 0, 0, 0, 1, 1]
    assert reasons == {}

    short_state: dict = {}
    apply_single_overlays(
        df.iloc[:3], pd.Series([0, 1, 0]), _params(),
        entry_tradable=tradable.iloc[:3], state_out=short_state,
    )
    assert short_state["entry_blocked"] is True


@pytest.mark.parametrize("execution_open", [9.5, 11.0])
def test_entry_observation_range_blocks_open_outside_its_bounds(execution_open):
    class BreakoutLike:
        @staticmethod
        def entry_observation_line(df, params):
            return pd.Series([9.0, 10.0, 10.0], index=df.index)

    df = _bars([10, 10, execution_open])
    native = pd.Series([0, 1, 1])
    params = _params(max_entry_premium=0.05)
    ceiling = single_entry_price_ceiling(BreakoutLike, df, params)
    floor = single_entry_price_floor(BreakoutLike, df, params)
    state: dict = {}

    positions, _ = apply_single_overlays(
        df, native, params,
        entry_price_floor=floor,
        entry_price_ceiling=ceiling,
        state_out=state,
    )

    assert ceiling is not None
    assert floor is not None
    assert floor.iat[1] == pytest.approx(10.0)
    assert ceiling.iat[1] == pytest.approx(10.5)
    assert positions.tolist() == [0, 1, 0]
    assert state["entry_blocked"] is True


def test_atr_overlay_uses_signal_day_atr_from_same_daily_bars():
    df = _bars([10] * 20, [10] * 16 + [8] + [8] * 3)
    native = pd.Series([0] * 15 + [1] * 5)
    params = _params(risk_overlay={
        "enabled": True, "type": "atr_multiple", "value": 2.0,
        "atr_period": 14,
    })

    positions, exits = apply_single_overlays(df, native, params)

    signal_day = pd.Timestamp(df["date"].iat[16])
    assert positions.iat[16] == 0
    assert exits[signal_day][0]["code"] == "risk_overlay"
    # 每日真实波幅为 0.4，2 ATR 风险线 = 10 - 0.8。
    assert exits[signal_day][0]["price_line"] == pytest.approx(9.2)


def test_take_profit_is_an_executed_exit_not_display_only():
    df = _bars([10, 10, 10, 10.81, 10.81, 10.81], [10, 10, 12, 12, 12, 12])
    native = pd.Series([0, 1, 1, 1, 1, 1])
    params = _params(take_profit={
        "enabled": True, "type": "fixed_pct", "value": 0.10,
        "atr_period": 14,
    })

    positions, reasons = apply_single_overlays(df, native, params)
    result = _batch_single(
        {"X": df}, {"X": positions}, DEFAULT_COSTS, df["date"].iat[0],
        exit_reasons_by_code={"X": reasons},
    )["X"]

    sell = next(item for item in result["trade_details"] if item["side"] == "sell")
    assert sell["primary_reason"]["code"] == "take_profit"
    assert result["exit_reason_distribution"]["by_primary"] == {"take_profit": 1}
    # 止盈只代表收盘条件命中；实际收益和胜率仍服从下一日可成交开盘价。
    assert sell["execution_price"] == pytest.approx(
        10.81 * (1 - DEFAULT_COSTS["slippage"]),
    )
    assert result["metrics"]["win_rate"] == 1.0


def test_single_limit_down_exit_retries_and_keeps_original_reasons():
    df = _bars(
        [10, 10, 10, 10, 9, 9, 9],
        [10, 10, 10, 10, 9, 9, 9],
    )
    native = pd.Series([0, 1, 1, 0, 0, 0, 0])
    signal_day = pd.Timestamp(df["date"].iat[3])
    reasons = {
        signal_day: [
            {"code": "risk_overlay", "name": "风险覆盖层"},
            {"code": "native", "name": "策略原生退出"},
        ],
    }

    result = _batch_single(
        {"X": df}, {"X": native}, DEFAULT_COSTS, df["date"].iat[0],
        exit_reasons_by_code={"X": reasons},
    )["X"]
    sell = next(item for item in result["trade_details"] if item["side"] == "sell")

    # d4 开盘恰好跌停，不成交；d5 开盘恢复后才模拟退出。
    assert sell["signal_date"] == str(signal_day.date())
    assert sell["execution_date"] == str(df["date"].iat[5])
    assert [item["code"] for item in sell["all_reasons"]] == [
        "risk_overlay", "native",
    ]
    assert result["metrics"]["trade_count"] == 2


def test_single_suspension_and_missing_bar_do_not_create_fake_sell():
    df = _bars([10] * 7)
    native = pd.Series([0, 1, 1, 0, 0, 0, 0])
    # 计划退出日 d4 停牌（零成交量），d5 也无有效开盘，d6 才恢复。
    df.loc[4, "volume"] = 0
    df.loc[5, "open"] = float("nan")

    result = _batch_single(
        {"X": df}, {"X": native}, DEFAULT_COSTS, df["date"].iat[0],
    )["X"]
    sell = next(item for item in result["trade_details"] if item["side"] == "sell")

    assert sell["signal_date"] == str(df["date"].iat[3])
    assert sell["execution_date"] == str(df["date"].iat[6])
    assert not any(
        item["side"] == "sell" and item["execution_date"] in {
            str(df["date"].iat[4]), str(df["date"].iat[5]),
        }
        for item in result["trade_details"]
    )


def test_portfolio_limit_down_sell_retries_with_original_overlay_reason():
    df = _bars(
        [10, 10, 10, 10, 9, 9, 9],
        [10, 10, 10, 10, 9, 9, 9],
    )
    idx = pd.DatetimeIndex(df["date"])
    weights = pd.DataFrame(
        {"X": [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]}, index=idx,
    )
    signal_day = idx[3]
    reasons = {
        (signal_day, "X"): [
            {"code": "risk_overlay", "name": "风险覆盖层"},
            {"code": "rebalance", "name": "组合调仓或资格变化"},
        ],
    }

    sim = _portfolio_sim(
        weights, {"X": df}, idx, DEFAULT_COSTS, exit_reasons=reasons,
    )
    sell = next(item for item in sim["trade_details"] if item["side"] == "sell")

    assert sell["signal_date"] == str(signal_day.date())
    assert sell["execution_date"] == str(idx[5].date())
    assert [item["code"] for item in sell["all_reasons"]] == [
        "risk_overlay", "rebalance",
    ]
    assert sim["metrics"]["trade_count"] == len(sim["trade_details"])


def test_portfolio_missing_bar_retries_exit_on_next_available_open():
    full = _bars([10] * 7)
    idx = pd.DatetimeIndex(full["date"])
    # X 在原计划成交日 d4 没有任何 bar；组合日历仍可能由其他成分股包含 d4。
    x_bars = full.drop(index=4).reset_index(drop=True)
    weights = pd.DataFrame(
        {"X": [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]}, index=idx,
    )

    sim = _portfolio_sim(weights, {"X": x_bars}, idx, DEFAULT_COSTS)
    sell = next(item for item in sim["trade_details"] if item["side"] == "sell")

    assert sell["signal_date"] == str(idx[3].date())
    assert sell["execution_date"] == str(idx[5].date())


def test_portfolio_limit_up_buy_is_dropped_and_not_indirectly_retried():
    a = _bars([10, 11, 11, 11, 11, 11], [10, 11, 11, 11, 11, 11])
    b = _bars([10] * 6)
    idx = pd.DatetimeIndex(a["date"])
    weights = pd.DataFrame({
        "A": [0.5] * 6,
        # B 后续权重变化会生成一整行组合目标；不能借机补买先前涨停失败的 A。
        "B": [0.5, 0.5, 0.5, 0.4, 0.4, 0.4],
    }, index=idx)

    sim = _portfolio_sim(weights, {"A": a, "B": b}, idx, DEFAULT_COSTS)
    orders = sim["pf"].orders.records_readable

    assert not (orders["Column"] == "A").any()
    assert (orders["Column"] == "B").any()


def test_portfolio_price_drift_cannot_buy_unchanged_target_at_limit_up():
    a = _bars([10, 10, 10, 10, 11, 11], [10, 10, 10, 10, 11, 11])
    b = _bars([10, 10, 10, 10, 20, 20], [10, 10, 10, 10, 20, 20])
    idx = pd.DatetimeIndex(a["date"])
    weights = pd.DataFrame({
        "A": [0.5] * 6,
        "B": [0.5, 0.5, 0.5, 0.4, 0.4, 0.4],
    }, index=idx)

    sim = _portfolio_sim(weights, {"A": a, "B": b}, idx, DEFAULT_COSTS)
    orders = sim["pf"].orders.records_readable

    assert not (
        (orders["Column"] == "A") & (orders["Timestamp"] == idx[4])
        & (orders["Side"] == "Buy")
    ).any()


def test_portfolio_price_drift_sell_retries_after_limit_down():
    a = _bars([10, 10, 10, 10, 9, 9], [10, 10, 10, 10, 9, 9])
    b = _bars([10, 10, 10, 10, 5, 5], [10, 10, 10, 10, 5, 5])
    idx = pd.DatetimeIndex(a["date"])
    weights = pd.DataFrame({
        "A": [0.5] * 6,
        "B": [0.5, 0.5, 0.5, 0.4, 0.4, 0.4],
    }, index=idx)

    sim = _portfolio_sim(weights, {"A": a, "B": b}, idx, DEFAULT_COSTS)
    sells = sim["pf"].orders.records_readable
    sells = sells[(sells["Column"] == "A") & (sells["Side"] == "Sell")]

    assert idx[4] not in set(sells["Timestamp"])
    assert idx[5] in set(sells["Timestamp"])


def test_portfolio_native_ma20_exit_is_not_labeled_as_rebalance():
    close = [10.0] * 20 + [8.0, 8.0]
    df = _bars(close, close)
    idx = pd.DatetimeIndex(df["date"])
    weights = pd.DataFrame({"X": [1.0] * 20 + [0.0, 0.0]}, index=idx)
    reasons = portfolio_base_exit_reasons(
        "momentum_rotation", weights, {"X": df},
        pd.Series([True] + [False] * 21, index=idx),
    )

    assert reasons[(idx[-2], "X")][0]["code"] == "native"
    sim = _portfolio_sim(
        weights, {"X": df}, idx, DEFAULT_COSTS, exit_reasons=reasons,
    )
    sell = next(item for item in sim["trade_details"] if item["side"] == "sell")
    assert sell["primary_reason"]["code"] == "native"


def test_portfolio_scheduled_rebalance_keeps_native_ma20_hit_too():
    close = [10.0] * 20 + [8.0, 8.0]
    df = _bars(close, close)
    idx = pd.DatetimeIndex(df["date"])
    weights = pd.DataFrame({"X": [1.0] * 20 + [0.0, 0.0]}, index=idx)
    rebalance = pd.Series(False, index=idx)
    rebalance.iat[-2] = True

    reasons = portfolio_base_exit_reasons(
        "momentum_rotation", weights, {"X": df}, rebalance,
    )

    assert [item["code"] for item in reasons[(idx[-2], "X")]] == [
        "native", "rebalance",
    ]


def test_portfolio_overlay_locks_until_next_planned_rebalance():
    df = _bars([10] * 8, [10, 10, 8, 10, 10, 10, 10, 10])
    idx = pd.DatetimeIndex(df["date"])
    weights = pd.DataFrame({"X": [1.0] * len(idx)}, index=idx)
    rebalance = pd.Series(
        [True, False, False, False, False, True, False, False], index=idx,
    )
    params = _params(risk_overlay={
        "enabled": True, "type": "fixed_pct", "value": 0.10, "atr_period": 14,
    })

    overlaid, reasons, snapshots = apply_portfolio_overlays(
        weights, {"X": df}, params, rebalance,
    )

    assert overlaid["X"].tolist() == [1, 1, 0, 0, 0, 1, 1, 1]
    assert reasons[(idx[2], "X")][0]["code"] == "risk_overlay"
    assert snapshots["X"]["rules"][0]["reference_line"] == 9.0
    assert snapshots["X"]["rules"][0]["data_date"] == str(idx[-1].date())

    sim = _portfolio_sim(
        overlaid, {"X": df}, idx, DEFAULT_COSTS, exit_reasons=reasons,
    )
    sell = next(item for item in sim["trade_details"] if item["side"] == "sell")
    assert sell["signal_date"] == str(idx[2].date())
    assert sell["execution_date"] == str(idx[3].date())
    assert sell["primary_reason"]["code"] == "risk_overlay"
    assert sim["metrics"]["trade_count"] == len(sim["trade_details"])


def test_portfolio_same_day_overlay_and_rebalance_keep_both_reasons():
    df = _bars([10] * 5, [10, 10, 8, 8, 8])
    idx = pd.DatetimeIndex(df["date"])
    weights = pd.DataFrame({"X": [1.0, 1.0, 0.0, 0.0, 0.0]}, index=idx)
    params = _params(risk_overlay={
        "enabled": True, "type": "fixed_pct", "value": 0.10, "atr_period": 14,
    })

    _, reasons, _ = apply_portfolio_overlays(
        weights, {"X": df}, params,
        pd.Series([True, False, True, False, False], index=idx),
    )

    assert [item["code"] for item in reasons[(idx[2], "X")]] == [
        "risk_overlay", "rebalance",
    ]


def test_trade_evidence_dates_reasons_and_metrics_share_actual_orders():
    df = _bars([10] * 7, [10, 10, 8, 8, 8, 8, 8])
    native = pd.Series([0, 1, 1, 0, 0, 0, 0])
    params = _params(risk_overlay={
        "enabled": True, "type": "fixed_pct", "value": 0.10, "atr_period": 14,
    })
    positions, reasons = apply_single_overlays(df, native, params)

    result = _batch_single(
        {"X": df}, {"X": positions}, DEFAULT_COSTS, df["date"].iat[0],
        exit_reasons_by_code={"X": reasons},
    )["X"]
    details = result["trade_details"]
    sell = next(item for item in details if item["side"] == "sell")

    assert sell["signal_date"] == str(df["date"].iat[2])
    assert sell["execution_date"] == str(df["date"].iat[3])
    assert sell["execution_price"] == pytest.approx(10 * (1 - DEFAULT_COSTS["slippage"]))
    assert sell["tradable"] is True
    assert sell["all_reasons"][0]["code"] == "risk_overlay"
    assert result["metrics"]["trade_count"] == len(details)
    assert result["metrics"]["round_trips"] == sum(
        item["closed_trades"] for item in details
    )
    assert result["metrics"]["win_rate"] == 0.0
    assert result["exit_reason_distribution"] == {
        "by_primary": {"risk_overlay": 1},
        "all_hits": {"risk_overlay": 1},
    }


def test_backtest_result_contains_reproducible_parameter_and_fee_snapshot(monkeypatch):
    closes = [10 + i * 0.05 for i in range(100)]
    df = _bars(closes)
    monkeypatch.setattr("app.backtest.engine.load_bars_df", lambda *args, **kwargs: df)
    strategy = SimpleNamespace(
        id=7, name="研究策略", template="ma_cross", params={"fast": 3, "slow": 8},
    )

    result = run_backtest(
        None, strategy, ["X"], df["date"].iat[20], df["date"].iat[-1],
        save=False,
    )

    assert result["params"] == result["parameter_snapshot"]
    assert result["params"]["risk_overlay"]["enabled"] is False
    assert result["params"]["take_profit"]["enabled"] is False
    assert result["fee_assumptions"] == _fee_assumptions(DEFAULT_COSTS)
    assert result["metrics"]["trade_count"] == len(result["trade_details"])
    assert "by_primary" in result["exit_reason_distribution"]


def test_atr_overlay_and_nested_parameter_scan_values_are_validated(monkeypatch):
    params = validate_strategy_params("breakout", {
        "take_profit": {
            "enabled": True, "type": "atr_multiple", "value": 2.5,
            "atr_period": 20,
        },
    })
    assert params["take_profit"] == {
        "enabled": True, "type": "atr_multiple", "value": 2.5,
        "atr_period": 20,
    }
    with pytest.raises(ValueError, match="ATR 倍数"):
        validate_strategy_params("breakout", {
            "take_profit": {"type": "atr_multiple", "value": 80},
        })

    df = _bars([10 + i * 0.05 for i in range(100)])
    monkeypatch.setattr("app.backtest.engine.load_bars_df", lambda *args, **kwargs: df)
    strategy = SimpleNamespace(
        id=8, name="覆盖层扫描", template="ma_cross", params={},
    )
    swept = run_sweep(
        None, strategy, ["X"], df["date"].iat[20], df["date"].iat[-1],
        {
            "risk_overlay.enabled": [True],
            "risk_overlay.value": [0.05, 0.10],
        },
    )
    assert len(swept["results"]) == 2
    assert {row["params"]["risk_overlay"]["value"]
            for row in swept["results"]} == {0.05, 0.10}
    assert all(row["params"]["risk_overlay"]["enabled"]
               for row in swept["results"])
