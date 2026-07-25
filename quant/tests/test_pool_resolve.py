"""统一池解析 resolve_pool 的分派与各 kind 口径。

对应 brief §3.1 / §5:
- kind='all' 断言 ST/退市/新股被剔除
- kind='index' 断言 point-in-time 正确

`quant_stock.list_date/delist_date/is_st` 由 agent-migrate 增加。为了在 schema
合并前就把口径固定住,这里用一张本地等价表模拟那三列(不改 app/models.py)。
schema 落地后本文件的断言直接适用于真实列。
"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pytest
from sqlalchemy import Boolean, Date, String, create_engine
from sqlalchemy.orm import (DeclarativeBase, Mapped, Session, mapped_column)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import universe
from app.db import Base
from app.models import IndexMember

DAY = date(2024, 6, 30)


# --------------------------------------------------------------------------
# kind='index':point-in-time
# --------------------------------------------------------------------------


@pytest.fixture()
def index_db():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        db.add_all([
            # 区间覆盖 DAY -> 在册
            IndexMember(id=1, index_name="hs300", code="sh.in",
                        in_date=date(2024, 1, 1), out_date=None),
            # DAY 之前就调出 -> 不在册(out_date 是开区间上界)
            IndexMember(id=2, index_name="hs300", code="sh.out",
                        in_date=date(2023, 1, 1), out_date=date(2024, 6, 1)),
            # out_date 恰好等于 DAY -> 半开区间 [in, out) 不含 DAY
            IndexMember(id=3, index_name="hs300", code="sh.boundary",
                        in_date=date(2023, 1, 1), out_date=DAY),
            # DAY 之后才进 -> 不在册
            IndexMember(id=4, index_name="hs300", code="sh.later",
                        in_date=date(2024, 12, 1), out_date=None),
            # 另一个指数,用于验证 index_name 过滤
            IndexMember(id=5, index_name="zz500", code="sh.zz",
                        in_date=date(2024, 1, 1), out_date=None),
        ])
        db.commit()
        yield db


def test_index_kind_is_point_in_time(index_db):
    """只返回 in_date <= DAY < out_date 的成分。"""
    codes = universe.resolve_pool(index_db, DAY, kind="index")

    assert codes == ["sh.in", "sh.zz"]
    # 已调出、边界日、未来才进的都不能出现
    for excluded in ("sh.out", "sh.boundary", "sh.later"):
        assert excluded not in codes


def test_index_kind_respects_index_name(index_db):
    """index_name 限定单指数,不再由调用方自己重写 SQL。"""
    assert universe.resolve_pool(
        index_db, DAY, kind="index", index_name="hs300") == ["sh.in"]
    assert universe.resolve_pool(
        index_db, DAY, kind="index", index_name="zz500") == ["sh.zz"]


def test_index_kind_at_earlier_day_returns_the_then_members(index_db):
    """回看更早的日子:当时在册的票要回来(防幸存者偏差的核心)。"""
    codes = universe.resolve_pool(index_db, date(2024, 3, 1), kind="index")

    # sh.out / sh.boundary 在 2024-03-01 时都还在册
    assert "sh.out" in codes
    assert "sh.boundary" in codes


# --------------------------------------------------------------------------
# kind='all':剔除新股 / 退市 / ST
# --------------------------------------------------------------------------


class _StockBase(DeclarativeBase):
    pass


class StockWithListing(_StockBase):
    """migrate 落地后的 quant_stock 形态(多 list_date/delist_date/is_st)。"""

    __tablename__ = "quant_stock"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)
    name: Mapped[str] = mapped_column(String(64), default="")
    industry: Mapped[str] = mapped_column(String(64), default="")
    is_watch: Mapped[bool] = mapped_column(Boolean, default=False)
    list_date: Mapped[date | None] = mapped_column(Date, nullable=True)
    delist_date: Mapped[date | None] = mapped_column(Date, nullable=True)
    is_st: Mapped[bool | None] = mapped_column(Boolean, nullable=True)


@pytest.fixture()
def all_db(monkeypatch):
    """建带三列的 quant_stock,并把 universe 的 Stock 指向它。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    _StockBase.metadata.create_all(engine)

    rows = [
        # 正常票:上市已久、未退市、非 ST
        StockWithListing(code="sh.good", list_date=date(2020, 1, 1),
                         delist_date=None, is_st=False),
        # is_st 为 NULL 时视为非 ST
        StockWithListing(code="sh.good2", list_date=date(2019, 5, 6),
                         delist_date=None, is_st=None),
        # 新股:上市距 DAY 不足 min_list_days(60 天)
        StockWithListing(code="sh.newbie", list_date=date(2024, 6, 1),
                         delist_date=None, is_st=False),
        # 已退市:delist_date 早于 DAY
        StockWithListing(code="sh.delisted", list_date=date(2015, 1, 1),
                         delist_date=date(2024, 1, 1), is_st=False),
        # ST
        StockWithListing(code="sh.st", list_date=date(2016, 1, 1),
                         delist_date=None, is_st=True),
        # list_date 缺失:会被静默漏掉,必须有告警
        StockWithListing(code="sh.unknown", list_date=None,
                         delist_date=None, is_st=False),
    ]
    monkeypatch.setattr(universe, "Stock", StockWithListing)
    with Session(engine) as db:
        db.add_all(rows)
        db.commit()
        yield db


