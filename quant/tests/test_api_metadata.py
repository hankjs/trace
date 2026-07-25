"""股票型 API 响应直接携带名称、行业和中文说明。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pytest
from fastapi import HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.backtest import list_strategies
from app.api.market import get_kline, search_stocks
from app.api.portfolio import TradeIn, add_trade, get_positions, list_trades
from app.api.signals import list_signals
from app.db import Base
from app.models import DailyBar, Signal, Stock, Trade


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False}, poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_stock_and_bar(db: Session) -> None:
    db.add(Stock(
        code="sh.600519", name="贵州茅台", industry="白酒", is_watch=False,
    ))
    db.add(DailyBar(
        id=1, code="sh.600519", date=date(2026, 7, 24),
        open=1400.0, high=1420.0, low=1390.0, close=1410.0,
        raw_close=1410.0, volume=1000.0, amount=1_410_000.0,
    ))
    db.commit()


def _seed_search_stocks(db: Session) -> None:
    db.add_all([
        Stock(code="sh.600519", name="贵州茅台", industry="白酒", is_watch=False),
        Stock(code="sz.000001", name="平安银行", industry="银行", is_watch=False),
        Stock(code="bj.430047", name="诺思兰德", industry="医药", is_watch=True),
        Stock(code="bj.920001", name="北交新股", industry="制造", is_watch=False),
        Stock(code="sh.600000", name="浦发银行", industry="银行", is_watch=True),
        Stock(code="sz.300750", name="宁德时代", industry="电池", is_watch=False),
        Stock(code="sh.601111", name="百分百科技", industry="软件", is_watch=False),
    ])
    db.commit()


def test_signal_response_has_stock_and_human_reason():
    with _session() as db:
        _seed_stock_and_bar(db)
        db.add(Signal(
            id=1, code="sh.600519", date=date(2026, 7, 24),
            strategy="ma_cross", side="buy", price=1410.0,
            reason={"params": {}, "prev_position": 0, "cur_position": 1},
        ))
        db.commit()

        result = list_signals(
            date_=None, code=None, strategy=None, side=None, limit=20, db=db,
        )

        item = result["items"][0]
        assert item["name"] == "贵州茅台"
        assert item["industry"] == "白酒"
        assert item["strategy_name"] == "双均线趋势策略"
        assert item["side_name"] == "入场提示"
        assert item["reason_type"] == "position_change"
        assert item["reason_text"] == "5日均线上穿20日均线，策略模拟状态变为持有。"


def test_strategy_list_keeps_legacy_keys_and_adds_catalog_items():
    result = list_strategies()
    assert result["strategies"] == [
        "breakout", "ma_cross", "mean_reversion", "momentum_rotation",
        "multifactor_hold", "volume_breakout",
    ]
    assert set(result["single"]) | set(result["portfolio"]) == set(result["strategies"])
    assert {item["key"] for item in result["items"]} == set(result["strategies"])
    assert all(item["params"] for item in result["items"])


def test_kline_response_has_stock_metadata():
    with _session() as db:
        _seed_stock_and_bar(db)
        result = get_kline(
            code="sh.600519", start=None, end=None, db=db,
        )
        assert result["name"] == "贵州茅台"
        assert result["industry"] == "白酒"


def test_stock_search_supports_name_six_digits_and_full_code():
    with _session() as db:
        _seed_search_stocks(db)

        by_name = search_stocks(q="茅台", limit=20, db=db)
        assert [item["code"] for item in by_name["items"]] == ["sh.600519"]
        assert by_name["items"][0]["name"] == "贵州茅台"
        assert by_name["items"][0]["industry"] == "白酒"

        by_symbol = search_stocks(q="000001", limit=20, db=db)
        assert [item["code"] for item in by_symbol["items"]] == ["sz.000001"]

        by_full_code = search_stocks(q="SH.600519", limit=20, db=db)
        assert [item["code"] for item in by_full_code["items"]] == ["sh.600519"]

        by_beijing_symbol = search_stocks(q="430047", limit=20, db=db)
        assert [item["code"] for item in by_beijing_symbol["items"]] == ["bj.430047"]

        by_new_beijing_symbol = search_stocks(q="920001", limit=20, db=db)
        assert [item["code"] for item in by_new_beijing_symbol["items"]] == ["bj.920001"]


def test_stock_search_supports_code_prefix_and_honors_limit():
    with _session() as db:
        _seed_search_stocks(db)
        result = search_stocks(q="600", limit=1, db=db)
        assert result["count"] == 1
        assert result["items"][0]["code"] == "sh.600000"


def test_empty_stock_search_returns_watchlist_only():
    with _session() as db:
        _seed_search_stocks(db)
        result = search_stocks(q="  ", limit=100, db=db)
        assert [item["code"] for item in result["items"]] == [
            "bj.430047", "sh.600000",
        ]
        assert all(item["is_watch"] for item in result["items"])


def test_stock_name_search_escapes_sql_wildcards():
    with _session() as db:
        _seed_search_stocks(db)
        assert search_stocks(q="%", limit=100, db=db)["items"] == []
        assert search_stocks(q="_", limit=100, db=db)["items"] == []


def test_manual_portfolio_responses_have_stock_metadata():
    with _session() as db:
        _seed_stock_and_bar(db)
        db.add(Trade(
            id=1, code="sh.600519", trade_date=date(2026, 7, 24),
            side="buy", price=1400.0, qty=100.0, fee=5.0, note="手工记录",
        ))
        db.commit()

        trades = list_trades(code=None, db=db)
        assert trades["items"][0]["name"] == "贵州茅台"
        assert trades["items"][0]["side_name"] == "买入"

        positions = get_positions(db=db)
        assert positions["positions"][0]["name"] == "贵州茅台"
        assert positions["positions"][0]["industry"] == "白酒"


def test_add_trade_rejects_unknown_stock_code():
    with _session() as db:
        body = TradeIn(
            code="sh.999999", trade_date=date(2026, 7, 24),
            side="buy", price=10.0, qty=100.0,
        )

        with pytest.raises(HTTPException) as exc_info:
            add_trade(body, db=db)

        assert exc_info.value.status_code == 422
        assert exc_info.value.detail == (
            "股票代码 sh.999999 不存在，请先从股票搜索结果中选择"
        )
        assert list_trades(code=None, db=db) == {"count": 0, "items": []}
