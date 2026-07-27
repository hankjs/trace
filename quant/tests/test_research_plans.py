"""版本化策略研究计划的模板语义、状态、持久化和 API 契约。"""
from __future__ import annotations

import sys
from datetime import date, datetime
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.signals import list_signals
from app.backtest.engine import DEFAULT_COSTS, validate_strategy_params
from app.db import Base
from app.models import (BacktestRun, DailyBar, Pool, ResearchPlan,
                        ResearchPlanItem, Signal, Snapshot, Stock, Strategy,
                        SYSTEM_OWNER_ID, TradeCalendar)
from app.research_plan.domain import (PRODUCT_BOUNDARY,
                                      build_portfolio_snapshot,
                                      build_single_snapshot,
                                      evaluate_single_entry_condition,
                                      parameter_snapshot, strategy_version)
from app.research_plan.service import (create_portfolio_plan,
                                       create_single_plan, effective_status,
                                       next_trading_day, plan_detail,
                                       plan_summary)
import app.research_plan.pipeline as plan_pipeline

USER = "11111111-1111-1111-1111-111111111111"
CLAIMS = {"sub": USER, "username": "researcher", "can_client": True}


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _bars(*, end: str = "2026-07-24", periods: int = 120,
          daily_return: float = 0.002) -> pd.DataFrame:
    dates = pd.bdate_range(end=end, periods=periods)
    close = [10.0 * (1 + daily_return) ** i for i in range(periods)]
    volume = [1_000_000.0] * periods
    return pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": [value * 1.01 for value in close],
        "low": [value * 0.99 for value in close],
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": [value * qty for value, qty in zip(close, volume)],
    })


def _strategy(template: str, kind: str = "single", params: dict | None = None,
              *, strategy_id: int = 1) -> Strategy:
    return Strategy(
        id=strategy_id, owner_id=USER, is_system=False,
        name=f"研究-{template}", template=template, kind=kind,
        params=params or {}, enabled=True,
    )


def _frame(close: list[float], *, volume: list[float] | None = None,
           high: list[float] | None = None,
           low: list[float] | None = None,
           end: str = "2026-07-24") -> pd.DataFrame:
    dates = pd.bdate_range(end=end, periods=len(close))
    volume = volume or [1_000_000.0] * len(close)
    high = high or [value + 0.1 for value in close]
    low = low or [value - 0.1 for value in close]
    return pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": high,
        "low": low,
        "close": close,
        "raw_close": close,
        "volume": volume,
        "amount": [value * qty for value, qty in zip(close, volume)],
    })


def _daily_bar_rows(code: str, frame: pd.DataFrame) -> list[DailyBar]:
    return [DailyBar(code=code, **row) for row in frame.to_dict("records")]


def _native_condition_case(template: str) -> tuple[dict, pd.DataFrame, pd.DataFrame]:
    if template == "ma_cross":
        initial = _frame([10 + index * 0.1 for index in range(40)])
        return {}, initial, _frame([5.0], end="2026-07-27")
    if template == "breakout":
        initial = _frame([10.0] * 29 + [11.0])
        return {}, initial, _frame([8.0], end="2026-07-27")
    if template == "mean_reversion":
        close = [10 + index * 0.2 for index in range(80)]
        close += [close[-1] - (index + 1) * 0.3 for index in range(13)]
        initial = _frame(close)
        return {}, initial, _frame([30.0], end="2026-07-27")
    if template == "volume_breakout":
        close = [10.0] * 29 + [10.2]
        volume = [1_000_000.0] * 24 + [500_000.0] * 5 + [3_000_000.0]
        high = [10.1] * 29 + [10.3]
        low = [9.9] * 30
        initial = _frame(close, volume=volume, high=high, low=low)
        return {}, initial, _frame([8.0], end="2026-07-27")
    raise AssertionError(template)


@pytest.mark.parametrize(
    ("template", "expected_kind"),
    [
        ("ma_cross", "none"),
        ("breakout", "line"),
        ("mean_reversion", "none"),
        ("volume_breakout", "line"),
    ],
)
def test_four_single_templates_keep_their_real_observation_semantics(
    template, expected_kind,
):
    bars = _bars()
    plan = build_single_snapshot(
        _strategy(template), bars, side="watch",
        data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27),
    )

    observation = plan["entry_observation"]
    assert observation["kind"] == expected_kind
    # 未配置溢价容忍时只允许客观观察线，不能伪造上下界。
    assert "lower" not in observation
    assert "upper" not in observation
    assert plan["params_snapshot"]["risk_overlay"]["enabled"] is False
    assert plan["take_profit"]["enabled"] is False
    assert plan["take_profit"]["explanation"].startswith("未设置止盈")


