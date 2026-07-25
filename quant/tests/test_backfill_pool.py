from datetime import date

import pandas as pd
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session

from app.data import ingest
from app.db import Base
from app.models import DailyBar
from scripts.backfill_pool import _done_codes, _parse_force_rescale, backfill_checked


def _bar(row_id: int, code: str, day: date) -> DailyBar:
    # quant_daily_bar 换 (code, date) 自然主键后已无代理 id 列;
    # row_id 仍留在签名里,只为不改各调用处的可读编号
    return DailyBar(
        code=code, date=day, open=10, high=10, low=10,
        close=10, raw_close=10, volume=1, amount=10,
    )


def _db() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


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


def test_parse_force_rescale():
    assert _parse_force_rescale("") == (False, set())
    assert _parse_force_rescale("all") == (True, set())
    assert _parse_force_rescale("ALL") == (True, set())
    assert _parse_force_rescale("sh.600519, SZ.000001") == (
        False, {"sh.600519", "sz.000001"})


def _frame(rows: list[tuple[date, float, float]]) -> pd.DataFrame:
    return pd.DataFrame(
        [
            {"date": d, "open": c, "high": c, "low": c, "close": c,
             "raw_close": r, "volume": 1000.0, "amount": c * 1000}
            for d, c, r in rows
        ]
    )


def test_backfill_checked_detects_rescale_instead_of_blind_upsert(monkeypatch):
    """全市场回填必须过重锚检查:旧版直调 ingest.backfill 会静默混接尺度。"""
    code = "sh.600519"
    with _db() as db:
        ingest.upsert_bars(db, code, _frame([(date(2026, 6, 1), 100.0, 100.0),
                                             (date(2026, 7, 20), 102.0, 102.0)]))
        # 新尺度:系数从 1.0 变成 0.9
        new_scale = _frame([(date(2026, 6, 1), 90.0, 100.0),
                            (date(2026, 7, 20), 91.8, 102.0)])
        calls: list[tuple] = []

        def fake_fetch(c, s, e):
            calls.append((c, s, e))
            return new_scale

        monkeypatch.setattr(ingest.baostock_client, "fetch_daily_bars", fake_fetch)
        backfill_checked(db, code, date(2026, 6, 1), date(2026, 7, 20))

        # 判定拉取 + 重锚后的全量回填
        assert len(calls) == 2
        rows = db.execute(
            select(DailyBar).where(DailyBar.code == code)).scalars().all()
        factors = {round(r.close / r.raw_close, 6) for r in rows}
        assert factors == {0.9}, f"库中仍混有多种复权尺度: {factors}"


def test_backfill_checked_force_bypasses_check(monkeypatch):
    """--force-rescale 强制重拉,只拉一次,不做判定。"""
    code = "sh.600519"
    with _db() as db:
        ingest.upsert_bars(db, code, _frame([(date(2026, 6, 1), 100.0, 100.0)]))
        calls: list[tuple] = []

        def fake_fetch(c, s, e):
            calls.append((c, s, e))
            return _frame([(date(2026, 6, 1), 90.0, 100.0)])

        monkeypatch.setattr(ingest.baostock_client, "fetch_daily_bars", fake_fetch)
        backfill_checked(db, code, date(2026, 6, 1), date(2026, 7, 20), force=True)
        assert len(calls) == 1


def test_done_codes_can_be_overridden_by_force_codes():
    """尺度错乱的股票被标 done 后,--force-rescale 必须能把它捞回来重拉。"""
    with _db() as db:
        db.add_all([_bar(1, "sh.600519", date(2019, 1, 2)),
                    _bar(2, "sh.600519", date.today())])
        db.commit()
        done = _done_codes(db, date(2019, 1, 1))
        assert "sh.600519" in done
        # main() 里的语义:done - force_codes
        assert "sh.600519" not in (done - {"sh.600519"})
