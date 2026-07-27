"""调度器测试:交易日历判断、Session 隔离、流水线门限。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session, sessionmaker
from sqlalchemy.pool import StaticPool

from app import scheduler
from app.models import TradeCalendar
from app.db import Base
from app.models import DailyBar, IndexMember


def _memory_sessionmaker():
    """共享内存库:scheduler 每批新开 Session,必须连同一个库。"""
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return sessionmaker(bind=engine, autoflush=False, expire_on_commit=False)


def _no_baostock_login(monkeypatch):
    """login_session 在测试里不得真连 baostock。"""
    import contextlib

    monkeypatch.setattr(scheduler.baostock_client, "login_session",
                        contextlib.nullcontext)


def test_evening_pipeline_stops_when_any_bar_update_fails(monkeypatch):
    selection_called = False

    monkeypatch.setattr(scheduler, "_is_trading_day", lambda *_: True)
    monkeypatch.setattr(
        scheduler, "job_daily_bars",
        lambda *_: {"skipped": False, "succeeded": 799,
                    "failed": ["sh.600001"], "empty": [], "empty_ratio": 0.0},
    )

    def selection():
        nonlocal selection_called
        selection_called = True
        return {"picked": 30}

    monkeypatch.setattr(scheduler, "job_factors_and_selection", selection)
    scheduler.job_evening_pipeline()

    assert selection_called is False


def test_evening_pipeline_stops_when_most_codes_have_no_bar(monkeypatch):
    """空帧不算 failed,但整体空帧占比过高时同样必须中止发布。"""
    selection_called = False

    monkeypatch.setattr(scheduler, "_is_trading_day", lambda *_: True)
    monkeypatch.setattr(
        scheduler, "job_daily_bars",
        lambda *_: {"skipped": False, "succeeded": 100, "failed": [],
                    "empty": [f"sh.{i}" for i in range(90)],
                    "empty_ratio": 0.9},
    )
    monkeypatch.setattr(
        scheduler, "job_factors_and_selection",
        lambda: (_ for _ in ()).throw(AssertionError("不应进入选股阶段")),
    )
    scheduler.job_evening_pipeline()
    assert selection_called is False


def test_holiday_skips_ingest_and_reconcile(monkeypatch):
    """节假日:日历标 is_open=False -> 不采集、不对账,不产生假告警。"""
    maker = _memory_sessionmaker()
    holiday = date(2026, 10, 1)  # 国庆,周四(工作日但非交易日)
    assert holiday.weekday() < 5, "用例前提:该日是工作日,才能证明日历生效"
    with maker() as db:
        db.add(TradeCalendar(date=holiday, is_open=False, source="test"))
        db.add(IndexMember(index_name="hs300", code="sh.600519",
                           in_date=date(2020, 1, 1)))
        db.commit()

    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(scheduler.trade_calendar, "SessionLocal", maker,
                        raising=False)
    _no_baostock_login(monkeypatch)
    calls: list[str] = []
    monkeypatch.setattr(
        scheduler.ingest, "ingest_daily",
        lambda *a, **kw: calls.append(a[1]) or {"has_day_bar": True},
    )

    result = scheduler.job_daily_bars(holiday)

    assert result["skipped"] is True
    assert calls == []            # 一次采集都没发生
    assert result["failed"] == []  # 也没有任何失败/告警


def test_trading_day_runs_ingest(monkeypatch):
    """同一天若日历标 is_open=True 则照常采集(对照组)。"""
    maker = _memory_sessionmaker()
    day = date(2026, 10, 9)
    with maker() as db:
        db.add(TradeCalendar(date=day, is_open=True, source="test"))
        db.add(IndexMember(index_name="hs300", code="sh.600519",
                           in_date=date(2020, 1, 1)))
        db.commit()

    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    _no_baostock_login(monkeypatch)
    calls: list[str] = []
    monkeypatch.setattr(
        scheduler.ingest, "ingest_daily",
        lambda db, code, **kw: (calls.append(code), {"has_day_bar": True})[1],
    )
    monkeypatch.setattr(scheduler.ingest, "cleanup_snapshots", lambda *a: 0)

    result = scheduler.job_daily_bars(day)

    assert result["skipped"] is False
    assert calls == ["sh.600519"]
    assert result["succeeded"] == 1


def test_session_poison_isolated_failing_code_does_not_break_rest(monkeypatch):
    """50 只中第 10 只把 Session 弄脏,第 11-50 只仍须成功入库。"""
    maker = _memory_sessionmaker()
    day = date(2026, 7, 24)
    codes = [f"sh.{600000 + i}" for i in range(50)]
    with maker() as db:
        db.add(TradeCalendar(date=day, is_open=True, source="test"))
        for code in codes:
            db.add(IndexMember(index_name="hs300", code=code,
                               in_date=date(2020, 1, 1)))
        db.commit()

    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(scheduler, "INGEST_BATCH_SIZE", 50)  # 全部挤进一个 Session
    _no_baostock_login(monkeypatch)
    monkeypatch.setattr(scheduler.ingest, "cleanup_snapshots", lambda *a: 0)

    bad = codes[9]

    def fake_ingest(db: Session, code: str, day=None, reconcile=False):
        if code == bad:
            # 制造真实的 Session 中毒:非法行入库后 flush 失败
            db.add(DailyBar(code=None, date=day, open=1, high=1,
                            low=1, close=1, raw_close=1, volume=1, amount=1))
            db.flush()
            raise AssertionError("不应到达:flush 必须抛错")
        db.add(DailyBar(code=code, date=day, open=10, high=10, low=9,
                        close=10, raw_close=10, volume=1, amount=10))
        db.commit()
        return {"code": code, "upserted": 1, "has_day_bar": True}

    monkeypatch.setattr(scheduler.ingest, "ingest_daily", fake_ingest)

    result = scheduler.job_daily_bars(day)

    assert result["failed"] == [bad]
    assert result["succeeded"] == 49
    with maker() as db:
        stored = {r[0] for r in db.execute(select(DailyBar.code)).all()}
    # 坏票后面的 40 只全部入库,证明失效事务已被隔离
    for code in codes[10:]:
        assert code in stored, f"{code} 未入库,Session 中毒未被隔离"
    assert bad not in stored


def test_valuation_job_uses_one_market_batch(monkeypatch):
    maker = _memory_sessionmaker()
    day = date(2026, 7, 24)
    calls: list[date] = []

    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(scheduler, "_is_trading_day", lambda *_: True)
    monkeypatch.setattr(
        scheduler.fundamentals,
        "sync_market_valuations",
        lambda db, value: calls.append(value) or {"upserted": 5000},
    )

    result = scheduler.job_sync_valuations(day)

    assert calls == [day]
    assert result == {"upserted": 5000}


def test_financial_job_refreshes_recent_market_periods(monkeypatch):
    maker = _memory_sessionmaker()
    day = date(2026, 7, 27)
    captured: list[list[date]] = []

    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(
        scheduler.fundamentals,
        "sync_market_financials",
        lambda db, periods: captured.append(periods) or {"upserted": 20000},
    )

    result = scheduler.job_sync_fundamentals(day)

    assert captured == [[
        date(2025, 6, 30),
        date(2025, 9, 30),
        date(2025, 12, 31),
        date(2026, 3, 31),
        date(2026, 6, 30),
    ]]
    assert result == {"upserted": 20000}
