"""数据质量汇总与回测 frames_data_quality。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data.quality import (
    _alert_level,
    clear_quality_cache,
    data_quality_public_summary,
    data_quality_report,
    frames_data_quality,
    refresh_data_quality_cache,
    snapshot_coverage,
    st_history_coverage,
)
from app.db import Base
from app.models import DailyBar, DataQualityCache, FundamentalSnapshot, Stock


@pytest.fixture()
def db():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as session:
        clear_quality_cache(session)
        session.add_all([
            Stock(code="sh.a", name="A", list_date=date(2020, 1, 1), is_st=False),
            Stock(code="sh.b", name="B", list_date=date(2020, 1, 1), is_st=False),
        ])
        session.add_all([
            DailyBar(
                code="sh.a", date=date(2024, 6, 1),
                open=10, high=11, low=9, close=10.5, volume=1, amount=1,
                is_st=False,
            ),
            DailyBar(
                code="sh.a", date=date(2024, 6, 2),
                open=10, high=11, low=9, close=10.5, volume=1, amount=1,
                is_st=None,
            ),
            DailyBar(
                code="sh.b", date=date(2024, 6, 1),
                open=10, high=11, low=9, close=10.5, volume=1, amount=1,
                is_st=False,
            ),
        ])
        session.commit()
        yield session
        clear_quality_cache(session)


def test_st_history_coverage_counts_null_bars(db):
    report = st_history_coverage(db)
    assert report["total_bars"] == 3
    assert report["known_bars"] == 2
    assert report["null_bars"] == 1
    assert report["incomplete"] is True
    assert report["bar_coverage_ratio"] == pytest.approx(2 / 3, rel=1e-3)
    assert report["scope"] == "recent_window"
    assert report["window_start"] is not None
    assert report["window_end"] == "2024-06-02"


def test_st_history_coverage_custom_range(db):
    report = st_history_coverage(
        db, start=date(2024, 6, 2), end=date(2024, 6, 2),
    )
    assert report["scope"] == "custom"
    assert report["total_bars"] == 1
    assert report["known_bars"] == 0


def test_st_stock_coverage_requires_min_known_share(db):
    """股票级口径:非空 is_st bar 占比 >= 80% 才计入。

    fixture 中 sh.a 有 2 根 bar、1 根为 NULL(占比 0.5,不计入),
    sh.b 1 根 bar 全部非空(占比 1.0,计入)。
    """
    report = st_history_coverage(db)
    assert report["total_stocks_with_bars"] == 2
    assert report["stocks_with_st_history"] == 1
    assert report["stock_coverage_ratio"] == pytest.approx(0.5, rel=1e-3)
    assert report["stock_min_known_share"] == 0.8


def test_zero_denominator_ratios_are_none(db):
    """分母为 0(空窗口)时比率为 None,且不会把空库误报为 critical。"""
    report = st_history_coverage(db, start=date(2020, 1, 1), end=date(2020, 1, 2))
    assert report["total_bars"] == 0
    assert report["bar_coverage_ratio"] is None
    assert report["stock_coverage_ratio"] is None

    snaps = snapshot_coverage(db, as_of=date(2020, 1, 2))
    assert snaps["universe_stocks"] == 0
    assert snaps["valuation_ratio"] is None
    assert snaps["fundamental_ratio"] is None

    # None 与 0% 区分:None 不触发告警,真实 0% 仍是 critical
    assert _alert_level(None, None) == "ok"
    assert _alert_level(None, 1.0) == "ok"
    assert _alert_level(0.0, 1.0) == "critical"
    assert _alert_level(None, 0.0) == "warning"


def test_fundamental_coverage_uses_recent_period_window(db):
    """财务覆盖率只统计 as_of 前最近约 4 个报告期内的财报。

    sh.a 有近期财报(2024Q1,已披露)计入;sh.b 只有 2022 年报,
    旧口径下会被计入(比率恒接近 1),新口径下不计入。
    """
    db.add_all([
        FundamentalSnapshot(
            code="sh.a", data_date=date(2024, 3, 31),
            report_period=date(2024, 3, 31), available_date=date(2024, 4, 30),
            source="test", roe=0.08,
        ),
        FundamentalSnapshot(
            code="sh.b", data_date=date(2022, 12, 31),
            report_period=date(2022, 12, 31), available_date=date(2023, 2, 1),
            source="test", roe=0.05,
        ),
    ])
    db.commit()

    snaps = snapshot_coverage(db, as_of=date(2024, 6, 2))
    assert snaps["universe_stocks"] == 2
    assert snaps["fundamental_stocks"] == 1
    assert snaps["fundamental_ratio"] == pytest.approx(0.5, rel=1e-3)
    assert snaps["fundamental_period_start"] == "2023-06-03"  # 365 天前(2024 闰年)
    # 字段级覆盖率同样按近期窗口统计
    assert snaps["fields"]["roe"]["available"] == 1


def test_data_quality_report_summary_shape(db):
    report = data_quality_report(db, as_of=date(2024, 6, 2))
    summary = report["summary"]
    assert summary["alert_level"] in {"ok", "warning", "critical"}
    assert summary["stock_count"] == 2
    assert "st_bar_coverage_ratio" in summary
    assert "valuation_coverage_ratio" in summary
    assert "st_history" in report
    assert "snapshots" in report
    assert "adjust_factors" in report
    assert report["snapshots"]["fields"]  # admin 路径含字段明细
    assert report["cache"]["computed_at"]
    # 旁路表已落一行
    row = db.get(DataQualityCache, "latest")
    assert row is not None
    assert row.as_of == date(2024, 6, 2)


def test_data_quality_public_summary_skips_field_detail(db):
    summary = data_quality_public_summary(db, as_of=date(2024, 6, 2))
    assert summary["stock_count"] == 2
    assert "alert_level" in summary
    assert "fields" not in summary
    assert "computed_at" in summary


def test_data_quality_cache_hit_skips_rebuild(db):
    first = data_quality_report(db, as_of=date(2024, 6, 2), force=True)
    # 污染 payload,验证第二次不重算而是读缓存
    row = db.get(DataQualityCache, "latest")
    assert row is not None
    poisoned = dict(row.payload)
    poisoned["summary"] = {**poisoned["summary"], "stock_count": 999}
    row.payload = poisoned
    db.commit()

    second = data_quality_report(db, as_of=date(2024, 6, 2), force=False)
    assert second["summary"]["stock_count"] == 999

    refreshed = data_quality_report(db, as_of=date(2024, 6, 2), force=True)
    assert refreshed["summary"]["stock_count"] == first["summary"]["stock_count"]


def test_refresh_and_clear_quality_cache(db):
    refresh_data_quality_cache(db, as_of=date(2024, 6, 2))
    assert db.execute(select(DataQualityCache)).scalars().first() is not None
    clear_quality_cache(db)
    assert db.get(DataQualityCache, "latest") is None


def test_frames_data_quality_flags_incomplete_st():
    frames = {
        "sh.a": pd.DataFrame({
            "date": [date(2024, 1, 1), date(2024, 1, 2)],
            "close": [1.0, 1.1],
            "is_st": [False, None],
            "pe_ttm": [10.0, None],
        }),
        "sh.b": pd.DataFrame({
            "date": [date(2024, 1, 1)],
            "close": [2.0],
            "is_st": [False],
            "pe_ttm": [12.0],
        }),
    }
    quality = frames_data_quality(frames, required_fields=["pe_ttm"])
    assert quality["st_history_incomplete"] is True
    assert quality["st_incomplete_code_count"] == 1
    assert "sh.a" in quality["st_incomplete_codes"]
    assert quality["field_coverage"]["pe_ttm"]["available"] == 2
    assert quality["field_coverage"]["pe_ttm"]["total"] == 3
    assert quality["warnings"]


def test_frames_data_quality_complete():
    frames = {
        "sh.a": pd.DataFrame({
            "date": [date(2024, 1, 1)],
            "close": [1.0],
            "is_st": [False],
        }),
    }
    quality = frames_data_quality(frames)
    assert quality["st_history_incomplete"] is False
    assert quality["st_null_bar_ratio"] == 0.0
    assert quality["warnings"] == []
