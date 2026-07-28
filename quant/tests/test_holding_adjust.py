"""HoldingSpec 加仓/减仓规则:规格校验、编译器档位状态机与回测中间档位。"""
from __future__ import annotations

import sys
from copy import deepcopy
from datetime import date
from pathlib import Path

import numpy as np
import pandas as pd
import pytest
from pydantic import ValidationError

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import _batch_single
from app.strategy.compiler import compile_single
from app.strategy.presets import SYSTEM_STRATEGY_SPECS
from app.strategy.spec import (
    CapabilityStatus,
    StrategySpec,
    canonical_spec_json,
    resolve_capabilities,
    strategy_spec_hash,
    validate_strategy_spec,
)


def _field(name: str) -> dict:
    return {"op": "field", "name": name}


def _literal(value: float) -> dict:
    return {"op": "literal", "value": value}


def _gt(name: str, value: float) -> dict:
    return {"op": "gt", "left": _field(name), "right": _literal(value)}


def _lt(name: str, value: float) -> dict:
    return {"op": "lt", "left": _field(name), "right": _literal(value)}


def _rule(condition: dict, reason_code: str) -> dict:
    return {"condition": condition, "reason_code": reason_code}


def _adjust_spec(
    *,
    step: float = 0.2,
    max_position: float = 0.8,
    target: float = 0.4,
    add_rule: dict | None = None,
    reduce_rule: dict | None = None,
) -> dict:
    """单标的规格:close>10 入场,close<8 离场,默认 close>12 加仓、close<9.5 减仓。"""
    if add_rule is None:
        add_rule = _rule(_gt("close", 12.0), "add_on_strength")
    if reduce_rule is None:
        reduce_rule = _rule(_lt("close", 9.5), "reduce_on_weakness")
    return {
        "schema_version": 1,
        "kind": "single",
        "metadata": {
            "canonical_id": "USER-ADJUST-01",
            "sources": [{"book": "测试", "candidate_id": "ADJUST-01"}],
            "evidence_status": "unverified",
            "hypothesis": "趋势走强时加档、走弱时减档。",
        },
        "universe": {
            "pool_id": 2, "exclude_st": True,
            "min_listing_days": 60, "min_amount_avg20": 0.0,
        },
        "data_requirements": [
            {"field": "close", "availability": "daily_close", "required": True},
            {"field": "volume", "availability": "daily_close", "required": True},
        ],
        "entry": _rule(_gt("close", 10.0), "entry_rule"),
        "positioning": {"type": "fixed", "target": target},
        "holding": {
            "allow_add": True,
            "allow_reduce": True,
            "add_rule": add_rule,
            "reduce_rule": reduce_rule,
            "step": step,
            "max_position": max_position,
            "cooldown_days": 0,
            "risk_reentry": "native_reset",
        },
        "native_exit": _rule(_lt("close", 8.0), "exit_rule"),
        "overlays": {
            "risk": {
                "enabled": False, "type": "fixed_pct", "value": 0.08,
                "atr_period": 14, "trailing": False,
            },
            "take_profit": {
                "enabled": False, "type": "fixed_pct", "value": 0.2,
                "atr_period": 14, "trailing": False,
            },
        },
        "portfolio_constraints": {
            "long_only": True, "max_positions": 500,
            "max_single_weight": 1.0, "max_total_weight": 1.0,
        },
        "execution": {
            "signal_time": "close", "execution_time": "next_open",
            "buy_limit_policy": "reject", "sell_limit_policy": "retry",
            "suspension_policy": "reject_entry_retry_exit",
            "missing_bar_policy": "reject_entry_retry_exit",
            "cost_model": "a_share_daily_v1", "max_entry_premium": 0.0,
        },
        "validation": {
            "baseline_ids": ["buy_and_hold"], "locked_oos": True,
            "rejection_criteria": ["no_net_oos_increment"],
            "parameter_scans": [],
        },
    }


