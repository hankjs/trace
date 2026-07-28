"""按日批量采集链路(P1~P3)的单元测试。

全部用合成数据/mock:baostock 层一律 monkeypatch,不触网;库用内存 SQLite。
覆盖:
1. raw_to_qfq 换算公式(待 P0 spike 验证,验证不过只需改该函数与本测试);
2. 批量封装的规范化(fetch_market_daily_bars / fetch_market_adjust_factors);
3. sync_adjust_factors_for_day 的 upsert 与因子变化检测;
4. ingest_market_day 的换算、幂等、无条件覆盖、safe_backfill 兜底分支;
5. scheduler 开关 bulk_daily_bars 关/开两条路径。
"""
from __future__ import annotations

import contextlib
import sys
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, func, select
from sqlalchemy.orm import Session, sessionmaker
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import scheduler  # noqa: E402
from app.data import baostock_client, ingest  # noqa: E402
from app.db import Base  # noqa: E402
from app.models import AdjustFactor, DailyBar, IndexMember, TradeCalendar  # noqa: E402

DAY = date(2026, 7, 24)


def _session() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


@pytest.fixture()
def db() -> Session:
    with _session() as session:
        yield session


def _nullcontext():
    return contextlib.nullcontext()


def _bulk_bars(rows: list[tuple]) -> pd.DataFrame:
    """模拟 fetch_market_daily_bars 的规范化输出(不复权原始价)。

    rows: (code, open, high, low, close, volume, amount, is_st)
    """
    return pd.DataFrame([
        {"date": DAY, "code": code, "open": o, "high": h, "low": l,
         "close": c, "volume": v, "amount": a, "tradestatus": True,
         "is_st": st}
        for code, o, h, l, c, v, a, st in rows
    ])


def _bulk_factors(rows: list[tuple]) -> pd.DataFrame:
    """rows: (code, divid_operate_date, fore_factor, back_factor)"""
    return pd.DataFrame(
        rows, columns=["code", "divid_operate_date", "fore_factor",
                       "back_factor"])


def _mock_market(monkeypatch, bars: pd.DataFrame, factors: pd.DataFrame):
    """mock 掉批量链路的 baostock 访问层。"""
    monkeypatch.setattr(ingest.baostock_client, "login_session", _nullcontext)
    monkeypatch.setattr(ingest.baostock_client, "fetch_market_daily_bars",
                        lambda day: bars)
    monkeypatch.setattr(ingest.baostock_client, "fetch_market_adjust_factors",
                        lambda day: factors)


# ---------------------------------------------------------------- raw_to_qfq


def test_raw_to_qfq_formula():
    """前复权价 = 不复权价 × 当日因子 ÷ 最新因子。"""
    assert ingest.raw_to_qfq(100.0, 0.856267) == pytest.approx(85.6267)
    assert ingest.raw_to_qfq(100.0, 0.9, latest_factor=1.2) == pytest.approx(75.0)
    # 无分红股票因子为 1:前复权价即原始价
    assert ingest.raw_to_qfq(11.43, 1.0) == pytest.approx(11.43)


def test_raw_to_qfq_guards_bad_input():
    assert ingest.raw_to_qfq(None, 0.9) is None
    assert ingest.raw_to_qfq(float("nan"), 0.9) is None
    assert ingest.raw_to_qfq(100.0, 0.0) is None
    assert ingest.raw_to_qfq(100.0, 0.9, latest_factor=0.0) is None


# ------------------------------------------------------- baostock 批量封装


def _raw_market_k_frame() -> pd.DataFrame:
    """模拟 query_daily_history_k_AStock 的原始返回(全字符串)。"""
    cols = ["date", "code", "open", "high", "low", "close", "preclose",
            "volume", "amount", "adjustflag", "turn", "tradestatus",
            "pctChg", "peTTM", "pbMRQ", "psTTM", "pcfNcfTTM", "isST"]
    return pd.DataFrame([
        ["2026-07-24", "sh.600519", "1400.0", "1410.0", "1390.0", "1405.0",
         "1400.0", "10000", "14050000", "3", "0.5", "1", "0.36",
         "20", "8", "5", "6", "0"],
        ["2026-07-24", "sh.600053", "", "", "", "", "11.43",
         "0", "0", "3", "", "0", "", "", "", "", "", "1"],
    ], columns=cols)


