"""交易日历采集与查询。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import calendar as trade_calendar
from app.data.compat import TradeCalendar
from app.db import Base


def _session() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


def _patch_remote(monkeypatch, rows: list[tuple[date, bool]]) -> None:
    monkeypatch.setattr(
        trade_calendar.baostock_client, "fetch_trade_dates",
        lambda start, end: pd.DataFrame(
            [{"date": d, "is_open": o} for d, o in rows]),
    )


def test_sync_writes_calendar_rows(monkeypatch):
    rows = [(date(2026, 10, 1), False), (date(2026, 10, 9), True)]
    _patch_remote(monkeypatch, rows)
    with _session() as db:
        result = trade_calendar.sync_trade_calendar(
            db, date(2026, 10, 1), date(2026, 10, 9))
        assert result["days"] == 2
        assert result["open_days"] == 1
        stored = dict(db.execute(
            select(TradeCalendar.date, TradeCalendar.is_open)).all())
        assert stored[date(2026, 10, 1)] is False
        assert stored[date(2026, 10, 9)] is True


def test_sync_skips_on_empty_remote(monkeypatch):
    """远端空响应按异常处理:不写库,更不能把整段标成休市。"""
    with _session() as db:
        db.add(TradeCalendar(date=date(2026, 10, 9), is_open=True, source="test"))
        db.commit()
        monkeypatch.setattr(
            trade_calendar.baostock_client, "fetch_trade_dates",
            lambda start, end: pd.DataFrame(columns=["date", "is_open"]),
        )
        result = trade_calendar.sync_trade_calendar(
            db, date(2026, 10, 1), date(2026, 10, 31))
        assert result["skipped"] is True
        assert trade_calendar.is_trading_day(db, date(2026, 10, 9)) is True


def test_sync_updates_changed_flag(monkeypatch):
    """日历修订(临时休市)必须能覆盖已有行。"""
    with _session() as db:
        db.add(TradeCalendar(date=date(2026, 10, 9), is_open=True, source="test"))
        db.commit()
        _patch_remote(monkeypatch, [(date(2026, 10, 9), False)])
        result = trade_calendar.sync_trade_calendar(
            db, date(2026, 10, 9), date(2026, 10, 9))
        assert result["changed"] == 1
        assert trade_calendar.is_trading_day(db, date(2026, 10, 9)) is False


def test_holiday_on_weekday_is_not_trading_day():
    """国庆(周四)在日历里标休市 -> 非交易日。这是 _is_weekday 的原始缺陷。"""
    holiday = date(2026, 10, 1)
    assert holiday.weekday() < 5
    with _session() as db:
        db.add(TradeCalendar(date=holiday, is_open=False, source="test"))
        db.commit()
        assert trade_calendar.is_trading_day(db, holiday) is False


def test_missing_calendar_falls_back_to_weekday_with_warning(caplog):
    """日历缺该日:降级为工作日判断并告警,不得静默停掉整条 pipeline。"""
    with _session() as db:
        with caplog.at_level("WARNING"):
            assert trade_calendar.is_trading_day(db, date(2026, 7, 23)) is True
            assert trade_calendar.is_trading_day(db, date(2026, 7, 25)) is False
        assert any("交易日历缺少" in r.getMessage() for r in caplog.records)


def test_last_trading_day_skips_holidays():
    with _session() as db:
        db.add_all([
            TradeCalendar(date=date(2026, 9, 30), is_open=True, source="test"),
            TradeCalendar(date=date(2026, 10, 1), is_open=False, source="test"),
            TradeCalendar(date=date(2026, 10, 2), is_open=False, source="test"),
        ])
        db.commit()
        assert trade_calendar.last_trading_day(db, date(2026, 10, 2)) == \
            date(2026, 9, 30)


def test_ensure_calendar_loaded_triggers_sync(monkeypatch):
    _patch_remote(monkeypatch, [(date(2026, 7, 24), True)])
    with _session() as db:
        assert trade_calendar.has_calendar(db, date(2026, 7, 24)) is False
        assert trade_calendar.ensure_calendar_loaded(db, date(2026, 7, 24)) is True
        assert trade_calendar.has_calendar(db, date(2026, 7, 24)) is True


def test_ensure_calendar_loaded_survives_sync_failure(monkeypatch):
    """日历同步失败不得抛出打断采集。"""
    def boom(start, end):
        raise RuntimeError("baostock 挂了")

    monkeypatch.setattr(trade_calendar.baostock_client, "fetch_trade_dates", boom)
    with _session() as db:
        assert trade_calendar.ensure_calendar_loaded(db, date(2026, 7, 24)) is False


def test_fetch_trade_dates_parses_baostock_flags(monkeypatch):
    """query_trade_dates 的 '1'/'0' 字符串正确解析为 bool。"""
    from unittest import mock

    from app.data import baostock_client as bc

    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = mock.Mock(error_code="0")
        rows = [["2026-10-01", "0"], ["2026-10-09", "1"]]
        rs = mock.Mock(error_code="0",
                       fields=["calendar_date", "is_trading_day"])
        rs.next.side_effect = [True, True, False]
        rs.get_row_data.side_effect = rows
        bs.query_trade_dates.return_value = rs

        df = bc.fetch_trade_dates("2026-10-01", "2026-10-09")

    assert list(df["date"]) == [date(2026, 10, 1), date(2026, 10, 9)]
    assert list(df["is_open"]) == [False, True]