def test_volume_breakout_plan_uses_the_templates_actual_native_risk_line():
    strategy = _strategy("volume_breakout")
    _, entry_frame, _ = _native_condition_case("volume_breakout")
    holding_frame = pd.concat([
        entry_frame,
        _frame([15.0], end="2026-07-27"),
    ], ignore_index=True)

    holding = build_single_snapshot(
        strategy, holding_frame, side="hold", data_date=date(2026, 7, 27),
        next_execution_date=date(2026, 7, 28),
    )
    atr_rule = next(
        rule for rule in holding["native_exit"]
        if rule["name"] == "模板 ATR 风险退出"
    )

    # 风险线仍以原入场信号价为基准，不能随 15 元的当日收盘价漂到其附近。
    assert atr_rule["reference_line"] is not None
    assert atr_rule["reference_line"] < 11

    watch = build_single_snapshot(
        strategy, _bars(), side="watch", data_date=date(2026, 7, 24),
        next_execution_date=date(2026, 7, 27),
    )
    watch_atr_rule = next(
        rule for rule in watch["native_exit"]
        if rule["name"] == "模板 ATR 风险退出"
    )
    assert watch_atr_rule["reference_line"] is None


def test_enabled_overlays_are_snapshot_but_no_price_is_fabricated_before_fill():
    bars = _bars()
    strategy = _strategy("breakout", params={
        "risk_overlay": {"enabled": True, "type": "fixed_pct", "value": 0.08},
        "take_profit": {"enabled": True, "type": "atr_multiple", "value": 3},
    })
    plan = build_single_snapshot(
        strategy, bars, side="buy", data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27),
    )

    overlay = [rule for rule in plan["risk_rules"] if rule["source"] == "overlay"][0]
    assert overlay["calculation_status"] == "pending_simulated_entry"
    assert "reference_line" not in overlay
    assert plan["take_profit"]["calculation_status"] == "pending_simulated_entry"
    assert "reference_line" not in plan["take_profit"]


def test_single_plan_prefers_the_overlay_state_machines_entry_atr_line():
    bars = _bars()
    strategy = _strategy("ma_cross", params={
        "risk_overlay": {
            "enabled": True, "type": "atr_multiple", "value": 2,
            "atr_period": 14,
        },
    })

    plan = build_single_snapshot(
        strategy, bars, side="hold", data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27), entry_price=20,
        overlay_state_rules=[{
            "source": "risk_overlay", "calculation_status": "calculated",
            "reference_line": 17.25, "simulated_entry_price": 20,
            "data_date": "2026-07-24",
        }],
    )
    overlay = next(
        rule for rule in plan["risk_rules"] if rule["source"] == "overlay"
    )

    assert overlay["reference_line"] == 17.25
    assert overlay["simulated_entry_price"] == 20


def test_exit_plan_reuses_the_triggered_overlay_price_lines():
    bars = _bars()
    strategy = _strategy("breakout", params={
        "risk_overlay": {"enabled": True, "type": "fixed_pct", "value": 0.08},
        "take_profit": {"enabled": True, "type": "fixed_pct", "value": 0.20},
    })
    plan = build_single_snapshot(
        strategy, bars, side="sell", data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27),
        exit_hits=[
            {"code": "risk_overlay", "name": "风险覆盖层", "price_line": 9.2},
            {"code": "take_profit", "name": "止盈覆盖层", "price_line": 12.0},
        ],
    )

    overlay = [rule for rule in plan["risk_rules"] if rule["source"] == "overlay"][0]
    assert overlay["calculation_status"] == "calculated"
    assert overlay["reference_line"] == 9.2
    assert plan["take_profit"]["calculation_status"] == "calculated"
    assert plan["take_profit"]["reference_line"] == 12.0