def test_fetch_market_daily_bars_normalizes(monkeypatch):
    monkeypatch.setattr(baostock_client._client, "_query_frame",
                        lambda query, ctx: _raw_market_k_frame())

    df = baostock_client.fetch_market_daily_bars(DAY)

    assert list(df.columns) == [
        "date", "code", "open", "high", "low", "close", "preclose",
        "volume", "amount", "turn", "pct_chg", "tradestatus", "is_st",
        "pe_ttm", "pb", "ps_ttm",
    ]
    row = df[df["code"] == "sh.600519"].iloc[0]
    assert row["date"] == DAY
    assert row["close"] == pytest.approx(1405.0)
    assert row["pe_ttm"] == pytest.approx(20.0)
    assert row["pb"] == pytest.approx(8.0)
    assert row["tradestatus"] is True or row["tradestatus"] == True  # noqa: E712
    assert row["is_st"] is False or row["is_st"] == False  # noqa: E712
    # 停牌行(tradestatus=0):价格为 NaN,由 upsert_bars 按既有语义丢弃
    halted = df[df["code"] == "sh.600053"].iloc[0]
    assert pd.isna(halted["close"])
    assert halted["tradestatus"] in (False, 0)
    assert halted["is_st"] in (True, 1)


def test_fetch_market_adjust_factors_normalizes(monkeypatch):
    raw = pd.DataFrame([
        ["sh.600519", "2026-06-26", "0.960527", "7.366525", "1.0"],
        ["sz.000001", "2025-06-12", "0.800000", "3.2", "1.0"],
        ["sh.600000", "", "", "", ""],  # 无除权日/因子的行剔除
    ], columns=["code", "dividOperateDate", "foreAdjustFactor",
                "backAdjustFactor", "adjustFactor"])
    monkeypatch.setattr(baostock_client._client, "_query_frame",
                        lambda query, ctx: raw)

    df = baostock_client.fetch_market_adjust_factors(DAY)

    assert list(df.columns) == ["code", "divid_operate_date", "fore_factor",
                                "back_factor"]
    assert len(df) == 2
    row = df[df["code"] == "sh.600519"].iloc[0]
    assert row["divid_operate_date"] == date(2026, 6, 26)
    assert row["fore_factor"] == pytest.approx(0.960527)


# ------------------------------------------------- sync_adjust_factors_for_day


def test_sync_factors_for_day_upserts_and_detects_changes(db, monkeypatch):
    """因子较库内记录变化的 code 要被识别;upsert 前先比对,基准不被自己覆盖。"""
    # 库内基线:600519 最新除权日 2020-06-24;000001 已是当前值
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.081667)]))
    ingest.upsert_adjust_factors(db, "sz.000001", _bulk_factors([
        ("sz.000001", date(2025, 6, 12), 0.8, 3.2)]))

    monkeypatch.setattr(ingest.baostock_client, "fetch_market_adjust_factors",
                        lambda day: _bulk_factors([
                            # 600519 出现新除权日 -> 变化
                            ("sh.600519", date(2026, 7, 20), 0.9, 7.5),
                            # 000001 与库内一致 -> 无变化
                            ("sz.000001", date(2025, 6, 12), 0.8, 3.2),
                            # 600000 库内无基线 -> 首次同步不算变化
                            ("sh.600000", date(2024, 5, 6), 0.95, 2.0),
                        ]))

    res = ingest.sync_adjust_factors_for_day(db, DAY)

    assert res["changed"] == ["sh.600519"]
    assert res["empty"] is False
    assert res["upserted"] == 3
    assert db.get(AdjustFactor, ("sh.600519", date(2026, 7, 20))) is not None
    assert db.get(AdjustFactor, ("sh.600000", date(2024, 5, 6))) is not None


def test_sync_factors_for_day_detects_revision(db, monkeypatch):
    """同除权日但因子值被上游修订,同样算变化。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.081667)]))
    monkeypatch.setattr(ingest.baostock_client, "fetch_market_adjust_factors",
                        lambda day: _bulk_factors([
                            ("sh.600519", date(2020, 6, 24), 0.9, 6.1)]))

    res = ingest.sync_adjust_factors_for_day(db, DAY)

    assert res["changed"] == ["sh.600519"]
    row = db.get(AdjustFactor, ("sh.600519", date(2020, 6, 24)))
    assert row.fore_factor == pytest.approx(0.9)  # 修订覆盖旧值


def test_sync_factors_for_day_empty_response_keeps_db(db, monkeypatch):
    """空响应视为数据源抖动,不清空已有因子,也不报变化。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.856267, 6.081667)]))
    monkeypatch.setattr(ingest.baostock_client, "fetch_market_adjust_factors",
                        lambda day: _bulk_factors([]))

    res = ingest.sync_adjust_factors_for_day(db, DAY)

    assert res["empty"] is True
    assert res["changed"] == []
    assert db.execute(
        select(func.count()).select_from(AdjustFactor)).scalar() == 1


# --------------------------------------------------------- ingest_market_day


