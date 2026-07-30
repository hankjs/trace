"""全局异步任务系统:submit/claim/cancel/handler 分发与回测任务链路。"""
from __future__ import annotations

import sys
from datetime import date, datetime, timedelta
from pathlib import Path

import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import tasks as tasks_mod
from app.db import Base
from app.models import (
    SYSTEM_OWNER_ID, BacktestRun, DailyBar, Stock, Strategy, Task,
)
from app.tasks import (
    TaskConflictError, cancel_task, recover_tasks, register_handler,
    run_task, submit_task,
)

START = date(2024, 1, 2)
END = date(2024, 6, 28)


def _session_local(monkeypatch):
    """单个内存库同时作为请求 Session 与 worker SessionLocal。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    SessionLocal = sessionmaker(bind=engine)
    monkeypatch.setattr(tasks_mod, "SessionLocal", SessionLocal)
    return SessionLocal


def _add_task(db: Session, **kwargs) -> Task:
    task = Task(
        user_id=kwargs.pop("user_id", "u1"),
        type=kwargs.pop("type", "dummy"),
        status=kwargs.pop("status", "pending"),
        title=kwargs.pop("title", "t"),
        created_at=datetime.now(),
        **kwargs,
    )
    db.add(task)
    db.commit()
    db.refresh(task)
    return task


def test_submit_conflict_per_user(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    register_handler("dummy", lambda db, task: {"ok": True})
    with SessionLocal() as db:
        _add_task(db, user_id="u1", status="running")
        with pytest.raises(TaskConflictError):
            submit_task(db, user_id="u1", type="dummy", title="x")
        # 不同用户互不影响;测试模式内联执行,返回即终态
        task = submit_task(db, user_id="u2", type="dummy", title="x")
        db.refresh(task)
        assert task.status == "done"
        assert task.result == {"ok": True}


def test_run_task_passes_params_and_stores_result(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    seen = {}

    def handler(db, task):
        seen.update(task.params)
        return {"echo": task.params}

    register_handler("echo", handler)
    with SessionLocal() as db:
        task = submit_task(
            db, user_id="u1", type="echo", title="x",
            params={"a": 1}, ref_id=42,
        )
        db.refresh(task)
        assert seen == {"a": 1}
        assert task.status == "done"
        assert task.result == {"echo": {"a": 1}}
        assert task.ref_id == 42
        assert task.started_at is not None
        assert task.finished_at is not None


def test_run_task_claim_only_once(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    calls = []

    def handler(db, task):
        calls.append(task.id)
        return None

    register_handler("counting", handler)
    with SessionLocal() as db:
        task = _add_task(db, type="counting")
        run_task(task.id)
        run_task(task.id)  # 已 done,不可再抢占
        assert calls == [task.id]


def test_failed_handler_marks_failed(monkeypatch):
    SessionLocal = _session_local(monkeypatch)

    def boom(db, task):
        raise RuntimeError("爆掉" * 3000)

    register_handler("boom", boom)
    with SessionLocal() as db:
        task = _add_task(db, type="boom")
        run_task(task.id)
        db.refresh(task)
        assert task.status == "failed"
        assert task.error.startswith("爆掉")
        assert len(task.error) <= 4000
        assert task.finished_at is not None


def test_unregistered_type_marks_failed(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    with SessionLocal() as db:
        task = _add_task(db, type="ghost")
        run_task(task.id)
        db.refresh(task)
        assert task.status == "failed"
        assert "未注册的任务类型" in task.error


def test_cancel_pending_task(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    calls = []
    register_handler("lazy", lambda db, task: calls.append(task.id))
    with SessionLocal() as db:
        task = _add_task(db, type="lazy")
        assert cancel_task(db, task) is True
        db.refresh(task)
        assert task.status == "cancelled"
        assert task.finished_at is not None
        run_task(task.id)  # 已取消,handler 不应执行
        assert calls == []
        # 终态不可再取消
        assert cancel_task(db, task) is False


def test_cancel_running_returns_false(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    with SessionLocal() as db:
        task = _add_task(db, status="running")
        assert cancel_task(db, task) is False


def test_cancel_backtest_task_marks_run(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    with SessionLocal() as db:
        run = BacktestRun(
            user_id="u1", strategy_id=1, start=START, end=END,
            status="pending", codes=["sh.600519"], created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        task = _add_task(db, type="backtest", ref_id=run.id)
        assert cancel_task(db, task) is True
        db.refresh(run)
        assert run.status == "cancelled"


def test_recover_tasks_marks_stale_running(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    with SessionLocal() as db:
        run = BacktestRun(
            user_id="u1", strategy_id=1, start=START, end=END,
            status="running", codes=["sh.600519"], created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        stale = _add_task(db, type="backtest", status="running", ref_id=run.id)
        pending = _add_task(db, user_id="u2", type="dummy")
        recover_tasks()
        db.refresh(stale)
        db.refresh(run)
        db.refresh(pending)
        assert stale.status == "failed"
        assert "服务重启" in stale.error
        assert run.status == "failed"
        # task_async=False(测试):pending 不重新派发,留在队列里
        assert pending.status == "pending"


def _seed_bars(db: Session, code: str = "sh.600519") -> None:
    db.add(Stock(code=code, name="测试", list_date=date(2015, 1, 1), is_st=False))
    rows = []
    d = START - timedelta(days=250)
    price = 10.0
    while d <= END:
        price *= 1.001
        rows.append(DailyBar(
            code=code, date=d,
            open=price, high=price * 1.01, low=price * 0.99, close=price,
            raw_close=price, volume=1e6, amount=1e7, is_st=False,
        ))
        d += timedelta(days=1)
    db.add_all(rows)
    db.commit()


def test_backtest_task_handler_completes(monkeypatch):
    """backtest 任务:worker 执行冻结的 BacktestRun,任务与 run 都到 done。"""
    from app import db as app_db
    from app.backtest import jobs as jobs_mod
    from app.api import backtest as backtest_api  # noqa: F401 - 触发 handler 注册
    from app.strategy.presets import get_preset_spec
    from app.strategy.spec import strategy_spec_hash

    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    SessionLocal = sessionmaker(bind=engine)
    monkeypatch.setattr(app_db, "SessionLocal", SessionLocal)
    monkeypatch.setattr(jobs_mod, "SessionLocal", SessionLocal)
    monkeypatch.setattr(tasks_mod, "SessionLocal", SessionLocal)

    with SessionLocal() as db:
        _seed_bars(db)
        spec = get_preset_spec("breakout")
        strategy = Strategy(
            owner_id=SYSTEM_OWNER_ID, is_system=True,
            name="突破", template="breakout", kind="single",
            params={}, spec=spec.model_dump(mode="json"),
            spec_hash=strategy_spec_hash(spec),
            research_status="unverified", enabled=True,
        )
        db.add(strategy)
        db.flush()
        run = BacktestRun(
            user_id="u1",
            strategy_id=strategy.id,
            params={},
            costs={},
            codes=["sh.600519"],
            start=START,
            end=END,
            strategy_spec_snapshot=spec.model_dump(mode="json"),
            strategy_spec_hash=strategy_spec_hash(spec),
            status="pending",
            request_snapshot={
                "codes": ["sh.600519"],
                "params": {},
                "costs": {},
                "dynamic_universe": False,
                "pool_id": None,
            },
            created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        task = _add_task(db, type="backtest", ref_id=run.id)
        task_id, run_id = task.id, run.id

    run_task(task_id)

    with SessionLocal() as db:
        task = db.get(Task, task_id)
        run = db.get(BacktestRun, run_id)
        assert task.status == "done"
        assert run.status == "done"
        assert run.metrics is not None
