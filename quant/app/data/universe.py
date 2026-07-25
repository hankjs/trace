"""股票池:沪深300 + 中证500 成分股名录维护与查询。

- sync_index_members: 从 baostock 同步成分股,增量维护 in_date/out_date;
- current_pool: 当前在册股票代码列表(去重);
- 成分股同时 upsert 到 quant_stock(拿名称,供 ST 过滤用),不动 is_watch。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import delete, or_, select
from sqlalchemy.orm import Session

from ..models import IndexMember, Stock
from . import baostock_client
from .ingest import upsert_stock

logger = logging.getLogger(__name__)

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


def current_pool(db: Session) -> list[str]:
    """当前在册股票代码列表(跨指数去重,按代码排序)"""
    rows = db.execute(
        select(IndexMember.code).where(IndexMember.out_date.is_(None)).distinct()
    ).all()
    return sorted(r[0] for r in rows)


def pool_at(db: Session, day: date) -> list[str]:
    """day 当日在册的股票代码列表(按 in_date/out_date 还原历史成分)。

    用于回测选股,避免用当前成分池回测历史引入幸存者偏差。
    注意:返回的是 day 这一时点的静态快照,回测区间内后续的成分变动不体现。
    """
    rows = db.execute(
        select(IndexMember.code).where(
            IndexMember.in_date <= day,
            (IndexMember.out_date.is_(None)) | (IndexMember.out_date > day),
        ).distinct()
    ).all()
    return sorted(r[0] for r in rows)


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


def pool_during(db: Session, start: date, end: date) -> list[str]:
    """返回区间内任一时点属于沪深300或中证500的股票并集。"""
    rows = db.execute(
        select(IndexMember.code).where(
            IndexMember.in_date <= end,
            or_(IndexMember.out_date.is_(None), IndexMember.out_date > start),
        ).distinct()
    ).all()
    return sorted(r[0] for r in rows)