def test_ingest_market_day_converts_raw_to_qfq(db, monkeypatch):
    """批量原始价 × 库内权威因子 = 前复权 OHLC;raw_close 存原始收盘。"""
    # 库内基线与批量因子一致 -> 无变化,不走 safe_backfill
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]))
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sh.600519", 100.0, 102.0, 99.0, 100.0, 1000, 100000, False),
            # 无因子记录 -> 因子 1,前复权价即原始价
            ("sz.000001", 10.0, 10.2, 9.9, 10.1, 2000, 20200, True),
        ]),
        factors=_bulk_factors([
            ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]),
    )

    res = ingest.ingest_market_day(db, DAY)

    assert res["codes"] == 2
    assert res["bars"] == 2
    bar = db.get(DailyBar, ("sh.600519", DAY))
    assert bar.close == pytest.approx(50.0)       # 100 × 0.5
    assert bar.high == pytest.approx(51.0)
    assert bar.raw_close == pytest.approx(100.0)  # 不复权原样存
    assert bar.is_st is False
    bar2 = db.get(DailyBar, ("sz.000001", DAY))
    assert bar2.close == pytest.approx(10.1)      # 因子 1,原始价
    assert bar2.is_st is True                     # isST 落库


def test_ingest_market_day_uses_factor_effective_on_day(db, monkeypatch):
    """换算用 divid_operate_date <= 当日 的末条因子,不是最新记录。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]))
    # 批量因子返回的还是 2020-06-24 那条(600519 当日无新除权)
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sh.600519", 100.0, 100.0, 100.0, 100.0, 1, 1, False)]),
        factors=_bulk_factors([
            ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]),
    )

    ingest.ingest_market_day(db, DAY)

    assert db.get(DailyBar, ("sh.600519", DAY)).close == pytest.approx(50.0)


def test_ingest_market_day_is_idempotent(db, monkeypatch):
    """重复跑同一天:行数不变,值不变(唯一键 upsert)。"""
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sz.000001", 10.0, 10.2, 9.9, 10.1, 1, 1, False)]),
        factors=_bulk_factors([]),
    )
    assert ingest.ingest_market_day(db, DAY)["bars"] == 1
    assert ingest.ingest_market_day(db, DAY)["bars"] == 1

    assert db.execute(
        select(func.count()).select_from(DailyBar)).scalar() == 1
    assert db.get(DailyBar, ("sz.000001", DAY)).close == pytest.approx(10.1)


def test_ingest_market_day_overwrites_existing_row_unconditionally(db, monkeypatch):
    """盘后覆盖是无条件整行 upsert:当日已存在的(盘中残留)行必须被权威值覆盖。"""
    db.add(DailyBar(code="sz.000001", date=DAY, open=9.0, high=9.1, low=8.9,
                    close=9.05, raw_close=9.05, volume=1, amount=1))
    db.commit()
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sz.000001", 10.0, 10.2, 9.9, 10.1, 2000, 20200, False)]),
        factors=_bulk_factors([]),
    )

    ingest.ingest_market_day(db, DAY)

    bar = db.get(DailyBar, ("sz.000001", DAY))
    assert bar.close == pytest.approx(10.1)
    assert bar.volume == pytest.approx(2000)


def test_factor_changed_code_falls_back_to_safe_backfill(db, monkeypatch):
    """因子变化的 code 走单票全历史重拉兜底,不用批量换算的当日行。"""
    # 库内基线停在 2020 年;批量因子出现新除权日 -> 变化
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]))
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sh.600519", 100.0, 100.0, 100.0, 100.0, 1, 1, False),
            ("sz.000001", 10.0, 10.0, 10.0, 10.0, 1, 1, False)]),
        factors=_bulk_factors([
            ("sh.600519", date(2026, 7, 20), 0.9, 7.5)]),
    )
    calls: list[dict] = []

    def fake_safe_backfill(db, code, start, end, force=False):
        calls.append({"code": code, "start": start, "end": end,
                      "force": force})
        return 3

    monkeypatch.setattr(ingest, "safe_backfill", fake_safe_backfill)

    res = ingest.ingest_market_day(db, DAY, sleep_per_reanchor=0)

    assert calls == [{"code": "sh.600519", "start": date(2015, 1, 1),
                      "end": DAY, "force": True}]
    assert res["reanchored"] == ["sh.600519"]
    assert res["factor_changed"] == ["sh.600519"]
    # 600519 的当日行走全历史重拉(mock 不写库),不经批量换算
    assert db.get(DailyBar, ("sh.600519", DAY)) is None
    # 无变化的 000001 正常批量入库
    assert db.get(DailyBar, ("sz.000001", DAY)) is not None


def test_safe_backfill_failure_isolated(db, monkeypatch):
    """单只全历史重拉失败不影响其他 code 的批量入库。"""
    ingest.upsert_adjust_factors(db, "sh.600519", _bulk_factors([
        ("sh.600519", date(2020, 6, 24), 0.5, 6.0)]))
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sh.600519", 100.0, 100.0, 100.0, 100.0, 1, 1, False),
            ("sz.000001", 10.0, 10.0, 10.0, 10.0, 1, 1, False)]),
        factors=_bulk_factors([
            ("sh.600519", date(2026, 7, 20), 0.9, 7.5)]),
    )

    def boom(db, code, start, end, force=False):
        raise RuntimeError("baostock 限速")

    monkeypatch.setattr(ingest, "safe_backfill", boom)

    res = ingest.ingest_market_day(db, DAY, sleep_per_reanchor=0)

    assert res["failed"] == ["sh.600519"]
    assert db.get(DailyBar, ("sz.000001", DAY)) is not None


def test_ingest_market_day_codes_filter(db, monkeypatch):
    """盘后增量只写池内+自选,保持 quant_daily_bar 收录范围不变。"""
    _mock_market(
        monkeypatch,
        bars=_bulk_bars([
            ("sh.600519", 100.0, 100.0, 100.0, 100.0, 1, 1, False),
            ("sz.000001", 10.0, 10.0, 10.0, 10.0, 1, 1, False)]),
        factors=_bulk_factors([]),
    )

    res = ingest.ingest_market_day(db, DAY, codes={"sh.600519"})

    assert res["codes"] == 1
    assert db.get(DailyBar, ("sh.600519", DAY)) is not None
    assert db.get(DailyBar, ("sz.000001", DAY)) is None


def test_ingest_market_day_empty_bulk_writes_nothing(db, monkeypatch):
    """批量日 K 返回空(非交易日/数据源异常)时不写库、不报错。"""
    _mock_market(monkeypatch, bars=_bulk_bars([]), factors=_bulk_factors([]))

    res = ingest.ingest_market_day(db, DAY)

    assert res["bars"] == 0
    assert db.execute(select(func.count()).select_from(DailyBar)).scalar() == 0


# -------------------------------------------------------- scheduler 开关


def _memory_sessionmaker():
    """共享内存库:scheduler 每批新开 Session,必须连同一个库。"""
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return sessionmaker(bind=engine, autoflush=False, expire_on_commit=False)


def _seed_calendar_and_pool(maker, day: date, codes: list[str]) -> None:
    with maker() as db:
        db.add(TradeCalendar(date=day, is_open=True, source="test"))
        for code in codes:
            db.add(IndexMember(index_name="hs300", code=code,
                               in_date=date(2020, 1, 1)))
        db.commit()


def test_switch_off_uses_per_code_path(monkeypatch):
    """开关关闭(默认):走现有按 code 路径,批量入口不被调用。"""
    maker = _memory_sessionmaker()
    _seed_calendar_and_pool(maker, DAY, ["sh.600519"])
    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(scheduler.settings, "bulk_daily_bars", False)
    monkeypatch.setattr(scheduler.baostock_client, "login_session",
                        _nullcontext)
    monkeypatch.setattr(scheduler.ingest, "cleanup_snapshots", lambda *a: 0)
    calls: list[str] = []
    monkeypatch.setattr(
        scheduler.ingest, "ingest_daily",
        lambda db, code, **kw: (calls.append(code), {"has_day_bar": True})[1])
    monkeypatch.setattr(
        scheduler.ingest, "ingest_market_day",
        lambda *a, **kw: (_ for _ in ()).throw(
            AssertionError("开关关闭时不应走批量路径")))

    result = scheduler.job_daily_bars(DAY)

    assert calls == ["sh.600519"]
    assert result["succeeded"] == 1


def test_switch_on_uses_bulk_path(monkeypatch):
    """开关开启:走 ingest_market_day 批量链路,按 code 路径不被调用。

    北交所不在批量结果中属预期,不计入 empty(它们走新浪源)。
    """
    maker = _memory_sessionmaker()
    _seed_calendar_and_pool(maker, DAY, ["sh.600519", "bj.920000"])
    monkeypatch.setattr(scheduler, "SessionLocal", maker)
    monkeypatch.setattr(scheduler.settings, "bulk_daily_bars", True)
    monkeypatch.setattr(scheduler.ingest, "cleanup_snapshots", lambda *a: 0)
    monkeypatch.setattr(
        scheduler.ingest, "ingest_daily",
        lambda *a, **kw: (_ for _ in ()).throw(
            AssertionError("开关开启时不应走按 code 路径")))
    captured: dict = {}

    def fake_market_day(db, day, codes=None, backfill_start=None):
        captured["day"] = day
        captured["codes"] = codes
        return {"codes": 1, "failed": [], "written_codes": ["sh.600519"]}

    monkeypatch.setattr(scheduler.ingest, "ingest_market_day", fake_market_day)

    result = scheduler.job_daily_bars(DAY)

    assert captured["day"] == DAY
    assert captured["codes"] == {"sh.600519", "bj.920000"}
    assert result["succeeded"] == 1
    # bj.920000 不在批量结果中,但不计入 empty
    assert result["empty"] == []
