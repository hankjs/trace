"""因子有效性评估引擎测试。"""
from __future__ import annotations

import threading
from datetime import date, timedelta

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session, sessionmaker

from app.backtest.validation import MAX_BACKTEST_YEARS
from app.db import Base
from app.factors.evaluation import (
    EvaluationCancelledError,
    evaluate_factor_efficacy,
)
from app.models import DailyBar, FactorDaily, FactorEvaluation, Stock

USER_ID = "11111111-1111-1111-1111-111111111111"


def _db() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_stocks_and_bars(db: Session, n_codes: int = 4, n_days: int = 40):
    """生成 n_codes 只股票,n_days 个连续交易日,价格随 code 指数漂移。"""
    codes = [f"sh.{600000 + i:06d}" for i in range(n_codes)]
    start = date(2024, 1, 2)
    for i, code in enumerate(codes):
        db.add(Stock(
            code=code,
            name=f"股票{i}",
            list_date=date(2015, 1, 1),
            is_st=False,
        ))
        base = 10.0 + i * 0.5
        daily_ret = 0.001 * (i + 1)
        for d in range(n_days):
            day = start + timedelta(days=d)
            # 高 code 的日收益更高,使 close(因子)与下期收益正相关
            close = base * ((1 + daily_ret) ** d)
            db.add(DailyBar(
                code=code,
                date=day,
                open=close * 0.99,
                high=close * 1.01,
                low=close * 0.99,
                close=close,
                raw_close=close,
                volume=1e6,
                amount=1e7,
                is_st=False,
            ))
    db.commit()
    return codes


def test_evaluate_by_expression_returns_ic_and_layers():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 2, 20),
            codes=codes,
            layers=2,
            rebalance="weekly",
        )
    assert isinstance(row, FactorEvaluation)
    assert row.status == "done"
    assert row.factor_key is None
    assert row.expression == {"op": "field", "name": "close"}
    result = row.result or {}
    ic = result["ic"]
    assert ic["n_periods"] >= 2
    assert -1 <= ic["ic_mean"] <= 1
    assert -1 <= ic["rank_ic_mean"] <= 1
    assert 0 <= ic["positive_ratio"] <= 1
    assert len(result["layers"]) == 2
    # 高 close 股票收益更高,顶层应跑赢底层
    assert result["layers"][0]["total_return"] < result["layers"][1]["total_return"]
    assert result["coverage"]["factor_value_ratio"] == 1.0


def test_evaluate_by_factor_key_reads_factor_daily():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        # 写入已计算的因子值:直接用 close 作因子
        for code in codes:
            bars = db.execute(
                select(DailyBar).where(DailyBar.code == code)
            ).scalars().all()
            for bar in bars:
                db.add(FactorDaily(
                    code=code,
                    date=bar.date,
                    values={"close_proxy": float(bar.close)},
                ))
        db.commit()
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            factor_key="close_proxy",
            start=date(2024, 1, 2),
            end=date(2024, 2, 20),
            codes=codes,
            layers=2,
            rebalance="weekly",
        )
    assert row.status == "done"
    assert row.factor_key == "close_proxy"
    assert row.result["ic"]["n_periods"] >= 2


def test_layers_clamped_to_max_10():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=25, n_days=40)
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 2, 20),
            codes=codes,
            layers=20,
            rebalance="weekly",
        )
    assert row.layers == 10
    assert len(row.result["layers"]) == 10


def test_universe_filters_default_all_domain():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 2, 20),
            layers=2,
            rebalance="weekly",
        )
    assert row.universe["size"] >= 4
    assert set(row.universe["filters"]) >= {"st", "suspended", "lt_60d"}


def test_cancel_event_marks_row_cancelled():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        event = threading.Event()
        event.set()
        with pytest.raises(EvaluationCancelledError):
            evaluate_factor_efficacy(
                db,
                user_id=USER_ID,
                expression={"op": "field", "name": "close"},
                start=date(2024, 1, 2),
                end=date(2024, 2, 20),
                codes=codes,
                layers=2,
                rebalance="weekly",
                cancel_event=event,
            )
        row = db.execute(
            select(FactorEvaluation).where(FactorEvaluation.user_id == USER_ID)
        ).scalars().first()
        assert row is not None
        assert row.status == "cancelled"
        assert row.result is None


def test_invalid_expression_raises_before_row():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        with pytest.raises(ValueError):
            evaluate_factor_efficacy(
                db,
                user_id=USER_ID,
                expression={"op": "unknown_op"},
                start=date(2024, 1, 2),
                end=date(2024, 2, 20),
                codes=codes,
                layers=2,
                rebalance="weekly",
            )
        # 表达式校验在落库前失败,不生成半量行
        row = db.execute(
            select(FactorEvaluation).where(FactorEvaluation.user_id == USER_ID)
        ).scalar_one_or_none()
        assert row is None


def test_window_longer_than_10_years_rejected():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        with pytest.raises(ValueError):
            evaluate_factor_efficacy(
                db,
                user_id=USER_ID,
                expression={"op": "field", "name": "close"},
                start=date(2010, 1, 1),
                end=date(2025, 1, 1),
                codes=codes,
                layers=2,
                rebalance="weekly",
            )
