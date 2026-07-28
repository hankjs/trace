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


class BarForCoverage(_StockBase):
    """最小 quant_daily_bar:护栏的统计口径只算「有日线的股票」。

    数据源不覆盖的品种(如北交所 sh.92xxxx,baostock 既无上市日也无日线)
    永远补不上 list_date,计入分母会让护栏永久触发。
    """

    __tablename__ = "quant_daily_bar"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)
    date: Mapped[date] = mapped_column(Date, primary_key=True)
    # ST 过滤取 day 当日的逐日状态(alembic 0010);NULL 表示未采集
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
    monkeypatch.setattr(universe, "DailyBar", BarForCoverage)
    with Session(engine) as db:
        db.add_all(rows)
        # 上面 6 只都有日线,故都计入护栏的统计口径;DAY 当日显式标 is_st=False
        # (严格口径要求 confirmed non-ST,不再回退 quant_stock.is_st)
        db.add_all([BarForCoverage(code=r.code, date=date(2024, 1, 2))
                    for r in rows])
        db.add_all([
            BarForCoverage(code="sh.good", date=DAY, is_st=False),
            BarForCoverage(code="sh.good2", date=DAY, is_st=False),
            BarForCoverage(code="sh.newbie", date=DAY, is_st=False),
            BarForCoverage(code="sh.delisted", date=DAY, is_st=False),
            # sh.st 当日不建 bar:无 is_st 确认 → 严格排除
            BarForCoverage(code="sh.unknown", date=DAY, is_st=False),
            # 退市日之前的查询日需要 confirmed non-ST
            BarForCoverage(code="sh.delisted", date=date(2023, 6, 30), is_st=False),
            BarForCoverage(code="sh.good", date=date(2023, 6, 30), is_st=False),
            BarForCoverage(code="sh.good2", date=date(2023, 6, 30), is_st=False),
        ])
        db.commit()
        yield db


def test_all_kind_excludes_st_delisted_and_new_listings(all_db):
    """kind='all':ST、已退市、上市未满 60 天的票都要被剔除。"""
    # 该 fixture 当日有 bar 的 5 只中 1 只缺 list_date(20%),超过 5% 硬阈值
    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.2)

    assert codes == ["sh.good", "sh.good2"]
    assert "sh.st" not in codes          # ST / 无当日 is_st=False
    assert "sh.delisted" not in codes    # 已退市
    assert "sh.newbie" not in codes      # 新股
    assert "sh.unknown" not in codes     # list_date 缺失


def test_all_kind_min_list_days_is_configurable(all_db):
    """min_list_days 是池属性:放宽到 5 天后新股进池。"""
    codes = universe.resolve_pool(all_db, DAY, kind="all", min_list_days=5,
                                  max_missing_ratio=0.2)

    assert "sh.newbie" in codes


def test_all_kind_refuses_to_resolve_when_list_date_coverage_is_poor(all_db):
    """list_date 缺失超阈值:拒绝解析而不是返回半个池子(硬护栏)。

    kind='all' 下 allow_current_fallback 那种"缺历史成分就抛错"的护栏失效了
    ——全A 任意历史日都能解析出一个结果,池子少三成也照样跑完回测。
    """
    with pytest.raises(universe.IncompleteListingDataError) as exc:
        universe.resolve_pool(all_db, DAY, kind="all")  # 默认阈值 5%

    # 报错必须点明缺多少、占比、以及怎么办。
    # 分母只计「研究日当日有 bar」的票(走 date 索引,避免全表 DISTINCT):
    # DAY 当日 5 只有 bar(sh.st 无当日 bar 不计入),其中 1 只缺 list_date。
    msg = str(exc.value)
    assert "1/5" in msg
    assert "20.0%" in msg
    assert "list_date" in msg