@pytest.mark.parametrize("template", ["breakout", "volume_breakout"])
def test_breakout_range_requires_explicit_positive_premium(template):
    bars = _bars()
    line = build_single_snapshot(
        _strategy(template), bars, side="watch",
        data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27),
    )["entry_observation"]
    ranged = build_single_snapshot(
        _strategy(template, params={"max_entry_premium": 0.03}),
        bars, side="watch", data_date=bars["date"].iat[-1],
        next_execution_date=date(2026, 7, 27),
    )["entry_observation"]

    assert line["kind"] == "line"
    assert "upper" not in line
    assert ranged["kind"] == "range"
    assert ranged["lower"] == ranged["line"]
    assert ranged["upper"] == pytest.approx(ranged["line"] * 1.03, abs=1e-4)


@pytest.mark.parametrize("template", ["momentum_rotation", "multifactor_hold"])
def test_two_portfolio_templates_return_per_stock_change_reasons(template):
    strategy = _strategy(template, kind="portfolio")
    snapshot, items = build_portfolio_snapshot(
        strategy, data_date=date(2026, 7, 24),
        next_execution_date=date(2026, 7, 27), pool_name="研究池",
        previous_weights={"a": 0.5, "b": 0.5},
        target_weights={"a": 0.5, "b": 0.0, "c": 0.5},
        scores={"a": 0.2, "b": 0.1, "c": 0.3},
        eligible={"a": True, "b": True, "c": True},
        risk_lines={"a": 10.0, "b": 9.0, "c": 8.0},
    )

    assert snapshot["entry_observation"]["kind"] == "portfolio_rebalance"
    assert "line" not in snapshot["entry_observation"]
    by_code = {item["code"]: item for item in items}
    assert by_code["b"]["change_type"] == "removed"
    assert by_code["c"]["change_type"] == "added"
    assert all(item["reasons"] for item in items)
    if template == "momentum_rotation":
        assert by_code["b"]["reasons"][0]["code"] == "left_top_n"


def test_single_plan_is_versioned_bound_to_exact_backtest_and_returned_with_signal():
    with _session() as db:
        strategy = _strategy("breakout")
        expected_version = strategy_version(strategy, parameter_snapshot(strategy))
        db.add(strategy)
        db.add(Stock(code="sh.600000", name="浦发银行", industry="银行"))
        # 明确日历使 T/T+1 口径可审计。
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        exact_params = {
            "entry": 20, "exit": 10, "max_entry_premium": 0.0,
            "risk_overlay": {
                "enabled": False, "type": "fixed_pct", "value": 0.08,
                "atr_period": 14,
            },
            "take_profit": {
                "enabled": False, "type": "fixed_pct", "value": 0.2,
                "atr_period": 14,
            },
        }
        db.add(BacktestRun(
            id=7, user_id=USER, strategy_id=1, params=exact_params,
            costs=DEFAULT_COSTS, codes=["sh.600000"],
            start=date(2024, 1, 1), end=date(2025, 12, 31),
            metrics={
                "total_return": 0.12, "max_drawdown": -0.08,
                "win_rate": 0.55, "trade_count": 20,
                "evidence": {
                    "strategy_version": expected_version,
                    "parameter_snapshot": exact_params,
                    "fee_assumptions": {"commission": {"rate": 0.00025}},
                    "exit_reason_distribution": {"native": 4},
                },
            },
        ))
        db.add(BacktestRun(
            id=8, user_id=USER, strategy_id=1, params=exact_params,
            costs={**DEFAULT_COSTS, "slippage": 0.02},
            codes=["sh.600000"], start=date(2024, 1, 1),
            end=date(2025, 12, 31),
            metrics={"evidence": {"strategy_version": expected_version}},
        ))
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=12.3,
            reason={"params": {}, "prev_position": 0, "cur_position": 1},
        )
        db.add(signal)
        db.flush()

        first = create_single_plan(db, strategy, signal, _bars())
        second = create_single_plan(db, strategy, signal, _bars())
        db.commit()

        assert first.revision == 1
        assert second.revision == 2
        assert second.supersedes_plan_id == first.id
        assert signal.plan_id == second.id
        assert first.params_snapshot == second.params_snapshot
        assert first.params_snapshot["simulation_costs"] == DEFAULT_COSTS
        assert first.product_boundary == PRODUCT_BOUNDARY
        assert second.backtest_run_id == 7
        assert second.backtest_evidence["status"] == "verified"
        assert second.backtest_evidence["metrics"]["trade_count"] == 20
        assert effective_status(first, date(2026, 7, 27), db=db)[0] == "expired"

        listed = list_signals(
            date_=None, code=None, strategy_id=None, side=None,
            limit=200, db=db, claims=CLAIMS,
        )
        summary = listed["items"][0]["research_plan"]
        assert "reason" not in listed["items"][0]
        assert listed["items"][0]["signal_close_price"] == 12.3
        assert listed["items"][0]["research_plan_id"] == second.id
        assert listed["items"][0]["plan_status"] == summary["status"]
        assert summary["plan_id"] == second.id
        assert summary["next_simulated_execution_date"] == "2026-07-27"
        assert summary["signal_close_price"] == 12.3

        detail = plan_detail(
            db, second, as_of=date(2026, 7, 27), viewer_user_id=USER)
        assert detail["strategy"]["version"].startswith("rp1-")
        assert detail["params_snapshot"]["effective_params"]["entry"] == 20
        assert detail["portfolio_changes"] == []


