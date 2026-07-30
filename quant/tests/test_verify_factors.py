"""缺因子单票验证(白名单哨兵)的行为断言。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import ingest
from app.db import Base
from app.models import AdjustFactor, Stock


def _db():
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_stock(db: Session, code: str, list_date: date | None = None) -> None:
    db.add(Stock(code=code, name="测试", industry="制造", list_date=list_date))
    db.flush()


def _real_factors(code: str) -> pd.DataFrame:
    return pd.DataFrame({
        "code": [code, code],
        "divid_operate_date": [date(2020, 6, 1), date(2023, 6, 1)],
        "fore_factor": [1.2, 1.5],
        "back_factor": [1.1, 1.4],
    })


def test_verify_empty_writes_sentinel(monkeypatch):
    """baostock 返回 0 行 → 写 1.0 哨兵,生效因子可被采纳。"""
    with _db() as db:
        _seed_stock(db, "sh.600107", list_date=date(2000, 3, 16))
        monkeypatch.setattr(
            ingest.baostock_client, "fetch_adjust_factors",
            lambda code: pd.DataFrame(
                columns=["code", "divid_operate_date", "fore_factor", "back_factor"]))
        result = ingest.verify_missing_factor_codes(
            db, ["sh.600107"], sleep_per_code=0)
        db.commit()

        assert result == {"verified_none": ["sh.600107"], "synced": [],
                          "failed": [], "remaining": []}
        row = db.execute(select(AdjustFactor)).scalar_one()
        assert row.code == "sh.600107"
        assert row.divid_operate_date == date(2000, 3, 16)
        assert row.fore_factor == 1.0 and row.source == "verified_none"
        # 哨兵被 _effective_fore_factors 自然采纳
        eff = ingest._effective_fore_factors(db, ["sh.600107"], date(2026, 7, 30))
        assert eff["sh.600107"] == 1.0


def test_verify_non_empty_syncs_real_factors(monkeypatch):
    """baostock 返回有行 → upsert 真实因子而非哨兵。"""
    with _db() as db:
        _seed_stock(db, "sh.600519")
        monkeypatch.setattr(
            ingest.baostock_client, "fetch_adjust_factors", _real_factors)
        result = ingest.verify_missing_factor_codes(
            db, ["sh.600519"], sleep_per_code=0)
        db.commit()

        assert result["synced"] == ["sh.600519"]
        assert result["verified_none"] == []
        rows = db.execute(select(AdjustFactor).order_by(
            AdjustFactor.divid_operate_date)).scalars().all()
        assert len(rows) == 2 and all(r.source == "baostock" for r in rows)
        # 真实因子按日期生效
        eff = ingest._effective_fore_factors(db, ["sh.600519"], date(2023, 7, 1))
        assert eff["sh.600519"] == 1.5


def test_verify_idempotent_and_bounded(monkeypatch):
    """已有因子行的 code 不重复验证;超 max_codes 的进 remaining。"""
    with _db() as db:
        _seed_stock(db, "sh.600107")
        db.add(AdjustFactor(
            code="sh.600001", divid_operate_date=date(2000, 1, 1),
            fore_factor=1.0, back_factor=1.0, source="verified_none"))
        db.flush()
        calls: list[str] = []
        monkeypatch.setattr(
            ingest.baostock_client, "fetch_adjust_factors",
            lambda code: calls.append(code) or pd.DataFrame(
                columns=["code", "divid_operate_date", "fore_factor", "back_factor"]))
        result = ingest.verify_missing_factor_codes(
            db, ["sh.600001", "sh.600107", "sh.600002"],
            sleep_per_code=0, max_codes=1)

        assert calls == ["sh.600107"]          # 600001 已有行,跳过
        assert result["verified_none"] == ["sh.600107"]
        assert result["remaining"] == ["sh.600002"]


def test_sentinel_then_first_dividend_real_factor_wins():
    """哨兵之后首次除权:更晚日期的真实因子接管。"""
    with _db() as db:
        db.add(AdjustFactor(
            code="sh.600107", divid_operate_date=date(2000, 3, 16),
            fore_factor=1.0, back_factor=1.0, source="verified_none"))
        db.add(AdjustFactor(
            code="sh.600107", divid_operate_date=date(2026, 6, 1),
            fore_factor=0.8, back_factor=0.9, source="baostock"))
        db.flush()
        before = ingest._effective_fore_factors(db, ["sh.600107"], date(2026, 5, 31))
        after = ingest._effective_fore_factors(db, ["sh.600107"], date(2026, 7, 30))
        assert before["sh.600107"] == 1.0
        assert after["sh.600107"] == 0.8
