"""回测取消检查点与列表查询测试。"""
from __future__ import annotations

import threading
import time
from contextlib import contextmanager
from datetime import date, datetime, timedelta

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session, sessionmaker

from app import tasks as tasks_mod
from app.backtest.engine import BacktestCancelledError, run_backtest
from app.backtest.jobs import execute_backtest_run
from app.backtest.listing import list_runs
from app.db import Base
from app.models import BacktestRun, DailyBar, Stock, Strategy, Task
from app.strategy.presets import get_preset_spec
from app.strategy.spec import strategy_spec_hash
from app.tasks import run_task, submit_task

USER_ID = "11111111-1111-1111-1111-111111111111"
OTHER_ID = "22222222-2222-2222-2222-222222222222"
START = date(2024, 1, 2)
END = date(2024, 2, 2)


def _session_local(monkeypatch):
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    SessionLocal = sessionmaker(bind=engine)
    monkeypatch.setattr(tasks_mod, "SessionLocal", SessionLocal)
    from app import db as app_db
    from app.backtest import jobs as jobs_mod
    monkeypatch.setattr(app_db, "SessionLocal", SessionLocal)
    monkeypatch.setattr(jobs_mod, "SessionLocal", SessionLocal)
    return SessionLocal


def _seed_strategy_and_bars(db: Session) -> Strategy:
    spec = get_preset_spec("breakout")
    strategy = Strategy(
        owner_id=USER_ID,
        is_system=False,
        name="突破",
        template="breakout",
        kind="single",
        params={},
        spec=spec.model_dump(mode="json"),
        spec_hash=strategy_spec_hash(spec),
        research_status="unverified",
        enabled=True,
    )
    db.add(strategy)
    db.flush()

    code = "sh.600519"
    db.add(Stock(code=code, name="测试", list_date=date(2015, 1, 1), is_st=False))
    price = 10.0
    d = START - timedelta(days=100)
    while d <= END:
        price *= 1.001
        db.add(DailyBar(
            code=code, date=d,
            open=price, high=price * 1.01, low=price * 0.99, close=price,
            raw_close=price, volume=1e6, amount=1e7, is_st=False,
        ))
        d += timedelta(days=1)
    db.commit()
    return strategy


@contextmanager
def _cancel_for_task(task_id: int):
    """在 run_task 把事件注册到 _cancel_events 后立即置位。"""
    def _set_event():
        deadline = time.time() + 1
        while task_id not in tasks_mod._cancel_events:
            if time.time() > deadline:
                return
            time.sleep(0.001)
        tasks_mod._cancel_events[task_id].set()

    t = threading.Thread(target=_set_event)
    t.start()
    try:
        yield
    finally:
        t.join(timeout=2)


def test_run_backtest_checks_cancel_event():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        strategy = _seed_strategy_and_bars(db)
        event = threading.Event()
        event.set()
        with pytest.raises(BacktestCancelledError):
            run_backtest(
                db, strategy, ["sh.600519"], START, END,
                execution_spec=strategy.spec,
                cancel_event=event,
            )


def test_execute_backtest_run_cancellation_marks_run_cancelled(monkeypatch):
    SessionLocal = _session_local(monkeypatch)
    # 触发 api/backtest.py 的 handler 注册
    from app.api import backtest as _  # noqa: F401

    with SessionLocal() as db:
        strategy = _seed_strategy_and_bars(db)
        run = BacktestRun(
            user_id=USER_ID,
            strategy_id=strategy.id,
            params={},
            costs={},
            codes=["sh.600519"],
            start=START,
            end=END,
            strategy_spec_snapshot=strategy.spec,
            strategy_spec_hash=strategy_spec_hash(strategy.spec),
            status="pending",
            request_snapshot={
                "codes": ["sh.600519"], "params": {}, "costs": {},
                "dynamic_universe": False, "pool_id": None,
            },
            created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        task = Task(
            user_id=USER_ID, type="backtest", status="pending",
            title="test", ref_id=run.id, created_at=datetime.now(),
        )
        db.add(task)
        db.commit()
        task_id, run_id = task.id, run.id

    with _cancel_for_task(task_id):
        run_task(task_id)

    with SessionLocal() as db:
        run = db.get(BacktestRun, run_id)
        task = db.get(Task, task_id)
        assert run.status == "cancelled"
        assert run.metrics is None
        assert task.status == "cancelled"
        assert "已在检查点中断" in task.error


def test_list_runs_isolation_and_pagination():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        for i in range(3):
            db.add(BacktestRun(
                user_id=USER_ID,
                strategy_id=1,
                codes=["sh.600000"],
                start=START,
                end=END,
                status="done",
                metrics={"total_return": i * 0.1},
                created_at=datetime.now(),
            ))
        db.add(BacktestRun(
            user_id=OTHER_ID,
            strategy_id=1,
            codes=["sh.600000"],
            start=START,
            end=END,
            status="done",
            metrics={"total_return": 9.0},
            created_at=datetime.now(),
        ))
        db.commit()

        page1 = list_runs(db, user_id=USER_ID, limit=2)
        assert len(page1["items"]) == 2
        assert page1["has_more"] is True
        # 倒序
        assert page1["items"][0]["run_id"] > page1["items"][1]["run_id"]

        page2 = list_runs(
            db, user_id=USER_ID, limit=2,
            before_run_id=page1["items"][1]["run_id"],
        )
        assert len(page2["items"]) == 1
        assert page2["has_more"] is False

        # 其他用户数据不可见
        other = list_runs(db, user_id=OTHER_ID, limit=10)
        assert len(other["items"]) == 1
        assert other["items"][0]["metrics"]["total_return"] == 9.0
