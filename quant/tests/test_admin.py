"""管理接口后台任务的线程、Session 与错误映射行为测试。"""
from __future__ import annotations

import asyncio
import sys
from datetime import date
from pathlib import Path

import pytest
from fastapi import HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api import admin


class SessionProbe:
    def __init__(self) -> None:
        self.entered = False
        self.exited = False

    def __enter__(self):
        self.entered = True
        return self

    def __exit__(self, exc_type, exc, traceback) -> None:
        self.exited = True


def test_run_signals_uses_scoped_session_and_returns_job_result(monkeypatch):
    session = SessionProbe()
    day = date(2026, 7, 24)
    seen: dict = {}
    monkeypatch.setattr(admin, "SessionLocal", lambda: session)

    def run(db, day=None):
        seen.update(db=db, day=day)
        return {"signals": 3}

    monkeypatch.setattr(admin, "run_signals", run)

    result = asyncio.run(admin.run_signals_now(day))

    assert result == {"signals": 3}
    assert seen == {"db": session, "day": day}
    assert session.entered is True
    assert session.exited is True


def test_import_stocks_maps_provider_failure_to_502_and_closes_session(
    monkeypatch,
):
    session = SessionProbe()
    monkeypatch.setattr(admin, "SessionLocal", lambda: session)

    def fail(_db):
        raise RuntimeError("provider unavailable")

    monkeypatch.setattr(admin.ingest, "import_stock_list", fail)

    with pytest.raises(HTTPException) as caught:
        asyncio.run(admin.import_stocks())

    assert caught.value.status_code == 502
    assert caught.value.detail == "股票列表导入失败: provider unavailable"
    assert session.exited is True


def test_trade_calendar_keeps_validation_error_as_422(monkeypatch):
    session = SessionProbe()
    monkeypatch.setattr(admin, "SessionLocal", lambda: session)

    def invalid(_db, *, start, end):
        assert start is None
        assert end is None
        raise ValueError("start 必须早于 end")

    monkeypatch.setattr(admin.trade_calendar, "sync_trade_calendar", invalid)

    with pytest.raises(HTTPException) as caught:
        asyncio.run(admin.sync_trade_calendar_now(start=None, end=None))

    assert caught.value.status_code == 422
    assert caught.value.detail == "start 必须早于 end"
    assert session.exited is True
