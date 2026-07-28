"""数据质量汇总与回测 frames_data_quality。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data.quality import (
    data_quality_report,
    frames_data_quality,
    st_history_coverage,
)
from app.db import Base
from app.models import DailyBar, Stock


@pytest.fixture()
def db():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as session:
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


def test_st_history_coverage_counts_null_bars(db):
    report = st_history_coverage(db)
    assert report["total_bars"] == 3
    assert report["known_bars"] == 2
    assert report["null_bars"] == 1
    assert report["incomplete"] is True
    assert report["bar_coverage_ratio"] == pytest.approx(2 / 3, rel=1e-3)


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