def test_unverified_plan_keeps_its_simulation_fee_snapshot():
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=12.3, reason={},
        )
        db.add(signal)
        db.flush()

        plan = create_single_plan(db, strategy, signal, _bars())
        summary = plan_summary(plan, db=db, viewer_user_id=USER)

        assert summary["backtest_evidence"]["status"] == "unverified"
        assert summary["backtest_evidence"]["costs"] == DEFAULT_COSTS


@pytest.mark.parametrize(
    ("template", "expected_text"),
    [
        ("ma_cross", "均线"),
        ("breakout", "高点"),
        ("mean_reversion", "RSI"),
        ("volume_breakout", "放量"),
    ],
)
def test_new_confirmed_close_invalidates_lost_native_entry_condition(
    template, expected_text,
):
    with _session() as db:
        params, initial, later = _native_condition_case(template)
        strategy = _strategy(template, params=params)
        effective = validate_strategy_params(template, strategy.params)
        assert evaluate_single_entry_condition(
            template, initial, effective, "buy")["satisfied"] is True
        db.add_all([
            strategy,
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
            *_daily_bar_rows("sh.600000", initial),
            *_daily_bar_rows("sh.600000", later),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=float(initial["close"].iat[-1]), reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, initial)

        status, reason = effective_status(plan, date(2026, 7, 27), db=db)

        assert status == "invalid"
        assert reason["code"] == "native_entry_condition_lost"
        assert "最新确认收盘" in reason["text"]
        assert expected_text in reason["text"]


def test_native_condition_reevaluation_uses_plan_snapshot_not_current_strategy_params():
    with _session() as db:
        close = [10 + index * 0.2 for index in range(80)]
        close += [close[-1] - (index + 1) * 0.3 for index in range(13)]
        initial = _frame(close)
        later = _frame(
            [float(initial["close"].iat[-1]) + 0.2], end="2026-07-27")
        combined = pd.concat([initial, later], ignore_index=True)
        strategy = _strategy("mean_reversion")
        db.add_all([
            strategy,
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
            *_daily_bar_rows("sh.600000", combined),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=float(initial["close"].iat[-1]), reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, initial)

        strategy.params = {"rsi_buy": 40}
        current_params = validate_strategy_params(strategy.template, strategy.params)
        assert evaluate_single_entry_condition(
            strategy.template, combined, current_params, "buy")["satisfied"] is True

        status, reason = effective_status(plan, date(2026, 7, 27), db=db)

        assert plan.params_snapshot["effective_params"]["rsi_buy"] == 30
        assert status == "invalid"
        assert reason["code"] == "native_entry_condition_lost"


def test_watch_plan_remains_valid_while_template_watch_condition_is_true():
    frame = _frame([10.0] * 30 + [9.99], end="2026-07-27")
    result = evaluate_single_entry_condition(
        "ma_cross", frame,
        validate_strategy_params("ma_cross", {}), "watch",
    )

    assert result["satisfied"] is True


def test_public_plan_binds_only_viewers_matching_strategy_version_backtest():
    user_b = "22222222-2222-2222-2222-222222222222"
    user_c = "33333333-3333-3333-3333-333333333333"
    claims_b = {"sub": user_b, "username": "b", "can_client": True}
    with _session() as db:
        strategy = Strategy(
            id=1, owner_id=SYSTEM_OWNER_ID, is_system=True,
            name="公共突破", template="breakout", kind="single",
            params={}, enabled=True,
        )
        db.add_all([
            strategy,
            Stock(code="sh.600000", name="浦发银行", industry="银行"),
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=11.0, reason={},
        )
        db.add(signal)
        db.flush()
        initial = _native_condition_case("breakout")[1]
        plan = create_single_plan(db, strategy, signal, initial)
        exact_params = plan.params_snapshot["effective_params"]

        def run(run_id: int, user_id: str, version: str) -> BacktestRun:
            return BacktestRun(
                id=run_id, user_id=user_id, strategy_id=1,
                params=exact_params, costs=DEFAULT_COSTS,
                codes=["sh.600000"],
                start=date(2024, 1, 1), end=date(2025, 12, 31),
                metrics={
                    "total_return": run_id / 100,
                    "evidence": {
                        "strategy_version": version,
                        "parameter_snapshot": exact_params,
                    },
                },
            )

        db.add_all([
            run(21, USER, plan.strategy_version),
            run(22, user_b, plan.strategy_version),
            run(23, user_c, "rp1-wrong-version"),
        ])
        db.commit()

        signal_for_a = list_signals(
            date_=None, code=None, strategy_id=None, side=None,
            limit=200, db=db, claims=CLAIMS,
        )["items"][0]["research_plan"]
        signal_for_b = list_signals(
            date_=None, code=None, strategy_id=None, side=None,
            limit=200, db=db, claims=claims_b,
        )["items"][0]["research_plan"]
        detail_for_b = plan_detail(db, plan, viewer_user_id=user_b)
        detail_for_c = plan_detail(db, plan, viewer_user_id=user_c)

        assert signal_for_a["backtest_evidence"]["run_id"] == 21
        assert signal_for_b["backtest_evidence"]["run_id"] == 22
        assert detail_for_b["backtest_evidence"]["run_id"] == 22
        assert detail_for_c["backtest_status"] == "unverified"
        assert "run_id" not in detail_for_c["backtest_evidence"]


def test_same_params_with_wrong_strategy_version_is_unverified():
    with _session() as db:
        strategy = _strategy("breakout")
        db.add_all([
            strategy,
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=11.0, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(
            db, strategy, signal, _native_condition_case("breakout")[1])
        db.add(BacktestRun(
            user_id=USER, strategy_id=1,
            params=plan.params_snapshot["effective_params"], costs={},
            codes=["sh.600000"], start=date(2024, 1, 1), end=date(2025, 1, 1),
            metrics={"evidence": {"strategy_version": "rp1-old"}},
        ))
        db.flush()

        summary = plan_summary(plan, db=db, viewer_user_id=USER)

        assert summary["backtest_status"] == "unverified"


def test_current_single_plan_expires_after_its_next_trading_day():
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=12.3, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())

        assert effective_status(plan, date(2026, 7, 27))[0] == "current"
        assert effective_status(plan, date(2026, 7, 28))[0] == "expired"


def test_single_plan_requires_reevaluation_when_snapshot_leaves_entry_range():
    with _session() as db:
        strategy = _strategy(
            "breakout", params={"max_entry_premium": 0.02})
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="watch", price=12.3, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())
        upper = plan.entry_observation["upper"]
        db.add(Snapshot(
            code=signal.code, ts=datetime(2026, 7, 25, 10, 0),
            price=upper + 0.5,
        ))
        db.flush()

        status, reason = effective_status(
            plan, date(2026, 7, 25), db=db)

        assert status == "reevaluate"
        assert reason["code"] == "price_outside_entry_range"


@pytest.mark.parametrize(
    ("execution_open", "is_st"),
    [(11.0, False), (10.5, True)],
    ids=["main_board_10_pct", "daily_st_5_pct"],
)
def test_single_plan_requires_reevaluation_when_next_open_is_limit_up(
    execution_open, is_st,
):
    with _session() as db:
        strategy = _strategy("breakout")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 24), open=9.9,
                high=10.1, low=9.8, close=10.0, raw_close=10.0,
                volume=1_000_000, amount=10_000_000,
            ),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 27),
                open=execution_open, high=execution_open, low=execution_open,
                close=execution_open, raw_close=execution_open,
                volume=1_000_000, amount=execution_open * 1_000_000,
                is_st=is_st,
            ),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=10.0, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())

        status, reason = effective_status(
            plan, date(2026, 7, 27), db=db)

        assert status == "reevaluate"
        assert reason["code"] == "open_limit_up"


