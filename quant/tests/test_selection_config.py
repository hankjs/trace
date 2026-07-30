"""选股配置影响选股与回测评分的断言。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.evaluate import top_scored_codes
from app.db import Base
from app.models import SYSTEM_OWNER_ID, DailyBar, FactorDaily, Pick, Pool, Stock
from app.selection.pipeline import run_selection
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


def _seed_bars(db: Session, code: str, start: date, end: date,
               *, trend: str = "up") -> None:
    import numpy as np
    rng = np.random.default_rng(42 if trend == "up" else 43)
    current = start
    close = 10.0
    while current <= end:
        if current.weekday() < 5:
            ret = rng.normal(0.005 if trend == "up" else -0.005, 0.02)
            close *= (1 + ret)
            db.add(DailyBar(
                code=code, date=current,
                open=close, high=close * 1.02, low=close * 0.98,
                close=close, raw_close=close,
                volume=1000000, amount=100000000, is_st=False,
            ))
        current += timedelta(days=1)
    db.flush()


def test_run_selection_honors_edited_weights_and_top_n():
    with _db() as db:
        seed_factor_defs(db)
        seed_stock(db, "sh.600001")
        seed_stock(db, "sh.600002")
        # 选股必须传入 codes 以避免依赖指数成分历史;使用周五确保日线存在
        day = date(2024, 6, 28)
        start = day - timedelta(days=250)
        # sh.600001 强趋势,sh.600002 弱趋势
        _seed_bars(db, "sh.600001", start, day, trend="up")
        _seed_bars(db, "sh.600002", start, day, trend="down")
        # 默认配置
        seed_selection_config(db, overrides={
            "score_weights": {"mom20": 0.5, "mom60": 0.5},
            "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            "hard_filters": [
                {"type": "exclude_st"},
                {"type": "exclude_suspended"},
                {"type": "min_bars", "value": 1},
            ],
            "top_n": 1,
        })
        result = run_selection(db, day=day, codes=["sh.600001", "sh.600002"])

    assert result["picked"] == 1
    pick = db.execute(select(Pick).where(Pick.date == day)).scalar_one()
    assert pick.code == "sh.600001"


def test_run_selection_honors_factor_gte_threshold():
    with _db() as db:
        seed_factor_defs(db)
        seed_stock(db, "sh.600001")
        day = date(2024, 6, 28)
        start = day - timedelta(days=250)
        _seed_bars(db, "sh.600001", start, day)
        # 设定极高的 amount_avg20 下限,使股票被过滤
        seed_selection_config(db, overrides={
            "score_weights": {"mom20": 1.0},
            "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            "hard_filters": [
                {"type": "exclude_st"},
                {"type": "exclude_suspended"},
                {"type": "min_bars", "value": 1},
                {"type": "factor_gte", "factor": "amount_avg20", "value": 1e12},
            ],
            "top_n": 10,
        })
        result = run_selection(db, day=day, codes=["sh.600001"])

    assert result["picked"] == 0
    assert result["filtered"].get("factor_gte_amount_avg20", 0) >= 1


def test_top_scored_codes_reflects_config():
    with _db() as db:
        seed_factor_defs(db)
        _seed_pool(db)
        seed_stock(db, "sh.600001")
        seed_stock(db, "sh.600002")
        day = date(2024, 6, 30)
        start = day - timedelta(days=250)
        _seed_bars(db, "sh.600001", start, day, trend="up")
        _seed_bars(db, "sh.600002", start, day, trend="down")
        seed_selection_config(db, overrides={
            "score_weights": {"mom20": 1.0},
            "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            "hard_filters": [
                {"type": "exclude_st"},
                {"type": "exclude_suspended"},
                {"type": "min_bars", "value": 1},
            ],
            "top_n": 10,
        })
        # 预写因子行
        db.add_all([
            FactorDaily(code="sh.600001", date=day,
                        values={"mom20": 0.2, "mom60": 0.3}),
            FactorDaily(code="sh.600002", date=day,
                        values={"mom20": -0.1, "mom60": -0.2}),
        ])
        db.commit()

        codes = top_scored_codes(db, n=1, as_of=day)

    assert codes == ["sh.600001"]


def test_run_selection_factor_codes_full_market():
    """factor_codes 超集:池外股票也落因子行,但 picks 只来自池内。"""
    with _db() as db:
        seed_factor_defs(db)
        seed_stock(db, "sh.600001")
        seed_stock(db, "sz.300002")
        day = date(2024, 6, 28)
        start = day - timedelta(days=250)
        _seed_bars(db, "sh.600001", start, day, trend="up")
        _seed_bars(db, "sz.300002", start, day, trend="down")
        seed_selection_config(db, overrides={
            "score_weights": {"mom20": 1.0},
            "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            "hard_filters": [{"type": "exclude_st"}],
            "top_n": 30,
        })
        result = run_selection(
            db, day=day,
            codes=["sh.600001"],                      # 选股池只有 1 只
            factor_codes=["sh.600001", "sz.300002"],  # 因子计算覆盖 2 只
        )

    # 池外股票也有因子行
    outside = db.execute(select(FactorDaily).where(
        FactorDaily.code == "sz.300002", FactorDaily.date == day,
    )).scalar_one_or_none()
    assert outside is not None and outside.values.get("mom20") is not None
    # picks 只来自池内
    picks = db.execute(select(Pick.code).where(Pick.date == day)).scalars().all()
    assert picks == ["sh.600001"]
    assert result["factor_scope"] == 2
    assert result["pool"] == 1
