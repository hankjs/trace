"""复权因子采集:权威值 upsert 的幂等性与空响应保护。

依据 alembic 0007 的设计说明。核心约束:

1. **幂等**:因子是权威事实,重复采集不该产生重复行或改变行数;
2. **空响应不清空**:`fetch_adjust_factors` 返回空表示「该股无分红送转」,
   但数据源抖动也会返回空 —— 不能把它当成「因子被撤销」而删掉已有数据
   (同 sync_index_members 对空响应的处理);
3. **单只失败不污染整轮**:采集 5000 只时一只抛异常要 rollback 并继续,
   否则 Session 进入 PendingRollbackError,后续每只都失败(REVIEW §3.2)。
"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, func, select
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import ingest
from app.db import Base
from app.models import AdjustFactor, Stock


def _session() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


def _factors(rows: list[tuple[str, date, float, float | None]]) -> pd.DataFrame:
    return pd.DataFrame(
        rows, columns=["code", "divid_operate_date", "fore_factor", "back_factor"])


@pytest.fixture()
def db() -> Session:
    with _session() as session:
        session.add_all([Stock(code="sh.600519"), Stock(code="sz.000001")])
        session.commit()
        yield session


def test_upsert_is_idempotent(db):
    """同一批因子重复写入:行数不变,值不变。"""
    df = _factors([
        ("sh.600519", date(2015, 7, 17), 0.792993, 6.081667),
        ("sh.600519", date(2016, 7, 1), 0.810116, 6.212985),
    ])
    assert ingest.upsert_adjust_factors(db, "sh.600519", df) == 2
    ingest.upsert_adjust_factors(db, "sh.600519", df)

    assert db.execute(select(func.count()).select_from(AdjustFactor)).scalar() == 2
    row = db.get(AdjustFactor, ("sh.600519", date(2015, 7, 17)))
    assert row.fore_factor == pytest.approx(0.792993)


def test_upsert_applies_upstream_revision(db):
    """baostock 修订因子时,新值必须覆盖旧值(故用 upsert 而非 insert-ignore)。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2015, 7, 17), 0.792993, 6.081667),
    ]))
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2015, 7, 17), 0.799999, 6.099999),
    ]))

    assert db.execute(select(func.count()).select_from(AdjustFactor)).scalar() == 1
    row = db.get(AdjustFactor, ("sh.600519", date(2015, 7, 17)))
    assert row.fore_factor == pytest.approx(0.799999)


def test_six_decimal_precision_survives_roundtrip(db):
    """权威值是 6 位小数,DECIMAL(16,6) 不得截断。

    这是本表存在的部分理由:用 close/raw_close 两个 4 位小数相除只能得到
    约 4~5 位有效精度,权威值不该再被截断到那个水平。
    """
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.566931),
    ]))
    row = db.get(AdjustFactor, ("sh.600519", date(2020, 6, 24)))
    assert row.fore_factor == pytest.approx(0.856267, abs=1e-9)
    assert row.back_factor == pytest.approx(6.566931, abs=1e-9)


def test_empty_response_does_not_wipe_existing_factors(db, monkeypatch):
    """空响应视为「无分红送转」,不得清空已有因子。

    数据源抖动也会返回空,把它当成「因子被撤销」会静默毁掉历史。
    """
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2015, 7, 17), 0.792993, 6.081667),
    ]))

    monkeypatch.setattr(ingest.baostock_client, "login_session",
                        lambda: _nullcontext())
    monkeypatch.setattr(ingest.baostock_client, "fetch_adjust_factors",
                        lambda code, start, end: _factors([]))

    result = ingest.sync_adjust_factors(db, codes=["sh.600519"])

    assert result["empty"] == 1
    assert result["upserted"] == 0
    # 已有因子仍在
    assert db.execute(select(func.count()).select_from(AdjustFactor)).scalar() == 1