@pytest.mark.parametrize(
    ("execution_open", "execution_volume"),
    [(10.0, 0.0), (0.0, 1_000_000.0)],
    ids=["suspended_zero_volume", "missing_open"],
)
def test_single_plan_requires_reevaluation_when_next_open_is_untradable(
    execution_open, execution_volume,
):
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 24), open=9.9,
                high=10.1, low=9.8, close=10.0, raw_close=10.0,
                volume=1_000_000, amount=10_000_000,
            ),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 27),
                open=execution_open, high=10.1, low=9.8, close=10.0,
                raw_close=10.0, volume=execution_volume,
                amount=10.0 * execution_volume,
            ),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=10.0, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())

        status, reason = effective_status(
            plan, date(2026, 7, 27), db=db)

        assert status == "reevaluate"
        assert reason["code"] == "next_day_untradable"


def test_single_plan_requires_reevaluation_when_next_day_has_no_bar():
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=12.3, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())

        status, reason = effective_status(
            plan, date(2026, 7, 28), db=db)

        assert status == "reevaluate"
        assert reason["code"] == "next_day_untradable"


def test_portfolio_plan_persists_structured_items_and_reasons():
    with _session() as db:
        strategy = _strategy("momentum_rotation", kind="portfolio")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
            Stock(code="a", name="甲", industry=""),
            Stock(code="b", name="乙", industry=""),
        ])
        pool = {
            "a": _bars(end="2024-07-01", periods=90, daily_return=0.006),
            "b": _bars(end="2024-07-01", periods=90, daily_return=0.002),
        }
        plan = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=1, pool_name="研究池", pool_dfs=pool,
        )
        db.commit()

        items = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == plan.id)).scalars().all()
        assert len(items) == 2
        assert all(item.reasons for item in items)
        assert all(item.score_details.get("factors") for item in items)
        detail = plan_detail(db, plan, as_of=date(2024, 7, 1))
        assert detail["plan_type"] == "portfolio_rebalance"
        assert {item["code"] for item in detail["portfolio_changes"]} == {"a", "b"}
        assert all(item["reasons"] for item in detail["portfolio_changes"])