def _bars(closes: list[float], volumes: list[float] | None = None) -> pd.DataFrame:
    dates = pd.bdate_range(date(2024, 1, 2), periods=len(closes))
    close = np.asarray(closes, dtype=float)
    volume = (
        np.asarray(volumes, dtype=float)
        if volumes is not None else np.full(len(closes), 1e6)
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


# ---------------------------------------------------------------- 规格校验


def test_allow_flags_require_matching_rules():
    spec = _adjust_spec()
    spec["holding"]["add_rule"] = None
    with pytest.raises(ValidationError, match="allow_add 为 true 时必须提供 add_rule"):
        StrategySpec.model_validate(spec)

    spec = _adjust_spec()
    spec["holding"]["allow_add"] = False
    with pytest.raises(ValidationError, match="allow_add 为 false 时 add_rule 必须为 null"):
        StrategySpec.model_validate(spec)

    spec = _adjust_spec()
    spec["holding"]["reduce_rule"] = None
    with pytest.raises(ValidationError, match="allow_reduce 为 true 时必须提供 reduce_rule"):
        StrategySpec.model_validate(spec)

    spec = _adjust_spec()
    spec["holding"]["allow_reduce"] = False
    with pytest.raises(ValidationError, match="allow_reduce 为 false 时 reduce_rule 必须为 null"):
        StrategySpec.model_validate(spec)


def test_valid_adjust_spec_passes_validation_and_capability():
    spec = _adjust_spec()
    result = validate_strategy_spec(spec)
    assert result.valid is True
    assert resolve_capabilities(spec).status == CapabilityStatus.SUPPORTED


def test_portfolio_rejects_holding_adjust():
    spec = _adjust_spec()
    spec["kind"] = "portfolio"
    spec["positioning"] = {
        "type": "portfolio",
        "score": {"op": "momentum", "input": _field("close"), "window": 20},
        "selection": {"type": "top_n", "n": 10},
        "weighting": {"type": "equal"},
        "rebalance": {"frequency": "weekly", "interval_days": None},
        "risk_filter": None,
    }
    with pytest.raises(ValidationError, match="组合策略暂不支持加仓/减仓"):
        StrategySpec.model_validate(spec)
    report = resolve_capabilities(spec)
    assert report.status == CapabilityStatus.MISSING_ENGINE
    assert any(
        issue.path == "$.holding.allow_add"
        and issue.code == "holding_adjust_portfolio"
        for issue in report.issues
    )


def test_legacy_spec_without_adjust_keys_keeps_old_canonical_shape():
    """旧规格没有新键:默认值等价旧行为,规范化 JSON 的 holding 段保持旧形状。"""
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    assert set(raw["holding"]) == {
        "allow_add", "allow_reduce", "cooldown_days", "risk_reentry",
    }
    parsed = StrategySpec.model_validate(raw)
    assert parsed.holding.add_rule is None
    assert parsed.holding.reduce_rule is None
    assert parsed.holding.step == 0.5
    assert parsed.holding.max_position == 1.0
    holding = canonical_spec_json(raw)
    assert '"add_rule"' not in holding
    assert strategy_spec_hash(raw) == strategy_spec_hash(parsed)


def test_spec_hash_covers_adjust_fields():
    base = _adjust_spec()
    without = _adjust_spec()
    without["holding"].update({
        "allow_add": False, "allow_reduce": False,
        "add_rule": None, "reduce_rule": None,
    })
    assert strategy_spec_hash(base) != strategy_spec_hash(without)

    other_step = _adjust_spec(step=0.3)
    assert strategy_spec_hash(base) != strategy_spec_hash(other_step)


def test_adjust_rule_fields_must_be_declared():
    spec = _adjust_spec()
    spec["data_requirements"] = [
        {"field": "close", "availability": "daily_close", "required": True},
    ]
    spec["holding"]["add_rule"] = _rule(_gt("volume", 2e6), "add_on_volume")
    with pytest.raises(ValidationError, match="data_requirements 未声明字段"):
        StrategySpec.model_validate(spec)


# ---------------------------------------------------------------- 编译器状态机


def test_compiler_multi_level_state_machine():
    df = _bars([9.0, 12.5, 12.6, 12.7, 12.8, 9.2, 9.1, 9.0, 9.0, 9.0, 11.0])
    compiled = compile_single(_adjust_spec(), df)
    positions = [round(float(value), 6) for value in compiled.positions]
    # 进场当日不加仓;到上限不再加;减到 0 等同离场;离场后可重新入场
    assert positions == [
        0.0, 0.4, 0.6, 0.8, 0.8, 0.6, 0.4, 0.2, 0.0, 0.0, 0.4,
    ]

    days = list(compiled.reasons)
    day_items = {str(day.date()): items for day, items in compiled.reasons.items()}
    add_days = [
        str(day.date()) for day in days
        if any(item.get("type") == "add" for item in compiled.reasons[day])
    ]
    assert add_days == [
        str(df["date"][2]), str(df["date"][3]),
    ], day_items
    add_item = day_items[str(df["date"][2])][0]
    assert add_item["reason_code"] == "add_on_strength"
    assert add_item["prev_target"] == pytest.approx(0.4)
    assert add_item["cur_target"] == pytest.approx(0.6)

    reduce_days = [
        str(day.date()) for day in days
        if any(item.get("type") == "reduce" for item in compiled.reasons[day])
    ]
    assert reduce_days == [
        str(df["date"][5]), str(df["date"][6]), str(df["date"][7]),
    ]

    # 减到 0:按原生离场语义记录,等待新的入场事件
    exit_items = day_items[str(df["date"][8])]
    assert exit_items[0]["code"] == "native_exit"
    assert exit_items[0]["reason_code"] == "reduce_on_weakness"
    assert exit_items[0]["via"] == "reduce_to_zero"


def test_compiler_same_day_priority_reduce_over_add():
    spec = _adjust_spec(
        add_rule=_rule(_gt("volume", 2e6), "add_on_volume"),
        reduce_rule=_rule(_lt("close", 9.5), "reduce_on_weakness"),
    )
    df = _bars(
        [11.0, 9.2, 9.3],
        volumes=[1e6, 3e6, 3e6],
    )
    compiled = compile_single(spec, df)
    positions = [round(float(value), 6) for value in compiled.positions]
    # 第二天 add 与 reduce 同日触发:reduce 优先
    assert positions == [0.4, 0.2, 0.0]
    items = compiled.reasons[pd.Timestamp(df["date"][1])]
    assert [item["type"] for item in items] == ["reduce"]
    # 第三天 reduce 减到 0 -> 离场
    exit_items = compiled.reasons[pd.Timestamp(df["date"][2])]
    assert exit_items[0]["code"] == "native_exit"


def test_compiler_same_day_priority_exit_over_reduce():
    df = _bars([11.0, 12.5, 7.5])
    compiled = compile_single(_adjust_spec(), df)
    positions = [round(float(value), 6) for value in compiled.positions]
    assert positions == [0.4, 0.6, 0.0]
    # 离场日 close<8 同时满足 reduce:exit 优先,原因是离场规则而非减仓
    exit_items = compiled.reasons[pd.Timestamp(df["date"][2])]
    assert exit_items[0]["code"] == "native_exit"
    assert exit_items[0]["reason_code"] == "exit_rule"
    assert "via" not in exit_items[0]


def test_compiler_overlay_path_matches_native_levels():
    """带执行状态的覆盖层路径(生产口径)与原生编译的档位序列一致。"""
    df = _bars([9.0, 12.5, 12.6, 12.7, 9.2, 9.1, 8.9, 9.0, 11.0])
    spec = _adjust_spec()
    native = compile_single(spec, df)
    tradable = pd.Series(True, index=pd.DatetimeIndex(df["date"]))
    overlaid = compile_single(spec, df, entry_tradable=tradable)
    assert [round(float(v), 6) for v in overlaid.positions] == [
        round(float(v), 6) for v in native.positions
    ]


def test_compiler_zero_change_regression_without_adjust():
    """未启用加减仓的旧规格:仓位序列仍是 0/target 二值。"""
    raw = deepcopy(SYSTEM_STRATEGY_SPECS["ma_cross"])
    df = _bars(list(np.linspace(10, 12, 40)) + list(np.linspace(12, 10, 40)))
    compiled = compile_single(raw, df)
    assert set(compiled.positions.unique()).issubset({0, 1})


# ---------------------------------------------------------------- 回测中间档位


def _rising_dfs(days: int = 60, daily_return: float = 0.01) -> dict:
    dates = pd.bdate_range(date(2024, 1, 2), periods=days)
    close = 10 * (1 + daily_return) ** np.arange(days)
    frame = pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": close,
        "low": close,
        "close": close,
        "raw_close": close,
        "volume": np.full(days, 1e6),
        "amount": close * 1e6,
    })
    return {"sh.600000": frame}