def test_one_failing_code_does_not_break_the_rest(db, monkeypatch):
    """单只失败要 rollback 并继续,否则 Session 中毒后每只都失败。"""
    def _fetch(code: str, start, end):
        if code == "sh.600519":
            raise RuntimeError("baostock 限速")
        return _factors([("sz.000001", date(2016, 6, 1), 0.5, 2.0)])

    monkeypatch.setattr(ingest.baostock_client, "login_session",
                        lambda: _nullcontext())
    monkeypatch.setattr(ingest.baostock_client, "fetch_adjust_factors", _fetch)

    result = ingest.sync_adjust_factors(db, codes=["sh.600519", "sz.000001"])

    assert result["failed"] == 1
    assert result["failed_codes"] == ["sh.600519"]
    # 失败的那只没拖垮后面的
    assert result["upserted"] == 1
    assert db.get(AdjustFactor, ("sz.000001", date(2016, 6, 1))) is not None


def test_sync_defaults_to_all_stocks(db, monkeypatch):
    """不传 codes 时覆盖 quant_stock 全表。"""
    seen: list[str] = []

    def _fetch(code: str, start, end):
        seen.append(code)
        return _factors([])

    monkeypatch.setattr(ingest.baostock_client, "login_session",
                        lambda: _nullcontext())
    monkeypatch.setattr(ingest.baostock_client, "fetch_adjust_factors", _fetch)

    ingest.sync_adjust_factors(db)

    assert sorted(seen) == ["sh.600519", "sz.000001"]


class _nullcontext:
    def __enter__(self):
        return None

    def __exit__(self, *exc):
        return False


def _bar(code: str, day: date, close: float, raw_close: float):
    from app.models import DailyBar
    return DailyBar(code=code, date=day, open=close, high=close, low=close,
                    close=close, raw_close=raw_close, volume=1000, amount=1000)


def test_audit_detects_preexisting_scale_error_that_self_check_cannot(db):
    """权威因子能发现**既存**错乱——这是自比对做不到的。

    detect_reanchor 是「库中反推因子 vs 新拉反推因子」的自比对,两边都来自
    close/raw_close。若某股入库时历史就已错乱,两边会一致地错下去,检测不出。
    权威因子是独立基准。
    """
    # 权威因子:2020-06-24 起 fore_factor = 0.856267
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.566931),
    ]))
    # 库中价格的隐含系数是 0.5,与权威值严重不符(既存错乱)
    db.add(_bar("sh.600519", date(2021, 3, 1), close=50.0, raw_close=100.0))
    db.commit()

    verdict = ingest.audit_scale_against_factors(db, "sh.600519")

    assert verdict.reanchored is True
    assert verdict.reason == "authoritative_mismatch"
    assert "权威因子" in (verdict.detail or "")


def test_audit_passes_when_stored_scale_matches_authority(db):
    """库中系数与权威因子一致时放行。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.566931),
    ]))
    db.add(_bar("sh.600519", date(2021, 3, 1),
                close=85.6267, raw_close=100.0))   # 系数正好 0.856267
    db.commit()

    verdict = ingest.audit_scale_against_factors(db, "sh.600519")

    assert verdict.reanchored is False
    assert verdict.reason == "authoritative_match"


def test_audit_degrades_when_factor_table_has_no_data(db):
    """因子表缺该股数据时返回 no_factors,调用方降级到自比对。

    不能因为缺基准就假定尺度正确。
    """
    db.add(_bar("sh.600519", date(2021, 3, 1), close=50.0, raw_close=100.0))
    db.commit()

    verdict = ingest.audit_scale_against_factors(db, "sh.600519")

    assert verdict.reanchored is False
    assert verdict.reason == "no_factors"


def test_audit_uses_factor_effective_on_the_bar_date(db):
    """取 divid_operate_date <= bar 日期的最后一个因子,不是最新因子。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.566931),
        ("sh.600519", date(2025, 6, 26), 0.960527, 7.366525),
    ]))
    # bar 在 2021 年:生效因子应是 0.856267 而非 0.960527
    db.add(_bar("sh.600519", date(2021, 3, 1),
                close=85.6267, raw_close=100.0))
    db.commit()

    verdict = ingest.audit_scale_against_factors(db, "sh.600519")

    assert verdict.reanchored is False
    assert "0.856267" in (verdict.detail or "")