def test_portfolio_plan_persists_active_per_holding_overlay_lines():
    with _session() as db:
        strategy = _strategy("momentum_rotation", kind="portfolio", params={
            "risk_overlay": {
                "enabled": True, "type": "fixed_pct", "value": 0.1,
            },
            "take_profit": {
                "enabled": True, "type": "fixed_pct", "value": 0.2,
            },
        })
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
        ])
        pool = {
            "a": _bars(end="2024-07-01", periods=90, daily_return=0.006),
            "b": _bars(end="2024-07-01", periods=90, daily_return=0.002),
        }

        plan = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=1, pool_name="研究池", pool_dfs=pool,
        )
        db.flush()
        active = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == plan.id,
            ResearchPlanItem.target_weight > 0,
        )).scalars().all()

        assert active
        for item in active:
            rules = item.risk_snapshot["rules"]
            by_source = {rule["source"]: rule for rule in rules}
            assert by_source["risk_overlay"]["reference_line"] > 0
            assert by_source["take_profit"]["reference_line"] > 0
            assert by_source["risk_overlay"]["data_date"] == "2024-07-01"


def test_triggered_portfolio_overlay_keeps_top_level_plan_snapshot():
    with _session() as db:
        strategy = _strategy("momentum_rotation", kind="portfolio", params={
            "top_n": 1,
            "risk_overlay": {
                "enabled": True, "type": "fixed_pct", "value": 0.05,
            },
        })
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
        ])
        prices = [10.0 * 1.005 ** index for index in range(89)]
        prices.append(prices[-1] * 0.7)

        plan = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=1, pool_name="研究池",
            pool_dfs={"a": _frame(prices, end="2024-07-01")},
        )

        assert plan.strategy_version.startswith("rp1-")
        assert plan.status == "exit_triggered"
        assert plan.status_reason["code"] == "portfolio_exit_condition_met"
        assert any(hit["code"] == "risk_overlay" for hit in plan.exit_hits)