def test_all_kind_excludes_st_delisted_and_new_listings(all_db):
    """kind='all':ST、已退市、上市未满 60 天的票都要被剔除。"""
    codes = universe.resolve_pool(all_db, DAY, kind="all")

    assert codes == ["sh.good", "sh.good2"]
    assert "sh.st" not in codes          # ST
    assert "sh.delisted" not in codes    # 已退市
    assert "sh.newbie" not in codes      # 新股
    assert "sh.unknown" not in codes     # list_date 缺失


def test_all_kind_min_list_days_is_configurable(all_db):
    """min_list_days 是池属性:放宽到 5 天后新股进池。"""
    codes = universe.resolve_pool(all_db, DAY, kind="all", min_list_days=5)

    assert "sh.newbie" in codes


def test_all_kind_warns_when_list_date_missing(all_db, caplog):
    """list_date 未回填会静默漏票,必须有计数告警(取代失效的 fallback 护栏)。"""
    with caplog.at_level("WARNING"):
        universe.resolve_pool(all_db, DAY, kind="all")

    warnings = [r.getMessage() for r in caplog.records]
    # 告警必须点名 list_date 并给出缺失计数(6 只里 1 只缺)
    assert any("list_date" in m for m in warnings)
    assert any("1/6" in m for m in warnings)


def test_delist_date_after_day_still_counts_as_listed(all_db):
    """退市日晚于查询日:那天它还在市,必须在池内(point-in-time)。"""
    codes = universe.resolve_pool(all_db, date(2023, 6, 30), kind="all")

    assert "sh.delisted" in codes  # 2024-01-01 才退市


# --------------------------------------------------------------------------
# 分派本身
# --------------------------------------------------------------------------


def test_resolve_pool_rejects_unknown_kind(index_db):
    with pytest.raises(ValueError, match="未知池类型"):
        universe.resolve_pool(index_db, DAY, kind="etf")


def test_static_kind_requires_pool_id(index_db):
    with pytest.raises(ValueError, match="pool_id"):
        universe.resolve_pool(index_db, DAY, kind="static")


def test_default_kind_is_all_market(index_db, monkeypatch):
    """默认口径必须是全A,不是指数成分。"""
    called = {}

    def _spy(db, day, min_list_days=universe.DEFAULT_MIN_LIST_DAYS):
        called["hit"] = True
        return ["sh.all"]

    monkeypatch.setattr(universe, "all_market_pool", _spy)
    assert universe.resolve_pool(index_db, DAY) == ["sh.all"]
    assert called.get("hit") is True
