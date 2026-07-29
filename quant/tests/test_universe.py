"""历史指数成分区间查询。"""
from datetime import date

import pandas as pd
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.db import Base
from app.data.universe import (
    intervals_from_snapshots,
    membership_intervals,
    pool_during,
    pool_eligibility_matrix,
    rebuild_index_members_from_snapshots,
    resolve_pool_during,
    sample_dates,
)
from app.models import DailyBar, IndexMember, Stock


def test_sample_dates_includes_end():
    days = sample_dates(date(2015, 1, 1), date(2015, 2, 15), step_days=14)
    assert days[0] == date(2015, 1, 1)
    assert days[-1] == date(2015, 2, 15)
    assert date(2015, 1, 15) in days


def test_intervals_from_snapshots_merge_membership():
    snaps = [
        (date(2015, 1, 1), {"a": "A", "b": "B"}),
        (date(2015, 1, 15), {"a": "A", "c": "C"}),
        (date(2015, 1, 29), {"a": "A", "c": "C"}),
    ]
    by_code = {row["code"]: row for row in intervals_from_snapshots(snaps)}
    assert by_code["a"]["in_date"] == date(2015, 1, 1)
    assert by_code["a"]["out_date"] is None
    assert by_code["b"]["out_date"] == date(2015, 1, 15)
    assert by_code["c"]["in_date"] == date(2015, 1, 15)


def test_rebuild_index_members_from_snapshots_writes_table():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        result = rebuild_index_members_from_snapshots(db, "hs300", [
            (date(2015, 1, 1), {"sh.600000": "浦发", "sh.600001": "旧"}),
            (date(2015, 1, 15), {"sh.600000": "浦发", "sh.600519": "茅台"}),
        ])
        assert result["samples"] == 2
        assert result["intervals"] == 3
        rows = {
            (r.code, r.in_date, r.out_date)
            for r in db.query(IndexMember).all()
        }
        assert ("sh.600001", date(2015, 1, 1), date(2015, 1, 15)) in rows
        assert ("sh.600000", date(2015, 1, 1), None) in rows
        assert db.get(Stock, "sh.600519").name == "茅台"


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


def test_index_eligibility_respects_requested_index_and_membership_dates():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    dates = pd.date_range("2024-01-01", periods=4, freq="D")
    with Session(engine) as db:
        db.add_all([
            IndexMember(index_name="hs300", code="a", in_date=date(2024, 1, 1),
                        out_date=date(2024, 1, 3)),
            IndexMember(index_name="zz500", code="a", in_date=date(2024, 1, 3)),
        ])
        db.commit()

        eligibility = pool_eligibility_matrix(
            db, dates, ["a"], kind="index", index_name="hs300",
        )

    assert eligibility["a"].tolist() == [True, True, False, False]


def test_all_market_interval_union_keeps_delisted_stock_and_daily_st_history():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    dates = pd.date_range("2024-01-01", periods=4, freq="D")
    with Session(engine) as db:
        db.add_all([
            Stock(code="a", list_date=date(2020, 1, 1), is_st=False),
            Stock(code="b", list_date=date(2020, 1, 1),
                  delist_date=date(2024, 1, 3), is_st=False),
        ])
        for code in ("a", "b"):
            for day in dates:
                if code == "b" and day.date() >= date(2024, 1, 3):
                    continue
                db.add(DailyBar(
                    code=code, date=day.date(), open=10, high=10, low=10,
                    close=10, raw_close=10, volume=1, amount=10,
                    is_st=code == "a" and day.date() == date(2024, 1, 2),
                ))
        db.commit()
        codes = resolve_pool_during(
            db, date(2024, 1, 1), date(2024, 1, 4), kind="all",
            min_list_days=0,
        )
        frames = {
            code: pd.DataFrame([{
                "date": row.date, "is_st": row.is_st,
            } for row in db.query(DailyBar).filter(DailyBar.code == code).all()])
            for code in codes
        }
        eligibility = pool_eligibility_matrix(
            db, dates, codes, kind="all", min_list_days=0,
            daily_frames=frames,
        )

    assert codes == ["a", "b"]
    assert eligibility["a"].tolist() == [True, False, True, True]
    assert eligibility["b"].tolist() == [True, True, False, False]
