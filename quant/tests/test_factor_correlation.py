"""因子相关性与正交性:构造数据断言已知答案。"""
from __future__ import annotations

from datetime import date, timedelta

import numpy as np
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.db import Base
from app.factors.correlation import (
    MAX_BENCHMARKS,
    _orthogonalize,
    compute_factor_correlation,
)
from app.models import DailyBar, FactorDaily, FactorDef, Stock, SYSTEM_OWNER_ID

USER_ID = "11111111-1111-1111-1111-111111111111"
USER_B = "22222222-2222-2222-2222-222222222222"


def _db() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_universe(db: Session, n_codes: int = 12, n_days: int = 80):
    """多标的日线 + 已落库因子。

    - bench_lin = close
    - target_lin = 2*bench + 1(完全线性相关 → 残差 0)
    - ortho_sig = 与收益相关、与 close 无关的独立信号(按日刷新随机数 + 收益项)
    """
    codes = [f"sh.{600000 + i:06d}" for i in range(n_codes)]
    start = date(2024, 1, 2)
    rng = np.random.default_rng(0)
    # 先写日线,再写因子(正交信号需要下期收益的构造)
    closes: dict[tuple[str, date], float] = {}
    for i, code in enumerate(codes):
        db.add(Stock(
            code=code, name=f"股{i}", list_date=date(2015, 1, 1), is_st=False,
            industry="制造" if i % 2 == 0 else "金融",
        ))
        base = 10.0 + i * 0.3
        for d in range(n_days):
            day = start + timedelta(days=d)
            close = base * ((1 + 0.001 * (i + 1)) ** d)
            closes[(code, day)] = close
            db.add(DailyBar(
                code=code, date=day,
                open=close * 0.99, high=close * 1.01, low=close * 0.99,
                close=close, raw_close=close,
                volume=1e6, amount=1e7, is_st=False,
            ))
    for i, code in enumerate(codes):
        for d in range(n_days):
            day = start + timedelta(days=d)
            close = closes[(code, day)]
            bench = float(close)
            target = 2.0 * bench + 1.0
            # 正交:纯噪声 + 与「下一日收益」同向的项;与 close 水平无结构相关
            nxt = start + timedelta(days=d + 1)
            if (code, nxt) in closes:
                fwd = closes[(code, nxt)] / close - 1.0
            else:
                fwd = 0.0
            ortho = float(rng.normal(0, 1.0)) + 5.0 * fwd
            db.add(FactorDaily(
                code=code, date=day,
                values={
                    "bench_lin": bench,
                    "target_lin": target,
                    "ortho_sig": ortho,
                },
            ))
    for key, name in [
        ("bench_lin", "线性对照"),
        ("target_lin", "线性待检"),
        ("ortho_sig", "正交信号"),
    ]:
        db.add(FactorDef(
            key=key, name=name, expression={"op": "field", "name": "close"},
            expression_hash=f"hash_{key}", min_bars=1, enabled=True,
            is_system=True, owner_id=SYSTEM_OWNER_ID,
        ))
    db.commit()
    return codes, start


def test_identical_factor_has_no_increment():
    with _db() as db:
        codes, start = _seed_universe(db)
        end = start + timedelta(days=70)
        row = compute_factor_correlation(
            db,
            user_id=USER_ID,
            factor_key="target_lin",
            benchmark_keys=["bench_lin"],
            start=start,
            end=end,
            codes=codes,
            rebalance="weekly",
        )
    assert row.status == "done"
    result = row.result or {}
    residual = result["residual"]
    # 完全线性相关:残差应接近 0 或被 skip,verdict 不得 has_increment
    assert result["verdict"] == "no_increment"
    if residual["n_periods"] > 0:
        assert abs(residual["ic_mean"] or 0) < 0.15
    pairs = result["pairs"]
    assert pairs[0]["factor_key"] == "bench_lin"
    assert pairs[0]["pearson_mean"] is not None
    assert abs(pairs[0]["pearson_mean"]) > 0.9


def test_orthogonal_factor_keeps_increment():
    with _db() as db:
        codes, start = _seed_universe(db)
        end = start + timedelta(days=70)
        row = compute_factor_correlation(
            db,
            user_id=USER_ID,
            factor_key="ortho_sig",
            benchmark_keys=["bench_lin"],
            start=start,
            end=end,
            codes=codes,
            rebalance="weekly",
        )
    result = row.result or {}
    pairs = result["pairs"]
    assert pairs[0]["pearson_mean"] is not None
    # 正交构造:因子值相关应接近 0
    assert abs(pairs[0]["pearson_mean"]) < 0.4
    # 残差 IC 与裸 IC 方向一致且量级接近(允许噪声)
    raw_ic = result["raw"]["ic_mean"]
    res_ic = result["residual"]["ic_mean"]
    if raw_ic is not None and res_ic is not None:
        assert abs(res_ic - raw_ic) < abs(raw_ic) + 0.2


