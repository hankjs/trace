"""股票池:沪深300 + 中证500 成分股名录维护与查询。

- sync_index_members: 从 baostock 同步成分股,增量维护 in_date/out_date;
- current_pool: 当前在册股票代码列表(去重);
- 成分股同时 upsert 到 quant_stock(拿名称,供 ST 过滤用),不动 is_watch。
"""
from __future__ import annotations

import logging
from datetime import date

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import IndexMember
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
