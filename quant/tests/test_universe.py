"""历史指数成分区间查询。"""
from datetime import date

from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.db import Base
from app.data.universe import membership_intervals, pool_during
from app.models import IndexMember


def test_pool_during_uses_half_open_membership_intervals():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    start = date(2024, 1, 10)
    end = date(2024, 1, 20)

    with Session(engine) as db:
        db.add_all([
            IndexMember(
                id=1, index_name="hs300", code="sh.before",
                in_date=date(2023, 1, 1),
                out_date=start,
            ),
            IndexMember(
                id=2, index_name="hs300", code="sh.exiting",
                in_date=date(2023, 1, 1),
                out_date=date(2024, 1, 15),
            ),
            IndexMember(
                id=3, index_name="zz500", code="sh.entering", in_date=end,
                out_date=None,
            ),
            IndexMember(
                id=4, index_name="hs300", code="sh.after",
                in_date=date(2024, 1, 21),
                out_date=None,
            ),
            IndexMember(
                id=5, index_name="hs300", code="sh.duplicate",
                in_date=date(2023, 1, 1),
                out_date=None,
            ),
            IndexMember(
                id=6, index_name="zz500", code="sh.duplicate",
                in_date=date(2023, 1, 1),
                out_date=None,
            ),
        ])
        db.commit()

        assert pool_during(db, start, end) == [
            "sh.duplicate", "sh.entering", "sh.exiting",
        ]
        intervals = membership_intervals(
            db,
            ["sh.before", "sh.entering", "sh.exiting", "sh.after"],
            start,
            end,
        )

    assert {row.code for row in intervals} == {"sh.entering", "sh.exiting"}