def test_portfolio_plan_keeps_prior_pool_removal_as_zero_weight_item():
    with _session() as db:
        strategy = _strategy(
            "multifactor_hold", kind="portfolio", params={"top_n": 2})
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
            TradeCalendar(date=date(2024, 7, 3), is_open=True),
        ])
        first_pool = {
            "a": _bars(end="2024-07-01", periods=90, daily_return=0.006),
            "b": _bars(end="2024-07-01", periods=90, daily_return=0.002),
        }
        first = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=1, pool_name="研究池", pool_dfs=first_pool,
        )
        db.flush()
        first_b = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == first.id,
            ResearchPlanItem.code == "b",
        )).scalar_one()
        assert first_b.target_weight > 0

        second = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 2),
            pool_id=1, pool_name="研究池",
            pool_dfs={
                "a": _bars(end="2024-07-02", periods=91, daily_return=0.006),
            },
        )
        db.flush()
        removed = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == second.id,
            ResearchPlanItem.code == "b",
        )).scalar_one()

        assert removed.previous_weight > 0
        assert removed.target_weight == 0
        assert removed.eligible is False
        assert removed.reasons[0]["code"] == "ineligible"


def test_portfolio_evidence_requires_the_same_pool_and_default_fees():
    with _session() as db:
        strategy = _strategy("multifactor_hold", kind="portfolio")
        db.add_all([
            strategy,
            Pool(id=7, kind="static", ref=None, owner_id=USER,
                 is_system=False, name="研究池", min_list_days=0),
            Pool(id=8, kind="static", ref=None, owner_id=USER,
                 is_system=False, name="其他池", min_list_days=0),
        ])
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
        ])
        plan = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=7, pool_name="研究池",
            pool_dfs={
                "a": _bars(end="2024-07-01", periods=90),
                "b": _bars(end="2024-07-01", periods=90, daily_return=0.003),
            },
        )
        params = plan.params_snapshot["effective_params"]
        evidence = {
            "strategy_version": plan.strategy_version,
            "parameter_snapshot": params,
        }
        db.add_all([
            BacktestRun(
                id=31, user_id=USER, strategy_id=strategy.id,
                params=params, costs=DEFAULT_COSTS, codes=["a", "b"],
                pool_id=7, start=date(2023, 1, 1), end=date(2024, 1, 1),
                metrics={"total_return": 0.1, "evidence": evidence},
            ),
            BacktestRun(
                id=32, user_id=USER, strategy_id=strategy.id,
                params=params, costs=DEFAULT_COSTS, codes=["a", "b"],
                pool_id=8, start=date(2023, 1, 1), end=date(2024, 1, 1),
                metrics={"total_return": 0.9, "evidence": evidence},
            ),
            BacktestRun(
                id=33, user_id=USER, strategy_id=strategy.id,
                params=params, costs=DEFAULT_COSTS, codes=["a"],
                pool_id=7, start=date(2023, 1, 1), end=date(2024, 1, 1),
                metrics={"total_return": 0.8, "evidence": evidence},
            ),
        ])
        db.flush()

        summary = plan_summary(plan, db=db, viewer_user_id=USER)

        assert summary["backtest_evidence"]["run_id"] == 31


@pytest.mark.parametrize(
    ("previous_weight", "target_weight", "execution_open", "reason_code"),
    [(0.0, 0.5, 11.0, "open_limit_up"),
     (0.5, 0.0, 9.0, "open_limit_down")],
)
def test_portfolio_plan_rechecks_each_changed_holding_at_next_open(
    previous_weight, target_weight, execution_open, reason_code,
):
    with _session() as db:
        strategy = _strategy("momentum_rotation", kind="portfolio")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
            DailyBar(
                code="sh.600000", date=date(2024, 7, 1), open=10,
                high=10, low=10, close=10, raw_close=10,
                volume=1_000_000, amount=10_000_000,
            ),
            DailyBar(
                code="sh.600000", date=date(2024, 7, 2),
                open=execution_open, high=execution_open, low=execution_open,
                close=execution_open, raw_close=execution_open,
                volume=1_000_000, amount=execution_open * 1_000_000,
            ),
        ])
        plan = create_portfolio_plan(
            db, strategy, data_date=date(2024, 7, 1),
            pool_id=1, pool_name="研究池",
            pool_dfs={
                "sh.600000": _bars(
                    end="2024-07-01", periods=90, daily_return=0.006),
            },
        )
        db.flush()
        item = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == plan.id,
            ResearchPlanItem.code == "sh.600000",
        )).scalar_one()
        item.previous_weight = previous_weight
        item.target_weight = target_weight
        db.flush()

        status, reason = effective_status(
            plan, date(2024, 7, 2), db=db)

        assert status == "reevaluate"
        assert reason["code"] == reason_code


