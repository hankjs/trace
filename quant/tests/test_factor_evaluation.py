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
from app.models import (
    DailyBar,
    FactorDaily,
    FactorEvaluation,
    Stock,
    ValuationSnapshot,
)

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


def _seed_industry_and_caps(db: Session, codes: list[str], n_days: int = 40):
    """给股票分两个行业,并按 code 序写入递增总市值快照。"""
    start = date(2024, 1, 2)
    for i, code in enumerate(codes):
        stock = db.get(Stock, code)
        stock.industry = "银行" if i % 2 == 0 else "医药"
        for d in range(n_days):
            day = start + timedelta(days=d)
            db.add(ValuationSnapshot(
                code=code,
                data_date=day,
                available_date=day,
                source="test",
                total_market_cap=1e9 * (i + 1),
            ))
    db.commit()


# --- 新增:中性化 / IC 衰减 / 显著性 / 多重检验 ---------------------------


def test_neutralize_records_modes_and_changes_ic():
    """中性化后 IC 与裸 IC 不同,且口径落库可复现。"""
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=10, n_days=40)
        _seed_industry_and_caps(db, codes)
        kwargs = dict(
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 2, 20),
            codes=codes,
            layers=2,
            rebalance="weekly",
        )
        raw = evaluate_factor_efficacy(db, **kwargs)
        # 第二次评估会 commit 并让上一行过期,先取出需要的值
        raw_neutralize = raw.neutralize
        raw_ic_mean = raw.result["ic"]["ic_mean"]
        raw_modes = raw.result["neutralization"]["modes"]

        neut = evaluate_factor_efficacy(
            db, neutralize=["industry", "market_cap"], **kwargs,
        )

        assert raw_neutralize is None
        assert raw_modes == []

        assert neut.status == "done"
        assert neut.neutralize == ["industry", "market_cap"]
        assert neut.result["neutralization"]["modes"] == ["industry", "market_cap"]
        assert neut.result["neutralization"]["applied_periods"] > 0
        # 因子(close)与市值同序,市值中性化必然改变 IC
        assert neut.result["ic"]["ic_mean"] != raw_ic_mean


def test_neutralize_rejects_unknown_mode():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        with pytest.raises(ValueError, match="neutralize"):
            evaluate_factor_efficacy(
                db,
                user_id=USER_ID,
                expression={"op": "field", "name": "close"},
                start=date(2024, 1, 2),
                end=date(2024, 2, 20),
                codes=codes,
                layers=2,
                rebalance="weekly",
                neutralize=["sector"],
            )


def test_ic_decay_covers_requested_horizons():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=10, n_days=60)
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 2, 29),
            codes=codes,
            layers=2,
            rebalance="weekly",
            horizons=[1, 5, 10],
        )
    decay = row.result["ic_decay"]
    assert [item["horizon_days"] for item in decay] == [1, 5, 10]
    assert row.horizons == [1, 5, 10]
    # 短 horizon 的样本数不应少于长 horizon(末尾窗口不足会丢期)
    n_by_h = {item["horizon_days"]: item["n_periods"] for item in decay}
    assert n_by_h[1] >= n_by_h[10]
    assert n_by_h[1] > 0


def test_horizons_out_of_range_rejected():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=4, n_days=40)
        for bad in ([0], [61], [1, 2, 3, 4, 5, 6, 7]):
            with pytest.raises(ValueError):
                evaluate_factor_efficacy(
                    db,
                    user_id=USER_ID,
                    expression={"op": "field", "name": "close"},
                    start=date(2024, 1, 2),
                    end=date(2024, 2, 20),
                    codes=codes,
                    layers=2,
                    rebalance="weekly",
                    horizons=bad,
                )


def test_horizons_empty_list_disables_decay():
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
            horizons=[],
        )
    assert row.result["ic_decay"] == []
    assert row.horizons is None


def test_ic_significance_and_multiplicity_present():
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=10, n_days=90)
        row = evaluate_factor_efficacy(
            db,
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 3, 31),
            codes=codes,
            layers=2,
            rebalance="weekly",
            horizons=[1, 5],
        )
    ic = row.result["ic"]
    assert ic["t_stat_method"] == "newey_west_normal_approx"
    # 样本期够长(>=6 期)时必须给出 t 值与 p 值
    assert ic["n_periods"] >= 6
    assert ic["ic_t_stat"] is not None
    assert ic["ic_p_value"] is not None
    assert 0.0 <= ic["ic_p_value"] <= 1.0
    assert ic["icir_annual"] is not None

    mult = row.result["multiplicity"]
    # 首次评估:先前次数为 0,试验次数 = horizon 数 × 1
    assert mult["n_prior_evaluations"] == 0
    assert mult["n_tests_estimated"] == 2
    assert mult["bonferroni_alpha"] == pytest.approx(0.025)
    assert isinstance(mult["survives_bonferroni"], bool)


def test_multiplicity_counts_prior_evaluations():
    """第二次评估应把上一次计入试验次数,阈值随之收紧。"""
    with _db() as db:
        codes = _seed_stocks_and_bars(db, n_codes=10, n_days=90)
        kwargs = dict(
            user_id=USER_ID,
            expression={"op": "field", "name": "close"},
            start=date(2024, 1, 2),
            end=date(2024, 3, 31),
            codes=codes,
            layers=2,
            rebalance="weekly",
            horizons=[1],
        )
        first = evaluate_factor_efficacy(db, **kwargs)
        first_mult = dict(first.result["multiplicity"])

        second = evaluate_factor_efficacy(db, **kwargs)
        second_mult = dict(second.result["multiplicity"])

    assert first_mult["n_prior_evaluations"] == 0
    assert second_mult["n_prior_evaluations"] == 1
    assert second_mult["bonferroni_alpha"] < first_mult["bonferroni_alpha"]


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
