"""回测 API 输入校验。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from pydantic import ValidationError

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.backtest import BacktestIn, SensitivityIn, SweepIn
from app.backtest.evaluate import leaderboard


TODAY = date.today()


def test_backtest_in_rejects_invalid_windows():
    base = {"strategy_id": 1, "codes": ["sh.600519"]}

    with pytest.raises(ValidationError, match="start 必须早于 end"):
        BacktestIn(**base, start=date(2024, 6, 1), end=date(2024, 6, 1))

    with pytest.raises(ValidationError, match="未来日期"):
        BacktestIn(
            **base,
            start=TODAY + timedelta(days=1),
            end=TODAY + timedelta(days=10),
        )

    with pytest.raises(ValidationError, match="未来日期"):
        BacktestIn(
            **base,
            start=date(2020, 1, 1),
            end=TODAY + timedelta(days=1),
        )

    with pytest.raises(ValidationError, match="跨度不能超过"):
        BacktestIn(
            **base,
            start=date(2000, 1, 1),
            end=date(2020, 1, 2),
        )

    # 合法窗口应能构造
    assert BacktestIn(
        **base, start=date(2024, 1, 2), end=date(2024, 6, 28),
    )


def test_sweep_in_rejects_oversized_param_grid():
    base = {
        "strategy_id": 1,
        "codes": ["sh.600519"],
        "start": date(2024, 1, 2),
        "end": date(2024, 6, 28),
    }

    # 10 * 60 = 600 > 500
    with pytest.raises(ValidationError, match="组合数 .* 超过上限"):
        SweepIn(
            **base,
            param_grid={"a": list(range(10)), "b": list(range(60))},
        )

    # 恰好 500 应通过校验
    assert SweepIn(
        **base,
        param_grid={"a": list(range(10)), "b": list(range(50))},
    )


def test_sensitivity_in_rejects_invalid_windows():
    base = {"strategy_id": 1, "codes": ["sh.600519"]}

    with pytest.raises(ValidationError, match="未来日期"):
        SensitivityIn(
            **base,
            start=date(2030, 1, 1),
            end=date(2030, 6, 1),
        )

    with pytest.raises(ValidationError, match="跨度不能超过"):
        SensitivityIn(
            **base,
            start=date(2000, 1, 1),
            end=date(2020, 1, 2),
        )

    assert SensitivityIn(
        **base, start=date(2024, 1, 2), end=date(2024, 6, 28),
    )


def test_leaderboard_pagination_returns_count_and_offset_slice():
    from sqlalchemy import create_engine
    from sqlalchemy.orm import sessionmaker
    from app.db import Base

    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    SessionLocal = sessionmaker(bind=engine)
    with SessionLocal() as db:
        result = leaderboard(db, "user-a", limit=10, offset=0)
        assert result["count"] == 0
        assert result["items"] == []