def test_all_kind_ignores_stocks_without_bars_in_coverage_check(all_db):
    """无日线的品种不计入护栏分母,否则默认口径会永久不可用。

    实测踩过:baostock 不提供北交所(sh.92xxxx)数据,库里 330 只北交所股票
    既无上市日也无日线,永远补不上 list_date。若把它们计入分母,缺失率
    5.9% 恒超 5% 阈值,kind='all' 作为默认口径就永久抛错——而它们本来也
    不该进池子(无日线无法回测)。
    """
    # 再插 20 只「无日线且缺 list_date」的票:分母不变,缺失率不受影响
    all_db.add_all([
        StockWithListing(code=f"sh.92{i:04d}", list_date=None,
                         delist_date=None, is_st=False)
        for i in range(20)
    ])
    all_db.commit()

    # 仍是 1/5(当日有 bar)而非 21/25 —— 这 20 只被排除在统计之外
    with pytest.raises(universe.IncompleteListingDataError) as exc:
        universe.resolve_pool(all_db, DAY, kind="all")
    assert "1/5" in str(exc.value)

    # 放宽阈值后它们也不会出现在池子里
    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.2)
    assert codes == ["sh.good", "sh.good2"]


def test_all_kind_warns_but_proceeds_below_threshold(all_db, caplog):
    """缺失比例在阈值内:告警放行(少量元数据滞后是常态)。"""
    with caplog.at_level("WARNING"):
        codes = universe.resolve_pool(all_db, DAY, kind="all",
                                      max_missing_ratio=0.2)

    assert codes == ["sh.good", "sh.good2"]
    warnings = [r.getMessage() for r in caplog.records]
    assert any("list_date" in m for m in warnings)
    assert any("1/5" in m for m in warnings)


def test_delist_date_after_day_still_counts_as_listed(all_db):
    """退市日晚于查询日:那天它还在市,必须在池内(point-in-time)。"""
    codes = universe.resolve_pool(all_db, date(2023, 6, 30), kind="all",
                                  max_missing_ratio=0.2)

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

    def _spy(db, day, min_list_days=universe.DEFAULT_MIN_LIST_DAYS,
             max_missing_ratio=universe.MAX_MISSING_LIST_DATE_RATIO):
        called["hit"] = True
        return ["sh.all"]

    monkeypatch.setattr(universe, "all_market_pool", _spy)
    assert universe.resolve_pool(index_db, DAY) == ["sh.all"]
    assert called.get("hit") is True


def test_all_kind_uses_daily_st_not_current_snapshot(all_db):
    """ST 过滤必须用 day 当日的逐日状态,不能用 quant_stock.is_st 当前快照。

    这是本次改造的核心:用当前状态过滤历史样本是系统性前视偏差 —— 实测抽样
    8 只当前 ST 股,22464 个交易日里真正处于 ST 的只有 14.4%,用当前标记会把
    其余 85.6% 一并剔除,而被剔掉的恰是后来才出问题的公司(alembic 0010)。

    构造:sh.st 当前是 ST(quant_stock.is_st=True),但 DAY 当日 is_st=False
    —— 它应当**入池**,因为研究日当时它不是 ST。
    """
    # DAY 当日该股不是 ST(逐日历史为 False)
    all_db.add(BarForCoverage(code="sh.st", date=DAY, is_st=False))
    all_db.commit()

    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.3)

    assert "sh.st" in codes, "当日非 ST 的票被当前快照错误剔除(前视偏差)"


def test_all_kind_excludes_stock_that_is_st_on_that_day(all_db):
    """反向:当日确实是 ST 就必须剔除,即便 quant_stock.is_st 还没更新。"""
    bar = all_db.get(BarForCoverage, ("sh.good", DAY))
    assert bar is not None
    bar.is_st = True
    all_db.commit()

    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.3)

    assert "sh.good" not in codes


def test_all_kind_excludes_when_daily_st_missing(all_db):
    """当日无 is_st 确认(NULL/无 bar)时严格排除,不回退 quant_stock.is_st。

    开发阶段宁可池子变小,也不用当前 ST 快照伪造历史资格(前视偏差)。
    """
    # sh.st 在 DAY 当日无 is_st=False 确认(fixture 未给它 DAY 行)
    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.3)

    assert "sh.st" not in codes, "缺当日 is_st 确认时应排除,不得回退当前快照"


def test_all_kind_does_not_use_stock_st_as_non_st_proxy(all_db):
    """quant_stock.is_st=False 且无当日 bar 时仍不得入池。"""
    all_db.add(StockWithListing(
        code="sh.nohist", list_date=date(2018, 1, 1),
        delist_date=None, is_st=False,
    ))
    all_db.add(BarForCoverage(code="sh.nohist", date=date(2024, 1, 2)))
    all_db.commit()

    codes = universe.resolve_pool(all_db, DAY, kind="all", max_missing_ratio=0.3)
    assert "sh.nohist" not in codes