def _positions(frame: pd.DataFrame, levels: list[float]) -> pd.Series:
    assert len(levels) == len(frame)
    return pd.Series(levels, index=frame.index, dtype=float)


def test_fractional_positions_execute_partial_trades():
    dfs = _rising_dfs()
    frame = dfs["sh.600000"]
    levels = [0.0] * 10 + [0.4] * 10 + [0.8] * 10 + [0.0] * 30
    zero_costs = {"commission": 0.0, "stamp_tax": 0.0, "slippage": 0.0}

    results = _batch_single(
        dfs, {"sh.600000": _positions(frame, levels)}, zero_costs,
        start=date(2024, 1, 2),
    )

    result = results["sh.600000"]
    sides = [item["side"] for item in result["trade_details"]]
    # 建仓 0.4、加仓到 0.8、全部卖出:两笔买入一笔卖出
    assert sides == ["buy", "buy", "sell"]
    assert result["metrics"]["trade_count"] == 3

    equity = result["equity"]
    # 零费用 + 固定日涨幅:0.8 档区间的日均收益约为 0.4 档区间的两倍
    eq = equity.to_numpy(dtype=float)
    # 仓位在信号次日开盘生效:第 11 根 bar 起 0.4 档,第 21 根起 0.8 档,第 31 根起空仓
    growth_half = eq[19] / eq[12] - 1
    growth_full = eq[29] / eq[22] - 1
    assert growth_half == pytest.approx(0.4 * ((1.01 ** 7 - 1)), rel=0.05)
    assert growth_full == pytest.approx(0.8 * ((1.01 ** 7 - 1)), rel=0.05)
    # 离场后无持仓,净值不再随价格波动
    assert eq[-1] == pytest.approx(eq[31], rel=1e-9)


