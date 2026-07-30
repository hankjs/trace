"""因子回填任务行为断言。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.db import Base
from app.factors.backfill import run_factor_backfill_task
from app.models import SYSTEM_OWNER_ID, DailyBar, FactorDaily, Pool, Task
from app.tasks import HANDLERS
from tests.factories import seed_factor_defs, seed_selection_config, seed_stock


def _db():
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_pool(db: Session) -> None:
    db.add(Pool(
        id=2, kind="all", ref=None, owner_id=SYSTEM_OWNER_ID, is_system=True,
        name="全部A股", min_list_days=0,
    ))
    db.flush()


def _seed_bars(db: Session, code: str, start: date, end: date) -> None:
    rng = __import__("numpy").random.default_rng(42)
    current = start
    close = 10.0
    while current <= end:
        if current.weekday() < 5:
            ret = rng.normal(0.0005, 0.02)
            close *= (1 + ret)
            db.add(DailyBar(
                code=code, date=current,
                open=close, high=close * 1.02, low=close * 0.98,
                close=close, raw_close=close,
                volume=1000, amount=10000, is_st=False,
            ))
        current += timedelta(days=1)
    db.flush()


def test_backfill_merge_never_clobbers_other_factor_keys():
    with _db() as db:
        seed_factor_defs(db)
        _seed_pool(db)
        seed_stock(db, "sh.600001")
        start = date(2024, 1, 1)
        end = date(2024, 1, 31)
        _seed_bars(db, "sh.600001", start, end)
        # 预插一行,只含 amount_avg20
        db.add(FactorDaily(
            code="sh.600001", date=date(2024, 1, 31),
            values={"amount_avg20": 9999.0},
        ))
        db.commit()

        task = Task(
            user_id="test", type="factor_backfill", status="pending",
            title="回填", params={
                "factor_key": "mom20",
                "start": str(start),
                "end": str(end),
                "codes": ["sh.600001"],
            },
        )
        result = run_factor_backfill_task(db, task)

    assert result["rows_written"] >= 1
    row = db.execute(
        select(FactorDaily).where(
            FactorDaily.code == "sh.600001",
            FactorDaily.date == date(2024, 1, 31),
        )
    ).scalar_one()
    assert "mom20" in row.values
    assert row.values.get("amount_avg20") == 9999.0


def test_backfill_omits_nan_keys_and_skips_unchanged_rows():
    with _db() as db:
        seed_factor_defs(db)
        _seed_pool(db)
        seed_stock(db, "sh.600001")
        start = date(2024, 1, 1)
        # 需要 >= mom20 的 21 根 K 线,扩展到 2 月末以产生有效行
        end = date(2024, 2, 29)
        _seed_bars(db, "sh.600001", start, end)
        db.commit()

        task = Task(
            user_id="test", type="factor_backfill", status="pending",
            title="回填", params={
                "factor_key": "mom20",
                "start": str(start),
                "end": str(end),
                "codes": ["sh.600001"],
            },
        )
        result1 = run_factor_backfill_task(db, task)
        # 再次回填应该跳过未变化的行
        task2 = Task(
            user_id="test", type="factor_backfill", status="pending",
            title="回填", params={
                "factor_key": "mom20",
                "start": str(start),
                "end": str(end),
                "codes": ["sh.600001"],
            },
        )
        result2 = run_factor_backfill_task(db, task2)

    assert result1["rows_written"] >= 1
    assert result2["rows_written"] == 0
    assert result2["skipped"] >= 1


def test_backfill_min_bars_gates_long_window_factors():
    with _db() as db:
        seed_factor_defs(db)
        _seed_pool(db)
        seed_stock(db, "sh.600001")
        # 只给 10 个交易日,不足 mom60(61 bars)
        start = date(2024, 1, 1)
        end = date(2024, 1, 15)
        _seed_bars(db, "sh.600001", start, end)
        db.commit()

        task = Task(
            user_id="test", type="factor_backfill", status="pending",
            title="回填", params={
                "factor_key": "mom60",
                "start": str(start),
                "end": str(end),
                "codes": ["sh.600001"],
            },
        )
        result = run_factor_backfill_task(db, task)

    assert result["rows_written"] == 0


def test_factor_backfill_handler_registered():
    assert "factor_backfill" in HANDLERS
