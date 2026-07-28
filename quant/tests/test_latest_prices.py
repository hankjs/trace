"""批量最新价:语义与逐票查询一致,避免 N+1。"""
from __future__ import annotations

import sys
from datetime import date, datetime
from pathlib import Path

from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data.latest_prices import latest_quotes, latest_reference_prices
from app.db import Base
from app.models import DailyBar, Snapshot


def test_latest_quotes_prefer_newer_snapshot_over_close():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        db.add_all([
            DailyBar(
                code="sh.a", date=date(2024, 6, 1),
                open=10, high=11, low=9, close=10.5, volume=1, amount=1,
            ),
            DailyBar(
                code="sh.b", date=date(2024, 6, 2),
                open=20, high=21, low=19, close=20.5, volume=1, amount=1,
            ),
            Snapshot(
                code="sh.a", ts=datetime(2024, 6, 2, 10, 0, 0),
                price=11.0, pct_chg=0.05,
            ),
            Snapshot(
                code="sh.b", ts=datetime(2024, 6, 1, 10, 0, 0),
                price=19.0, pct_chg=-0.01,
            ),
        ])
        db.commit()
        quotes = latest_quotes(db, ["sh.a", "sh.b", "sh.missing"])
        prices = latest_reference_prices(db, ["sh.a", "sh.b"])

    assert quotes["sh.a"]["source"] == "snapshot"
    assert quotes["sh.a"]["price"] == 11.0
    assert quotes["sh.a"]["pct_chg"] == 0.05
    assert quotes["sh.b"]["source"] == "close"
    assert quotes["sh.b"]["price"] == 20.5
    assert quotes["sh.missing"]["source"] is None
    assert prices["sh.a"] == (11.0, "snapshot")
    assert prices["sh.b"] == (20.5, "close")
