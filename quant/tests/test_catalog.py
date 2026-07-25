"""固定中文研究目录与信号说明的纯函数测试。"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.catalog import (BACKTEST_METRICS, FACTOR_FIELDS, FILTER_FIELDS,
                         STRATEGY_TEMPLATES, catalog_payload,
                         render_signal_reason, signal_reason_type)
from app.strategy.strategies import REGISTRY


def test_catalog_covers_runtime_factors_and_strategies():
    assert set(FACTOR_FIELDS) == {
        "mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5",
        "ma20_slope", "amount_avg20",
    }
    assert set(STRATEGY_TEMPLATES) == set(REGISTRY)
    for key, module in REGISTRY.items():
        entry = STRATEGY_TEMPLATES[key]
        params = {item["key"]: item["default"] for item in entry["params"]}
        assert params == module.DEFAULT_PARAMS
        # kind 在目录与模块里各存一份(quant_strategy.kind 由模块回填),
        # 三者漂移会让组合策略被当成单标的跑进按个股出信号的引擎
        assert entry["kind"] == module.KIND


def test_filter_field_contract_reserves_fundamental_keys():
    required = {
        "pe_ttm", "pb", "ps_ttm", "dividend_yield", "total_market_cap",
        "roe", "revenue_yoy", "profit_yoy", "gross_margin", "net_margin",
        "debt_ratio", "cashflow_ratio",
    }
    assert required <= set(FILTER_FIELDS)
    for key in required:
        assert FILTER_FIELDS[key]["source"] == "fundamental"
        assert FILTER_FIELDS[key]["available"] is True
        assert {
            "eq", "ne", "gt", "gte", "lt", "lte", "between",
            "is_null", "not_null",
        } <= set(FILTER_FIELDS[key]["operators"])
    assert FILTER_FIELDS["roe"]["input_scale"] == 0.01
    assert FILTER_FIELDS["mom20"]["input_scale"] == 0.01
    assert FILTER_FIELDS["rsi14"]["input_scale"] == 1.0
    assert FILTER_FIELDS["amount_avg20"]["input_scale"] == 100_000_000
    assert FILTER_FIELDS["total_market_cap"]["input_scale"] == 100_000_000
    assert FILTER_FIELDS["is_st"]["value_type"] == "boolean"
    assert FILTER_FIELDS["listing_days"]["unit"] == "交易日"


def test_catalog_items_have_explanations_and_limits():
    payload = catalog_payload()
    assert payload["product_boundary"]["execution"] == "manual_external"
    assert payload["signals"] == payload["signal_sides"]
    for section in (
        "factors", "indicators", "filter_fields", "strategy_templates",
        "signal_sides", "manual_trade_sides", "signal_reason_types",
        "backtest_metrics",
    ):
        assert payload[section]
        for item in payload[section]:
            assert item["key"]
            assert item["name"]
            assert item["description"]
            assert "unit" in item
            assert item["direction"]
            assert item["limits"]


def test_backtest_catalog_covers_engine_metrics():
    assert {
        "total_return", "annual_return", "max_drawdown", "sharpe",
        "win_rate", "trade_count", "round_trips",
    } <= set(BACKTEST_METRICS)


def test_signal_reason_is_human_readable_not_json_dump():
    reason = {
        "params": {}, "prev_position": 0, "cur_position": 1, "close": 10.5,
    }
    text = render_signal_reason("ma_cross", "buy", reason)
    assert signal_reason_type(reason) == "position_change"
    assert text == "5日均线上穿20日均线，策略模拟状态变为持有。"
    assert "{" not in text
    assert "prev_position" not in text


def test_watch_reason_uses_reason_type_details():
    reason = {
        "type": "near_entry_line", "close": 10.0,
        "entry_line": 10.1, "dist": 0.01, "params": {},
    }
    text = render_signal_reason("breakout", "watch", reason)
    assert signal_reason_type(reason) == "near_entry_line"
    assert "20日突破线" in text
    assert "1.00%" in text


def test_watch_reason_tolerates_missing_optional_values():
    text = render_signal_reason(
        "ma_cross", "watch", {"type": "near_cross", "gap_pct": None},
    )
    assert "均线交叉" in text