def test_next_execution_date_is_unknown_when_calendar_has_no_future_session():
    with _session() as db:
        db.add(TradeCalendar(date=date(2026, 9, 30), is_open=True))
        db.flush()

        assert next_trading_day(db, date(2026, 9, 30)) is None


def test_exit_plan_rechecks_limit_down_on_its_execution_day():
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 24), open=10,
                high=10, low=10, close=10, raw_close=10,
                volume=1_000_000, amount=10_000_000,
            ),
            DailyBar(
                code="sh.600000", date=date(2026, 7, 27), open=9,
                high=9, low=9, close=9, raw_close=9,
                volume=1_000_000, amount=9_000_000,
            ),
        ])
        signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="sell", price=10, reason={},
        )
        db.add(signal)
        db.flush()
        plan = create_single_plan(db, strategy, signal, _bars())

        status, reason = effective_status(
            plan, date(2026, 7, 27), db=db)

        assert status == "reevaluate"
        assert reason["code"] == "open_limit_down"


def test_signal_side_change_continues_single_plan_version_chain():
    with _session() as db:
        strategy = _strategy("ma_cross")
        db.add(strategy)
        db.add_all([
            TradeCalendar(date=date(2026, 7, 23), is_open=True),
            TradeCalendar(date=date(2026, 7, 24), is_open=True),
            TradeCalendar(date=date(2026, 7, 27), is_open=True),
        ])
        first_signal = Signal(
            code="sh.600000", date=date(2026, 7, 23), strategy_id=1,
            side="buy", price=10, reason={},
        )
        db.add(first_signal)
        db.flush()
        first = create_single_plan(db, strategy, first_signal, _bars(
            end="2026-07-23"))
        second_signal = Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="sell", price=9, reason={},
        )
        db.add(second_signal)
        db.flush()

        second = create_single_plan(db, strategy, second_signal, _bars())

        assert second.supersedes_plan_id == first.id
        assert second.revision == first.revision + 1


def test_portfolio_plan_rejects_non_trading_day():
    with _session() as db:
        strategy = _strategy("multifactor_hold", kind="portfolio")
        db.add(strategy)
        db.add(TradeCalendar(date=date(2026, 7, 26), is_open=False))
        with pytest.raises(ValueError, match="非交易日"):
            create_portfolio_plan(
                db, strategy, data_date=date(2026, 7, 26),
                pool_id=1, pool_name="研究池",
                pool_dfs={"a": _bars(end="2026-07-24")},
            )


def test_portfolio_pipeline_generates_enabled_strategy_plan(monkeypatch):
    with _session() as db:
        strategy = _strategy("momentum_rotation", kind="portfolio")
        pool = Pool(
            id=9, kind="static", ref=None, owner_id=USER, is_system=False,
            name="研究池", min_list_days=0,
        )
        db.add_all([
            strategy, pool,
            TradeCalendar(date=date(2024, 7, 1), is_open=True),
            TradeCalendar(date=date(2024, 7, 2), is_open=True),
        ])
        frames = {
            "a": _bars(end="2024-07-01", periods=90, daily_return=0.006),
            "b": _bars(end="2024-07-01", periods=90, daily_return=0.002),
        }
        monkeypatch.setattr(plan_pipeline, "resolve_pool",
                            lambda *args, **kwargs: ["a", "b"])
        monkeypatch.setattr(plan_pipeline, "resolve_pool_during",
                            lambda *args, **kwargs: ["a", "b"])
        monkeypatch.setattr(
            plan_pipeline, "load_bars_df",
            lambda db_, code, start=None, end=None: frames[code],
        )

        result = plan_pipeline.run_portfolio_plans(
            db, day=date(2024, 7, 1), pool=pool, strategies=[strategy])

        assert result["count"] == 1
        plan = db.get(ResearchPlan, result["plans"][0]["plan_id"])
        assert plan is not None
        assert plan.plan_type == "portfolio_rebalance"