def test_fractional_sell_retries_when_limit_down():
    dfs = _rising_dfs()
    frame = dfs["sh.600000"].copy()
    # 第 31 根 bar(减仓信号的 T+1 执行日)开盘一字跌停:卖不出,顺延到下一交易日
    frame.loc[31, "open"] = frame.loc[30, "close"] * 0.9
    frame.loc[31, "close"] = frame.loc[30, "close"] * 0.9
    frame.loc[31, "low"] = frame.loc[31, "open"]
    frame.loc[31, "high"] = frame.loc[31, "open"]
    dfs["sh.600000"] = frame
    levels = [0.0] * 10 + [0.4] * 10 + [0.8] * 10 + [0.0] * 30
    zero_costs = {"commission": 0.0, "stamp_tax": 0.0, "slippage": 0.0}

    results = _batch_single(
        dfs, {"sh.600000": _positions(frame, levels)}, zero_costs,
        start=date(2024, 1, 2),
    )

    details = results["sh.600000"]["trade_details"]
    sells = [item for item in details if item["side"] == "sell"]
    assert len(sells) == 1
    # 信号在第 30 根(date[30]),T+1(第 31 根)跌停卖不出,顺延到其后首个可成交开盘
    assert sells[0]["signal_date"] == str(frame["date"][30])
    assert sells[0]["execution_date"] > str(frame["date"][31])


def test_binary_positions_stay_on_signal_path(monkeypatch):
    """二值仓位仍走 from_signals 路径,不触发中间档位撮合。"""
    import app.backtest.engine as engine

    called = []
    original = engine._batch_single_fractional

    def spy(*args, **kwargs):
        called.append(True)
        return original(*args, **kwargs)

    monkeypatch.setattr(engine, "_batch_single_fractional", spy)
    dfs = _rising_dfs()
    frame = dfs["sh.600000"]
    levels = [0.0] * 10 + [1.0] * 20 + [0.0] * 30
    results = _batch_single(
        dfs, {"sh.600000": _positions(frame, levels)},
        {"commission": 0.0, "stamp_tax": 0.0, "slippage": 0.0},
        start=date(2024, 1, 2),
    )
    assert called == []
    sides = [
        item["side"] for item in results["sh.600000"]["trade_details"]
    ]
    assert sides == ["buy", "sell"]


def test_reduce_reason_appears_in_trade_details():
    dfs = _rising_dfs()
    frame = dfs["sh.600000"]
    levels = [0.0] * 10 + [0.8] * 10 + [0.4] * 10 + [0.0] * 30
    signal_day = pd.Timestamp(frame["date"][20])
    exit_reasons = {
        "sh.600000": {
            signal_day: [{"code": "reduce", "name": "减仓规则触发"}],
        },
    }
    results = _batch_single(
        dfs, {"sh.600000": _positions(frame, levels)},
        {"commission": 0.0, "stamp_tax": 0.0, "slippage": 0.0},
        start=date(2024, 1, 2),
        exit_reasons_by_code=exit_reasons,
    )
    details = results["sh.600000"]["trade_details"]
    sells = [item for item in details if item["side"] == "sell"]
    assert len(sells) == 2  # 减档 + 清仓
    reduce_sell = sells[0]
    assert reduce_sell["primary_reason"]["code"] == "reduce"
    assert reduce_sell["primary_reason"]["name"] == "减仓规则触发"
    assert reduce_sell["signal_date"] == str(frame["date"][20])