def test_high_correlation_pair_is_flagged():
    with _db() as db:
        codes, start = _seed_universe(db)
        end = start + timedelta(days=70)
        # 再造 5 日/10 日动量式:target 与 bench 高度相关
        row = compute_factor_correlation(
            db,
            user_id=USER_ID,
            factor_key="target_lin",
            benchmark_keys=["bench_lin"],
            start=start,
            end=end,
            codes=codes,
            rebalance="weekly",
        )
    ratio = row.result["pairs"][0]["high_corr_ratio"]
    assert ratio is not None
    assert ratio > 0.5


def test_orthogonalize_returns_none_on_singular_design():
    y = np.array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
    b1 = np.array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
    b2 = 2.0 * b1  # 完全共线
    assert _orthogonalize(y, [b1, b2]) is None
    # 正常可解
    b3 = np.array([6.0, 5.0, 4.0, 3.0, 2.0, 1.0])
    residual = _orthogonalize(y, [b3])
    assert residual is not None
    assert len(residual) == 6


def test_benchmark_limit_enforced():
    with _db() as db:
        codes, start = _seed_universe(db)
        keys = [f"k{i}" for i in range(MAX_BENCHMARKS + 1)]
        for k in keys:
            db.add(FactorDef(
                key=k, name=k, expression={"op": "field", "name": "close"},
                expression_hash=k, min_bars=1, enabled=True,
                is_system=False, owner_id=USER_ID,
            ))
        db.commit()
        with pytest.raises(ValueError, match="最多"):
            compute_factor_correlation(
                db,
                user_id=USER_ID,
                factor_key="target_lin",
                benchmark_keys=keys,
                start=start,
                end=start + timedelta(days=40),
                codes=codes,
            )


def test_missing_benchmark_key_errors():
    with _db() as db:
        codes, start = _seed_universe(db)
        with pytest.raises(ValueError, match="不存在"):
            compute_factor_correlation(
                db,
                user_id=USER_ID,
                factor_key="target_lin",
                benchmark_keys=["no_such_bench"],
                start=start,
                end=start + timedelta(days=40),
                codes=codes,
            )


def test_self_excluded_from_benchmarks():
    with _db() as db:
        codes, start = _seed_universe(db)
        row = compute_factor_correlation(
            db,
            user_id=USER_ID,
            factor_key="bench_lin",
            benchmark_keys=["bench_lin", "ortho_sig"],
            start=start,
            end=start + timedelta(days=70),
            codes=codes,
            rebalance="weekly",
        )
    assert "bench_lin" not in row.benchmark_keys
    assert "ortho_sig" in row.benchmark_keys
    assert row.result.get("note")
    assert "剔除" in row.result["note"]


def test_verdict_inconclusive_on_short_sample():
    with _db() as db:
        codes, start = _seed_universe(db, n_codes=12, n_days=20)
        # 极短区间:调仓日很少
        row = compute_factor_correlation(
            db,
            user_id=USER_ID,
            factor_key="target_lin",
            benchmark_keys=["bench_lin"],
            start=start,
            end=start + timedelta(days=12),
            codes=codes,
            rebalance="weekly",
        )
    assert row.result["verdict"] == "inconclusive"


# ---- A2A 层用例(复用 test_a2a 夹具) ----

from tests.test_a2a import (  # noqa: E402
    CLIENT_CLAIMS,
    _fail_text,
    _send,
    _state,
    _token,
    client,
)


def test_correlation_requires_confirmed(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.correlation",
        {
            "factor_key": "x",
            "benchmark_keys": ["y"],
            "start": "2024-01-01",
            "end": "2024-06-01",
        },
    )
    assert _state(result) == "failed"
    assert "高成本" in _fail_text(result)


def test_correlation_get_rejects_other_users_row(client):
    from app import db as app_db
    from app.models import FactorCorrelation
    from datetime import datetime

    with app_db.SessionLocal() as db:
        row = FactorCorrelation(
            user_id=USER_ID,
            factor_key="x",
            benchmark_keys=["y"],
            start=date(2024, 1, 1),
            end=date(2024, 6, 1),
            rebalance="weekly",
            universe={"size": 0, "filters": []},
            status="done",
            result={"verdict": "no_increment"},
            created_at=datetime.now(),
        )
        db.add(row)
        db.commit()
        cid = row.id

    other = {
        "sub": USER_B,
        "username": "b",
        "can_admin": False,
        "can_client": True,
    }
    result = _send(
        client,
        _token(other),
        "factor.correlation_get",
        {"correlation_id": cid},
    )
    assert _state(result) == "failed"
    assert "不存在" in _fail_text(result)
