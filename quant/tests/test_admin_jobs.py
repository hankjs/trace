"""定时任务查看/手动触发/执行日志(quant_job_run)测试。"""
from __future__ import annotations

import asyncio
import sys
import threading
import time
from pathlib import Path

import pytest
from fastapi import HTTPException
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import job_log
from app import scheduler as sched
from app import scheduler_lock
from app.api import admin
from app.db import Base

ADMIN_CLAIMS = {"username": "admin", "can_admin": True}


@pytest.fixture()
def log_db(monkeypatch, tmp_path):
    """job_log 与 admin 端点共用的测试库。

    用临时文件库而非内存库:手动触发在后台线程写库,主线程同时轮询,
    StaticPool 单连接并发使用会让写入偶发失败(被 job_log 旁路吞掉,
    行停在 running);文件库每个 Session 独立连接,行为接近生产 MySQL。
    """
    engine = create_engine(
        f"sqlite+pysqlite:///{tmp_path}/job_test.db",
        connect_args={"check_same_thread": False},
    )
    Base.metadata.create_all(engine)
    testing_session = sessionmaker(bind=engine)
    monkeypatch.setattr(job_log, "SessionLocal", testing_session)
    monkeypatch.setattr(admin, "SessionLocal", testing_session)
    monkeypatch.setattr(scheduler_lock, "engine", engine)
    return testing_session


def test_job_registry_matches_scheduled_ids():
    """注册表 id 必须与 start_scheduler 中 add_job 的 id 一一对应,防漂移。"""
    registry_ids = {j["id"] for j in sched.JOB_DEFS}
    scheduled_ids = {
        "evening_pipeline",
        "sync_index_members",
        "sync_trade_calendar",
        "sync_stock_list",
        "sync_valuations",
        "sync_fundamentals",
        "prune_research_plans",
        "intraday_snapshot",
    }
    assert registry_ids == scheduled_ids
    for job in sched.JOB_DEFS:
        assert callable(job["func"])
        assert job["name"] and job["schedule"] and job["description"]


def test_list_jobs_empty_history(log_db):
    result = asyncio.run(admin.list_jobs())
    assert result["scheduler_running"] is False
    jobs = result["jobs"]
    assert [j["id"] for j in jobs] == [j["id"] for j in sched.JOB_DEFS]
    for job in jobs:
        assert job["next_run_time"] is None
        assert job["last_system_run"] is None
        assert job["manual_run"] is None


class _FakeEvent:
    def __init__(self, job_id, exception=None, retval=None):
        self.job_id = job_id
        self.exception = exception
        self.retval = retval


def test_system_run_recorded_to_db(log_db):
    sched._record_system_run(_FakeEvent("intraday_snapshot", retval={"n": 3}))
    sched._record_system_run(
        _FakeEvent("sync_valuations", exception=RuntimeError("boom")))

    jobs = {j["id"]: j for j in asyncio.run(admin.list_jobs())["jobs"]}
    ok = jobs["intraday_snapshot"]["last_system_run"]
    assert ok["status"] == "finished"
    assert ok["result"] == {"n": 3}
    assert ok["started_at"] and ok["finished_at"]
    bad = jobs["sync_valuations"]["last_system_run"]
    assert bad["status"] == "failed"
    assert "boom" in bad["error"]


def _wait_run_finished(job_id: str, timeout: float = 5.0) -> dict:
    deadline = time.time() + timeout
    while time.time() < deadline:
        latest = job_log.latest_runs().get((job_id, job_log.TRIGGER_MANUAL))
        if latest and latest["status"] != "running":
            return latest
        time.sleep(0.02)
    raise AssertionError(f"{job_id} 手动执行未在 {timeout}s 内完成")


def test_run_job_unknown_id_returns_404(log_db):
    with pytest.raises(HTTPException) as exc:
        asyncio.run(admin.run_job("no_such_job", ADMIN_CLAIMS))
    assert exc.value.status_code == 404


def test_run_job_executes_and_persists(log_db, monkeypatch):
    monkeypatch.setitem(
        sched.job_def("sync_trade_calendar"), "func", lambda: {"synced": 7})
    result = asyncio.run(admin.run_job("sync_trade_calendar", ADMIN_CLAIMS))
    assert result == {"status": "started", "job_id": "sync_trade_calendar"}

    run = _wait_run_finished("sync_trade_calendar")
    assert run["status"] == "finished"
    assert run["result"] == {"synced": 7}
    assert run["operator"] == "admin"
    assert run["finished_at"] is not None

    history = asyncio.run(admin.job_runs("sync_trade_calendar", limit=20))
    assert len(history) == 1
    assert history[0]["trigger"] == "manual"


def test_run_job_concurrent_trigger_returns_409(log_db, monkeypatch):
    gate = threading.Event()

    def slow():
        gate.wait(5)

    monkeypatch.setitem(sched.job_def("sync_stock_list"), "func", slow)
    try:
        asyncio.run(admin.run_job("sync_stock_list", ADMIN_CLAIMS))
        with pytest.raises(HTTPException) as exc:
            asyncio.run(admin.run_job("sync_stock_list", ADMIN_CLAIMS))
        assert exc.value.status_code == 409
    finally:
        gate.set()
    assert _wait_run_finished("sync_stock_list")["status"] == "finished"


def test_run_job_failure_recorded(log_db, monkeypatch):
    def boom():
        raise RuntimeError("provider down")

    monkeypatch.setitem(sched.job_def("sync_index_members"), "func", boom)
    asyncio.run(admin.run_job("sync_index_members", ADMIN_CLAIMS))
    run = _wait_run_finished("sync_index_members")
    assert run["status"] == "failed"
    assert "provider down" in run["error"]


def test_stale_running_closed_on_next_trigger(log_db):
    """进程崩溃遗留的 running 行,在下次手动触发时被标记为 failed。"""
    from datetime import datetime

    stale_id = job_log.record_run(
        "sync_fundamentals", job_log.TRIGGER_MANUAL, job_log.STATUS_RUNNING,
        started_at=datetime.now())
    job_log.fail_stale_running("sync_fundamentals")
    latest = job_log.latest_runs()[
        ("sync_fundamentals", job_log.TRIGGER_MANUAL)]
    assert latest["id"] == stale_id
    assert latest["status"] == "failed"
    assert "中断" in latest["error"]


def test_job_runs_unknown_id_returns_404(log_db):
    with pytest.raises(HTTPException) as exc:
        asyncio.run(admin.job_runs("no_such_job", limit=20))
    assert exc.value.status_code == 404
