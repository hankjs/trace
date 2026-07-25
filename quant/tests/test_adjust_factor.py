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


def test_derive_factors_keeps_only_change_points(db):
    """自算因子按变化点稀疏化,与 baostock 按除权日返回的形态一致。"""
    bars = pd.DataFrame([
        # 因子恒为 0.9:只该产出 1 行(首行)
        {"date": date(2024, 1, 2), "close": 9.0, "raw_close": 10.0},
        {"date": date(2024, 1, 3), "close": 18.0, "raw_close": 20.0},
        # 因子跳到 0.95:除权,再产出 1 行
        {"date": date(2024, 6, 3), "close": 19.0, "raw_close": 20.0},
        {"date": date(2024, 6, 4), "close": 28.5, "raw_close": 30.0},
    ])
    out = ingest.derive_adjust_factors(bars)

    assert list(out["divid_operate_date"]) == [date(2024, 1, 2), date(2024, 6, 3)]
    assert out["fore_factor"].tolist() == pytest.approx([0.9, 0.95])


def test_derive_factors_ignores_decimal_rounding_noise(db):
    """DECIMAL(12,4) 的舍入噪声不得被当成除权。

    close 与 raw_close 都只有 4 位小数,相除会在第 6 位抖动。实测阈值
    1e-4 与 1e-3 结果几乎相同(1563 vs 1530 个变化点),说明真实除权的
    跳变远大于噪声。
    """
    bars = pd.DataFrame([
        {"date": date(2024, 1, 2), "close": 6.5926, "raw_close": 6.6700},
        {"date": date(2024, 1, 3), "close": 7.0572, "raw_close": 7.1400},
        {"date": date(2024, 1, 4), "close": 6.8891, "raw_close": 6.9700},
    ])
    out = ingest.derive_adjust_factors(bars)

    # 三行的因子都是 0.98839x,只差在第 6 位 —— 只该有 1 行
    assert len(out) == 1


def test_derive_factors_skips_unusable_rows(db):
    """raw_close 缺失或零价的行不能参与因子计算。"""
    bars = pd.DataFrame([
        {"date": date(2024, 1, 2), "close": 9.0, "raw_close": None},
        {"date": date(2024, 1, 3), "close": 0.0, "raw_close": 10.0},
        {"date": date(2024, 1, 4), "close": 9.5, "raw_close": 10.0},
    ])
    out = ingest.derive_adjust_factors(bars)

    assert list(out["divid_operate_date"]) == [date(2024, 1, 4)]


def test_source_column_distinguishes_authority_from_derived(db):
    """source 必须能区分权威值与自算值,否则审计结论会被悄悄稀释。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.566931),
    ]))
    ingest.upsert_adjust_factors(
        db, "bj.920000",
        _factors([("bj.920000", date(2023, 5, 8), 0.942579, None)]),
        source="sina")

    assert db.get(AdjustFactor, ("sh.600519", date(2020, 6, 24))).source == "baostock"
    assert db.get(AdjustFactor, ("bj.920000", date(2023, 5, 8))).source == "sina"


def test_baostock_sync_skips_beijing_codes(db, monkeypatch):
    """baostock 不覆盖北交所,默认采集必须跳过,不白发注定失败的请求。"""
    from app.models import Stock as _Stock
    db.add(_Stock(code="bj.920000"))
    db.commit()
    seen: list[str] = []

    monkeypatch.setattr(ingest.baostock_client, "login_session",
                        lambda: _nullcontext())
    monkeypatch.setattr(ingest.baostock_client, "fetch_adjust_factors",
                        lambda code, start, end: seen.append(code) or _factors([]))

    ingest.sync_adjust_factors(db)

    assert "bj.920000" not in seen
    assert sorted(seen) == ["sh.600519", "sz.000001"]


def test_derive_factors_survives_low_price_rounding_noise(db):
    """低价股的舍入噪声不得被当成除权 —— 这个坑被高价股掩盖过。

    close/raw_close 各只有 4 位小数,噪声大小取决于**股价量级**:
    - 茅台(1300 元):相邻因子相对变化约 1e-06,阈值 1e-4 看似够用
    - bj.920000(约 10 元):P50=2.1e-04, P90=1.1e-03, P99=1.9e-03 全是噪声

    实测用 1e-4 阈值跑 bj.920000 的 1353 行日线,产出 898 个「除权日」
    (压缩比 1.5:1,而茅台是 175:1),而真实除权只有 4 次。
    """
    # 模拟低价股:因子真值恒为 0.772,但 4 位小数相除后在 1e-4~1e-3 抖动
    noisy = [
        (date(2021, 3, 1), 7.7200, 10.0000),   # 0.772000
        (date(2021, 3, 2), 8.5029, 11.0100),   # 0.772289
        (date(2021, 3, 3), 6.9508, 9.0000),    # 0.772311
        (date(2021, 3, 4), 7.4112, 9.6000),    # 0.772000
        # 真实除权:跳到 0.9425(相对变化 22%,远超噪声)
        (date(2021, 6, 17), 9.4250, 10.0000),
    ]
    bars = pd.DataFrame(
        [{"date": d, "close": c, "raw_close": r} for d, c, r in noisy])

    out = ingest.derive_adjust_factors(bars)

    # 只该有首行 + 那次真实除权,不该有 4 个假除权日
    assert list(out["divid_operate_date"]) == [date(2021, 3, 1), date(2021, 6, 17)]
    assert out["fore_factor"].tolist() == pytest.approx([0.772, 0.9425], rel=1e-3)
