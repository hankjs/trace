"""定时任务查看与手动触发端点测试(/api/admin/jobs)。"""
from __future__ import annotations

import asyncio
import sys
import threading
import time
from pathlib import Path

import pytest
from fastapi import HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import scheduler as sched
from app.api import admin


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
        "intraday_snapshot",
    }
    assert registry_ids == scheduled_ids
    for job in sched.JOB_DEFS:
        assert callable(job["func"])
        assert job["name"] and job["schedule"] and job["description"]


def test_list_jobs_returns_registry_without_running_scheduler():
    result = asyncio.run(admin.list_jobs())
    assert result["scheduler_running"] is False
    jobs = result["jobs"]
    assert [j["id"] for j in jobs] == [j["id"] for j in sched.JOB_DEFS]
    for job in jobs:
        assert job["next_run_time"] is None
        assert job["manual_run"] is None


def test_run_job_unknown_id_returns_404():
    with pytest.raises(HTTPException) as exc:
        asyncio.run(admin.run_job("no_such_job"))
    assert exc.value.status_code == 404


def test_run_job_executes_in_thread_and_records_result(monkeypatch):
    monkeypatch.setitem(
        sched.job_def("sync_trade_calendar"), "func", lambda: {"synced": 7})
    admin._manual_runs.pop("sync_trade_calendar", None)
    result = asyncio.run(admin.run_job("sync_trade_calendar"))
    assert result == {"status": "started", "job_id": "sync_trade_calendar"}
    # 后台线程执行,等待完成
    deadline = time.time() + 5
    while time.time() < deadline:
        run = admin._manual_runs.get("sync_trade_calendar")
        if run and run["status"] != "running":
            break
        time.sleep(0.02)
    run = admin._manual_runs["sync_trade_calendar"]
    assert run["status"] == "finished"
    assert run["result"] == {"synced": 7}
    assert run["finished_at"] is not None


def test_run_job_concurrent_trigger_returns_409(monkeypatch):
    gate = threading.Event()

    def slow():
        gate.wait(5)

    monkeypatch.setitem(sched.job_def("sync_stock_list"), "func", slow)
    admin._manual_runs.pop("sync_stock_list", None)
    try:
        asyncio.run(admin.run_job("sync_stock_list"))
        with pytest.raises(HTTPException) as exc:
            asyncio.run(admin.run_job("sync_stock_list"))
        assert exc.value.status_code == 409
    finally:
        gate.set()


def test_run_job_failure_recorded(monkeypatch):
    def boom():
        raise RuntimeError("provider down")

    monkeypatch.setitem(sched.job_def("sync_index_members"), "func", boom)
    admin._manual_runs.pop("sync_index_members", None)
    asyncio.run(admin.run_job("sync_index_members"))
    deadline = time.time() + 5
    while time.time() < deadline:
        run = admin._manual_runs.get("sync_index_members")
        if run and run["status"] != "running":
            break
        time.sleep(0.02)
    run = admin._manual_runs["sync_index_members"]
    assert run["status"] == "failed"
    assert "provider down" in run["error"]
