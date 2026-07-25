from datetime import date

from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.db import Base
from app.models import DailyBar
from scripts.backfill_pool import _done_codes


def _bar(row_id: int, code: str, day: date) -> DailyBar:
    return DailyBar(
        id=row_id, code=code, date=day, open=10, high=10, low=10,
        close=10, raw_close=10, volume=1, amount=10,
    )


def test_done_codes_requires_coverage_near_requested_start():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        db.add_all([
            _bar(1, "sh.complete", date(2019, 1, 2)),
            _bar(2, "sh.complete", date.today()),
            _bar(3, "sh.partial", date(2023, 1, 2)),
            _bar(4, "sh.partial", date.today()),
        ])
        db.commit()
        done = _done_codes(db, date(2019, 1, 1))

    assert "sh.complete" in done
    assert "sh.partial" not in done
