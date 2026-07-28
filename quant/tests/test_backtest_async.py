"""异步回测作业:pending → running → done,以及 claim 语义。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import numpy as np
import pandas as pd
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.jobs import claim_run, execute_backtest_run
from app.db import Base
from app.models import SYSTEM_OWNER_ID, BacktestRun, DailyBar, Strategy, Stock
from app.strategy.presets import get_preset_spec
from app.strategy.spec import strategy_spec_hash


START = date(2024, 1, 2)
END = date(2024, 6, 28)


def _session():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return sessionmaker(bind=engine)()


def _seed_bars(db: Session, code: str = "sh.600519") -> None:
    db.add(Stock(code=code, name="测试", list_date=date(2015, 1, 1), is_st=False))
    rows = []
    d = START - timedelta(days=250)
    price = 10.0
    i = 0
    while d <= END:
        price *= 1.001
        rows.append(DailyBar(
            code=code, date=d,
            open=price, high=price * 1.01, low=price * 0.99, close=price,
            raw_close=price, volume=1e6, amount=1e7, is_st=False,
        ))
        d += timedelta(days=1)
        i += 1
    db.add_all(rows)
    db.commit()


def test_claim_run_only_once():
    from datetime import datetime

    with _session() as db:
        # strategy FK: need a strategy row
        spec = get_preset_spec("breakout")
        db.add(Strategy(
            id=1, owner_id=SYSTEM_OWNER_ID, is_system=True,
            name="t", template="breakout", kind="single",
            params={}, spec=spec.model_dump(mode="json"),
            spec_hash=strategy_spec_hash(spec),
            research_status="unverified", enabled=True,
        ))
        run = BacktestRun(
            user_id="u1", strategy_id=1, start=START, end=END,
            status="pending", codes=["sh.600519"],
            created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        run_id = run.id

        claimed = claim_run(db, run_id)
        assert claimed is not None
        assert claimed.status == "running"
        assert claim_run(db, run_id) is None


def test_execute_backtest_run_completes(monkeypatch):
    from app import db as app_db
    from app.backtest import jobs as jobs_mod

    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    SessionLocal = sessionmaker(bind=engine)
    monkeypatch.setattr(app_db, "SessionLocal", SessionLocal)
    monkeypatch.setattr(jobs_mod, "SessionLocal", SessionLocal)

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
        )
        db.add(run)
        db.commit()
        run_id = run.id

    execute_backtest_run(run_id)

    with SessionLocal() as db:
        done = db.get(BacktestRun, run_id)
        assert done is not None
        assert done.status == "done"
        assert done.metrics is not None
        assert done.data_fingerprint
        assert done.finished_at is not None
