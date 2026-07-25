"""股票池:成分名录维护 + 按 kind 的统一池解析入口。

- sync_index_members: 从 baostock 同步成分股,增量维护 in_date/out_date;
- resolve_pool: **统一入口**,按 kind 分派(index / all / static);
- current_pool / pool_at: 指数口径的当前与历史时点成分;
- 成分股同时 upsert 到 quant_stock(拿名称,供 ST 过滤用),不动 is_watch。

任何需要"某天的股票池"的调用方都应走 resolve_pool / pool_at,不要自己
重写 in_date/out_date 条件——重复实现会各自漂移(已修掉两处)。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import delete, func, or_, select
from sqlalchemy.orm import Session

from ..models import DailyBar, IndexMember, Stock
from . import baostock_client
from .ingest import upsert_stock

logger = logging.getLogger(__name__)

# 池类型。index=指数成分(point-in-time);all=全A;static=静态名单
POOL_KINDS = ("index", "all", "static")
DEFAULT_MIN_LIST_DAYS = 60  # 新股上市未满此天数不入池(池属性默认值)
# list_date 缺失比例上限:超过则 kind='all' 拒绝解析而不是返回半个池子。
# 少量新上市/退市票的元数据滞后是常态,故留 5% 余量。
MAX_MISSING_LIST_DATE_RATIO = 0.05

INDEX_NAMES = ("hs300", "zz500")


def sync_index_members(db: Session, index_name: str,
                       today: date | None = None) -> dict:
    """同步一个指数的成分股:新进插入 in_date,调出置 out_date。"""
    today = today or date.today()
    df = baostock_client.fetch_index_members(index_name)
    remote = {r.code: r.name for r in df.itertuples()}
    if not remote:
        # 数据源空响应多半是异常,直接跳过,避免把整个股票池误判为调出
        logger.error("成分股同步 %s: 远端返回空结果,跳过本次同步", index_name)
        return {"index": index_name, "remote": 0,
                "added": 0, "removed": 0, "skipped": True}

    active_rows = db.execute(
        select(IndexMember).where(
            IndexMember.index_name == index_name,
            IndexMember.out_date.is_(None),
        )
    ).scalars().all()
    active = {r.code: r for r in active_rows}

    added = removed = 0
    for code, name in remote.items():
        upsert_stock(db, code, name=name)
        if code not in active:
            db.add(IndexMember(index_name=index_name, code=code, in_date=today))
            added += 1
    for code, row in active.items():
        if code not in remote:
            row.out_date = today
            removed += 1
    db.commit()
    logger.info("成分股同步 %s: 远端 %d,新进 %d,调出 %d",
                index_name, len(remote), added, removed)
    return {"index": index_name, "remote": len(remote),
            "added": added, "removed": removed}


def sync_all_indices(db: Session, today: date | None = None) -> dict:
    """同步全部指数名录"""
    with baostock_client.login_session():
        return {name: sync_index_members(db, name, today) for name in INDEX_NAMES}


def rebuild_index_members(db: Session, index_name: str, start: date,
                          end: date | None = None,
                          step_days: int = 14) -> dict:
    """按历史采样重建一个指数的成分区间(in_date/out_date),覆盖现有记录。

    baostock 支持按日期查询历史时点成分(query_xxx_stocks(date=...));
    从 start 起每 step_days 天采样一次,把连续在册段合并为区间。
    粒度误差 <= step_days 天(在册/调出日期最多偏一个采样间隔)。
    末尾再跑一次增量同步,把最后采样点到今日之间的变动对齐。

    建议在 baostock_client.login_session() 内调用(采样点数 = 跨度/step)。
    """
    end = end or date.today()
    if start >= end:
        raise ValueError(f"start({start}) 必须早于 end({end})")

    days: list[date] = []
    d = start
    while d <= end:
        days.append(d)
        d += timedelta(days=step_days)
    if days[-1] != end:
        days.append(end)

    snapshots: list[tuple[date, dict[str, str]]] = []
    for d in days:
        df = baostock_client.fetch_index_members(index_name, day=d)
        remote = {r.code: r.name for r in df.itertuples()}
        if not remote:
            # 空响应按异常处理,跳过该点(避免把整池误判为调出)
            logger.warning("历史成分 %s %s: 远端空,跳过该采样点", index_name, d)
            continue
        snapshots.append((d, remote))
    if not snapshots:
        raise ValueError(f"{index_name} 历史成分采样全部为空,未重建")

    # 连续在册段 -> (in_date, out_date) 区间;缺采样的段按前一点延续
    intervals: list[dict] = []
    open_since: dict[str, date] = {}
    for day, members in snapshots:
        for code in members:
            if code not in open_since:
                open_since[code] = day
        for code in list(open_since):
            if code not in members:
                intervals.append({"code": code,
                                  "in_date": open_since.pop(code),
                                  "out_date": day})
    for code, in_d in open_since.items():
        intervals.append({"code": code, "in_date": in_d, "out_date": None})

    # 历史股票的名称也补进 quant_stock(供 ST 过滤/展示),不动已有记录
    names: dict[str, str] = {}
    for _, members in snapshots:
        names.update(members)
    existing = {r[0] for r in db.execute(select(Stock.code)).all()}
    for code, name in names.items():
        if code not in existing:
            db.add(Stock(code=code, name=name))

    db.execute(delete(IndexMember).where(IndexMember.index_name == index_name))
    db.execute(
        IndexMember.__table__.insert(),
        [{"index_name": index_name, **iv} for iv in intervals],
    )
    db.commit()
    logger.info("成分重建 %s [%s, %s]: 采样 %d 点,区间 %d 条",
                index_name, start, end, len(snapshots), len(intervals))

    sync = sync_index_members(db, index_name, today=end)
    return {"index": index_name, "samples": len(snapshots),
            "intervals": len(intervals), "sync": sync}


def current_pool(db: Session, index_name: str | None = None) -> list[str]:
    """当前在册股票代码列表(按代码排序)。

    index_name 为 None 时跨全部指数去重;给定时只取该指数。
    """
    q = select(IndexMember.code).where(IndexMember.out_date.is_(None))
    if index_name is not None:
        q = q.where(IndexMember.index_name == index_name)
    return sorted(r[0] for r in db.execute(q.distinct()).all())


def pool_at(db: Session, day: date, index_name: str | None = None) -> list[str]:
    """day 当日在册的股票代码列表(按 in_date/out_date 还原历史成分)。

    用于回测选股,避免用当前成分池回测历史引入幸存者偏差。
    注意:返回的是 day 这一时点的静态快照,回测区间内后续的成分变动不体现。
    index_name 为 None 时跨全部指数去重;给定时只取该指数(单指数口径统一走
    这里,不要在调用方重写 in_date/out_date 条件)。
    """
    q = select(IndexMember.code).where(
        IndexMember.in_date <= day,
        (IndexMember.out_date.is_(None)) | (IndexMember.out_date > day),
    )
    if index_name is not None:
        q = q.where(IndexMember.index_name == index_name)
    return sorted(r[0] for r in db.execute(q.distinct()).all())


def membership_intervals(db: Session, codes: list[str], start: date,
                         end: date) -> list[IndexMember]:
    """返回与区间重叠的成分记录，供动态股票池回测构造逐日可选掩码。"""
    if not codes:
        return []
    return list(db.execute(
        select(IndexMember).where(
            IndexMember.code.in_(codes),
            IndexMember.in_date <= end,
            or_(IndexMember.out_date.is_(None), IndexMember.out_date > start),
        )
    ).scalars().all())


def pool_during(db: Session, start: date, end: date,
                index_name: str | None = None) -> list[str]:
    """返回区间内任一时点在册的股票并集。

    index_name 为 None 时跨全部指数(即沪深300+中证500 口径);
    给定时只取该指数,与 pool_at / current_pool 的参数语义一致。
    """
    q = select(IndexMember.code).where(
        IndexMember.in_date <= end,
        or_(IndexMember.out_date.is_(None), IndexMember.out_date > start),
    )
    if index_name is not None:
        q = q.where(IndexMember.index_name == index_name)
    return sorted(r[0] for r in db.execute(q.distinct()).all())


def _has_listing_columns() -> bool:
    """quant_stock 是否已有 list_date/delist_date/is_st 三列。

    这三列由 agent-migrate 增加。未到位时 kind='all' 只能退化为全表,
    调用方会收到明确告警(见 all_market_pool)。
    """
    return all(hasattr(Stock, name)
               for name in ("list_date", "delist_date", "is_st"))


class IncompleteListingDataError(RuntimeError):
    """`quant_stock.list_date` 回填不完整,`kind='all'` 无法可信解析。"""


def all_market_pool(db: Session, day: date,
                    min_list_days: int = DEFAULT_MIN_LIST_DAYS,
                    max_missing_ratio: float = MAX_MISSING_LIST_DATE_RATIO
                    ) -> list[str]:
    """全A 口径:剔除新股、已退市、ST。

    条件:`list_date <= day - min_list_days`
      AND `(delist_date IS NULL OR delist_date > day)`
      AND `NOT is_st`

    **`list_date` 缺失的票会被静默漏掉**(NULL 不满足 `<=` 比较)。这是
    `kind='all'` 特有的新失败模式:index 口径靠 `allow_current_fallback`
    在缺历史成分时抛错,而全A 任意历史日都"能"解析出一个结果——池子少了
    三成也照样跑完回测,数字全错却没人知道。

    因此这里做**硬护栏**:缺失比例超过 max_missing_ratio 直接抛
    IncompleteListingDataError,不返回半个池子。低于阈值时告警但放行
    (少量新上市/退市票的元数据滞后是常态)。
    """
    if not _has_listing_columns():
        logger.warning(
            "quant_stock 缺 list_date/delist_date/is_st,kind='all' 退化为全表"
            "(未剔除新股/退市/ST);待 schema 就绪后自动生效")
        return [r[0] for r in db.execute(
            select(Stock.code).order_by(Stock.code)).all()]

    cutoff = day - timedelta(days=min_list_days)
    # 统计口径只算「有日线的股票」:数据源不覆盖的品种(如北交所 sh.92xxxx,
    # baostock 既无上市日也无日线)永远补不上 list_date,把它们计入分母会让
    # 护栏永久触发、默认口径永久不可用。它们本来也不该进全A池 —— 无日线
    # 就无法回测。真正要防的是「有日线却缺上市日」,那才是元数据滞后。
    has_bars = select(DailyBar.code).distinct().subquery()
    total = db.execute(
        select(func.count()).select_from(Stock)
        .where(Stock.code.in_(select(has_bars.c.code)))
    ).scalar() or 0
    missing = db.execute(
        select(func.count()).select_from(Stock)
        .where(Stock.list_date.is_(None),
               Stock.code.in_(select(has_bars.c.code)))
    ).scalar() or 0
    if missing and total:
        ratio = missing / total
        if ratio > max_missing_ratio:
            raise IncompleteListingDataError(
                f"{missing}/{total} 只有日线的股票缺 list_date"
                f"(占比 {ratio:.1%},上限 {max_missing_ratio:.0%}),"
                f"kind='all' 会静默漏掉这些票导致回测口径错误。"
                f"请先回填上市日期(ingest.backfill_list_dates),"
                f"或显式传 max_missing_ratio 放宽"
            )
        logger.warning(
            "kind='all' 解析 %s: %d/%d 只有日线的股票缺 list_date(占比 %.1f%%),"
            "这些票会被漏掉,请回填上市日期", day, missing, total, ratio * 100)

    rows = db.execute(
        select(Stock.code).where(
            Stock.list_date.is_not(None),
            Stock.list_date <= cutoff,
            or_(Stock.delist_date.is_(None), Stock.delist_date > day),
            or_(Stock.is_st.is_(None), Stock.is_st.is_(False)),
        ).order_by(Stock.code)
    ).all()
    return [r[0] for r in rows]


def static_pool(db: Session, pool_id: int) -> list[str]:
    """静态名单池:直查 quant_pool_member(无历史,已接受该偏差)。"""
    from ..models import PoolMember  # 延迟导入:该表由 agent-migrate 新增

    return sorted(r[0] for r in db.execute(
        select(PoolMember.code).where(PoolMember.pool_id == pool_id).distinct()
    ).all())


def resolve_pool(db: Session, day: date, *, kind: str = "all",
                 index_name: str | None = None, pool_id: int | None = None,
                 min_list_days: int = DEFAULT_MIN_LIST_DAYS,
                 max_missing_ratio: float = MAX_MISSING_LIST_DATE_RATIO
                 ) -> list[str]:
    """统一池解析入口:按 kind 分派为 day 当日的代码列表。

    | kind    | 规则 |
    |---------|------|
    | index   | quant_index_member 的 in_date <= day < out_date(point-in-time) |
    | all     | 全A 剔除新股/退市/ST(见 all_market_pool) |
    | static  | quant_pool_member 直查(无历史) |

    默认 kind='all'(全A)。index 口径可用 index_name 限定单指数,
    为 None 时取全部指数并集(即原沪深300+中证500 的 pool_at 语义)。
    """
    if kind not in POOL_KINDS:
        raise ValueError(f"未知池类型: {kind},可选: {', '.join(POOL_KINDS)}")
    if kind == "index":
        return pool_at(db, day, index_name=index_name)
    if kind == "static":
        if pool_id is None:
            raise ValueError("kind='static' 必须提供 pool_id")
        return static_pool(db, pool_id)
    return all_market_pool(db, day, min_list_days=min_list_days,
                           max_missing_ratio=max_missing_ratio)


def resolve_pool_during(db: Session, start: date, end: date, *,
                        kind: str = "all", index_name: str | None = None,
                        pool_id: int | None = None,
                        min_list_days: int = DEFAULT_MIN_LIST_DAYS,
                        max_missing_ratio: float = MAX_MISSING_LIST_DATE_RATIO
                        ) -> list[str]:
    """区间并集口径:回测样本需覆盖区间内任一时点入池过的票。

    index 走 pool_during(含期间调出的票,逐日 eligibility 掩码另行处理);
    all 用 end 当日解析(退市票由 delist_date > end 条件保留至退市当日,
    区间内新上市的票按 end 时点判定,不会漏);static 与时点无关。
    """
    if kind == "index":
        return pool_during(db, start, end, index_name=index_name)
    return resolve_pool(db, end, kind=kind, index_name=index_name,
                        pool_id=pool_id, min_list_days=min_list_days,
                        max_missing_ratio=max_missing_ratio)
