"""前复权重锚检测:必须在尺度错乱时强制全量回填,不得静默 upsert。

依据 REVIEW §3.1。两个必测场景:
1. 新旧尺度混接(分红后 baostock 回溯调整全部历史前复权价);
2. 无重叠行(库中有历史但取不到重叠日,原实现 `if stored and ...` 静默放过)。
"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import ingest
from app.db import Base
from app.models import DailyBar, Stock


def _session() -> Session:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_bars(db: Session, code: str, rows: list[tuple[date, float, float]]) -> None:
    """rows: (date, close(前复权), raw_close(不复权))"""
    for day, close, raw in rows:
        db.add(DailyBar(code=code, date=day, open=close, high=close,
                        low=close, close=close, raw_close=raw,
                        volume=1000, amount=close * 1000))
    db.commit()


def _frame(rows: list[tuple[date, float, float]]) -> pd.DataFrame:
    return pd.DataFrame(
        [
            {"date": day, "open": close, "high": close, "low": close,
             "close": close, "raw_close": raw, "volume": 1000.0,
             "amount": close * 1000}
            for day, close, raw in rows
        ]
    )


def _patch_fetch(monkeypatch, df: pd.DataFrame) -> list[tuple]:
    """拦截 baostock,记录全量回填的调用参数。"""
    calls: list[tuple] = []

    def fake_fetch(code, start, end):
        calls.append((code, start, end))
        return df

    monkeypatch.setattr(ingest.baostock_client, "fetch_daily_bars", fake_fetch)
    return calls


# --- 场景 1:新旧尺度混接 ------------------------------------------------------


def test_reanchor_detects_mixed_scale_and_forces_full_backfill(monkeypatch):
    """分红后 baostock 回溯调整前复权价:必须整段重拉,而不是把新尺度接上去。

    旧尺度:close == raw_close(复权系数 1.0)
    新尺度:close == raw_close * 0.9(分红后系数变 0.9)
    重叠日 7/20 两侧 raw_close 相同、close 不同 -> 系数偏差 10% >> 阈值。
    """
    code = "sh.600519"
    old = [(date(2026, 7, 17), 100.0, 100.0), (date(2026, 7, 20), 102.0, 102.0)]
    incoming = [(date(2026, 7, 20), 91.8, 102.0), (date(2026, 7, 21), 92.7, 103.0)]

    with _session() as db:
        _seed_bars(db, code, old)
        df = _frame(incoming)

        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True
        assert verdict.reason == "factor_mismatch"

        # 全量回填返回的数据:整段都是新尺度
        full = _frame([(date(2026, 7, 17), 90.0, 100.0),
                       (date(2026, 7, 20), 91.8, 102.0),
                       (date(2026, 7, 21), 92.7, 103.0)])
        calls = _patch_fetch(monkeypatch, full)
        n = ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 21))

        # 断言真的触发了全量回填,且起点足够早以覆盖库中全部历史
        assert len(calls) == 1, "未触发全量回填"
        assert calls[0][1] <= date(2026, 7, 17), \
            "回填起点晚于库中最早日期,旧尺度行不会被覆盖"
        assert n == 3

        stored = {
            r.date: (r.close, r.raw_close)
            for r in db.execute(
                select(DailyBar).where(DailyBar.code == code)).scalars().all()
        }
        # 关键断言:旧尺度那一行已被新尺度覆盖,库里不再横跨两种尺度
        assert stored[date(2026, 7, 17)][0] == pytest.approx(90.0)
        factors = {round(c / r, 6) for c, r in stored.values()}
        assert factors == {0.9}, f"库中仍混有多种复权尺度: {factors}"


def test_reanchor_detected_when_first_day_missing_but_later_days_overlap(monkeypatch):
    """本批首日库中没有、但后续日有重叠且尺度已变 -> 仍须判定重锚。

    旧实现只取 `df["date"].min()` 一天去比对,首日 `stored is None` 就直接
    `upsert_bars`,后面那些真正能证明尺度已变的重叠日**根本没被看过**。
    这是单点比对的核心缺陷。
    """
    code = "sh.600028"
    # 库中只有 7/20(注意:没有 7/17,而 7/17 正是本批最早一天)
    old = [(date(2026, 7, 20), 102.0, 102.0)]
    incoming = [(date(2026, 7, 17), 90.0, 100.0),   # 库中无此日
                (date(2026, 7, 20), 91.8, 102.0)]   # 重叠且系数已变 1.0 -> 0.9

    with _session() as db:
        _seed_bars(db, code, old)
        df = _frame(incoming)

        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True, "首日无重叠就放过 = 单点比对的洞"
        assert verdict.reason == "factor_mismatch"

        full = _frame(incoming)
        calls = _patch_fetch(monkeypatch, full)
        ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 20))
        assert len(calls) == 1, "未触发全量回填"

        rows = db.execute(
            select(DailyBar).where(DailyBar.code == code)).scalars().all()
        factors = {round(r.close / r.raw_close, 6) for r in rows}
        assert factors == {0.9}, f"库中仍混有多种复权尺度: {factors}"


def test_same_scale_increment_upserts_without_refetch(monkeypatch):
    """尺度一致(系数相同)时只做 upsert,不得无谓触发全量回填。"""
    code = "sz.000001"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 7, 17), 100.0, 100.0),
                              (date(2026, 7, 20), 102.0, 102.0)])
        df = _frame([(date(2026, 7, 20), 102.0, 102.0),
                     (date(2026, 7, 21), 103.0, 103.0)])
        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is False
        assert verdict.reason == "factor_match"

        calls = _patch_fetch(monkeypatch, df)
        n = ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 21))
        assert calls == [], "尺度一致却触发了全量回填"
        assert n == 2
        assert db.execute(
            select(DailyBar.date).where(DailyBar.code == code)
        ).scalars().all() == [date(2026, 7, 17), date(2026, 7, 20),
                              date(2026, 7, 21)]


def test_factor_change_with_unchanged_close_is_detected(monkeypatch):
    """close 恰好没变但复权系数变了 -> 只比 close 永远发现不了。

    构造:库中 close=100 / raw_close=100(系数 1.0);
    新数据 close=100 / raw_close=125(系数 0.8)。
    close 完全相同,单点 close 比对必然放过;比系数则一眼看出尺度已换。
    """
    code = "sh.600030"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 7, 20), 100.0, 100.0)])
        df = _frame([(date(2026, 7, 20), 100.0, 125.0),
                     (date(2026, 7, 21), 101.0, 126.25)])

        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True, "close 未变即放过 = 只看价格的洞"
        assert verdict.reason == "factor_mismatch"

        calls = _patch_fetch(monkeypatch, df)
        ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 21))
        assert len(calls) == 1, "未触发全量回填"


def test_price_only_shift_without_raw_close_still_detected(monkeypatch):
    """老数据 raw_close 为空时退化为 close 比对,仍须判定重锚。"""
    code = "sh.600000"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 7, 20), 102.0, None)])
        df = _frame([(date(2026, 7, 20), 91.8, None),
                     (date(2026, 7, 21), 92.7, None)])
        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True
        assert verdict.reason == "close_mismatch"

        calls = _patch_fetch(monkeypatch, df)
        ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 21))
        assert len(calls) == 1


# --- 场景 2:无重叠行(stored is None)---------------------------------------


def test_reanchor_no_overlap_warns_and_forces_backfill(monkeypatch, caplog):
    """库中有历史但与本批无任何重叠日:必须告警并强制回填,不得静默 upsert。

    这是旧实现 `if stored and ...` 的洞:`stored is None` 直接落到
    `upsert_bars`,把未经校验的新尺度 bar 接到旧尺度历史上。
    """
    code = "sh.600036"
    gap_history = [(date(2026, 5, 4), 50.0, 50.0), (date(2026, 5, 5), 51.0, 51.0)]
    incoming = [(date(2026, 7, 20), 45.9, 51.0), (date(2026, 7, 21), 46.8, 52.0)]

    with _session() as db:
        _seed_bars(db, code, gap_history)
        df = _frame(incoming)

        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True
        assert verdict.reason == "no_overlap"

        full = _frame([(date(2026, 5, 4), 45.0, 50.0),
                       (date(2026, 5, 5), 45.9, 51.0),
                       (date(2026, 7, 20), 45.9, 51.0),
                       (date(2026, 7, 21), 46.8, 52.0)])
        calls = _patch_fetch(monkeypatch, full)
        with caplog.at_level("WARNING"):
            n = ingest._upsert_with_reanchor_check(
                db, code, df, fallback_start=date(2026, 7, 11),
                end=date(2026, 7, 21))

        assert len(calls) == 1, "无重叠行时未强制回填(旧实现在此静默 upsert)"
        assert calls[0][1] == date(2026, 5, 4), "回填起点未回到库中最早日期"
        assert n == 4
        warnings = [r.getMessage() for r in caplog.records
                    if r.levelname == "WARNING"]
        assert any("前复权重锚" in m and "no_overlap" in m for m in warnings), \
            f"未告警,实际日志: {warnings}"

        rows = db.execute(
            select(DailyBar).where(DailyBar.code == code)).scalars().all()
        assert len(rows) == 4
        factors = {round(r.close / r.raw_close, 6) for r in rows}
        assert factors == {0.9}


def test_first_ingest_without_history_is_plain_upsert(monkeypatch):
    """库中完全无历史(新股首次 ingest):不存在混接风险,直接 upsert。"""
    code = "sz.301999"
    with _session() as db:
        df = _frame([(date(2026, 7, 20), 20.0, 20.0),
                     (date(2026, 7, 21), 21.0, 21.0)])
        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is False
        assert verdict.reason == "no_history"

        calls = _patch_fetch(monkeypatch, df)
        n = ingest._upsert_with_reanchor_check(
            db, code, df, fallback_start=date(2026, 7, 11), end=date(2026, 7, 21))
        assert calls == []
        assert n == 2


def test_overlap_rows_without_comparable_price_force_backfill(monkeypatch):
    """重叠行存在但两侧都无可比价格(零价):同样不得假定尺度一致。"""
    code = "sh.600001"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 7, 20), 0.0, 0.0)])
        df = _frame([(date(2026, 7, 20), 0.0, 0.0)])
        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is True
        assert verdict.reason == "unverifiable"


# --- safe_backfill:全市场回填不得绕过检查 -----------------------------------


def test_safe_backfill_routes_through_reanchor_check(monkeypatch):
    """有历史时 safe_backfill 必须过重锚判定;尺度错乱 -> 从最早日期重拉。"""
    code = "sh.601318"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 6, 1), 60.0, 60.0),
                              (date(2026, 7, 20), 61.0, 61.0)])
        incoming = _frame([(date(2026, 7, 20), 54.9, 61.0),
                           (date(2026, 7, 21), 55.8, 62.0)])
        calls = _patch_fetch(monkeypatch, incoming)
        ingest.safe_backfill(db, code, date(2026, 7, 1), date(2026, 7, 21))
        # 第一次是判定用的拉取,第二次是重锚后的全量回填(起点回到 6/1)
        assert len(calls) == 2
        assert calls[0][1] == date(2026, 7, 1)
        assert calls[1][1] == date(2026, 6, 1)


def test_safe_backfill_force_skips_check(monkeypatch):
    """--force-rescale 直接全量重拉,只拉一次。"""
    code = "sh.601318"
    with _session() as db:
        _seed_bars(db, code, [(date(2026, 6, 1), 60.0, 60.0)])
        df = _frame([(date(2026, 6, 1), 54.0, 60.0)])
        calls = _patch_fetch(monkeypatch, df)
        ingest.safe_backfill(db, code, date(2026, 6, 1), date(2026, 7, 21),
                             force=True)
        assert len(calls) == 1


# --- import_stock_list:ST 改名必须被更新 -------------------------------------


def _patch_stock_sources(monkeypatch, listing: list[tuple[str, str]],
                         basic: dict[str, dict]) -> None:
    monkeypatch.setattr(
        ingest.akshare_client, "fetch_stock_list",
        lambda: pd.DataFrame([{"code": c, "name": n} for c, n in listing]),
    )
    monkeypatch.setattr(ingest, "_fetch_stock_basic_map", lambda: basic)


def test_import_stock_list_updates_st_rename(monkeypatch):
    """初次导入后改名为 *ST 的股票,再次导入必须刷新 name 与 is_st。"""
    with _session() as db:
        _patch_stock_sources(
            monkeypatch,
            [("sz.000001", "平安银行"), ("sh.600001", "邯郸钢铁")],
            {"sh.600001": {"name": "邯郸钢铁", "list_date": date(1998, 1, 1),
                           "delist_date": None, "status": "1"}},
        )
        first = ingest.import_stock_list(db)
        assert first["imported"] == 2
        assert db.get(Stock, "sh.600001").is_st is False
        assert db.get(Stock, "sh.600001").list_date == date(1998, 1, 1)

        # 第二次:该股改名为 *ST 邯钢
        _patch_stock_sources(
            monkeypatch,
            [("sz.000001", "平安银行"), ("sh.600001", "*ST邯钢")],
            {"sh.600001": {"name": "*ST邯钢", "list_date": date(1998, 1, 1),
                           "delist_date": None, "status": "1"}},
        )
        second = ingest.import_stock_list(db)

        assert second["imported"] == 0
        assert second["updated"] == 2
        stock = db.get(Stock, "sh.600001")
        assert stock.name == "*ST邯钢"
        assert stock.is_st is True, "改名为 *ST 未刷新 is_st(旧实现只 insert 不 update)"


def test_import_stock_list_marks_delisted_without_deleting(monkeypatch):
    """名录中消失的股票标 delist_date,不删行(历史回测需要)。"""
    with _session() as db:
        _patch_stock_sources(monkeypatch,
                             [("sz.000001", "平安银行"), ("sh.600002", "齐鲁石化")], {})
        ingest.import_stock_list(db)

        _patch_stock_sources(monkeypatch, [("sz.000001", "平安银行")],
                             {"sh.600002": {"name": "齐鲁石化",
                                            "list_date": date(1998, 4, 1),
                                            "delist_date": date(2006, 3, 1),
                                            "status": "0"}})
        result = ingest.import_stock_list(db)

        assert result["delisted"] == 1
        stock = db.get(Stock, "sh.600002")
        assert stock is not None, "退市股被删行,历史回测将缺数据"
        assert stock.delist_date == date(2006, 3, 1)
        assert stock.is_st is True


def test_backfill_list_dates_falls_back_to_first_bar(monkeypatch):
    """baostock 拿不到上市日的,退化为库中最早一根日线(全A 池解析前置)。"""
    with _session() as db:
        db.add(Stock(code="sh.600003", name="某股"))
        db.commit()
        _seed_bars(db, "sh.600003", [(date(2015, 3, 2), 10.0, 10.0),
                                     (date(2015, 3, 3), 10.5, 10.5)])
        monkeypatch.setattr(ingest, "_fetch_stock_basic_map", lambda: {})

        result = ingest.backfill_list_dates(db)

        assert result["from_bars"] == 1
        assert db.get(Stock, "sh.600003").list_date == date(2015, 3, 2)


def test_reanchor_tolerance_ignores_float_noise():
    """阈值内的浮点噪声不得误判为重锚(否则全市场天天全量重拉)。"""
    code = "sh.600004"
    day = date(2026, 7, 20)
    with _session() as db:
        _seed_bars(db, code, [(day, 100.0, 100.0)])
        noisy = 100.0 * (1 + ingest.REANCHOR_TOLERANCE / 2)
        df = _frame([(day, noisy, 100.0), (day + timedelta(days=1), 101.0, 101.0)])
        verdict = ingest.detect_reanchor(db, code, df)
        assert verdict.reanchored is False
