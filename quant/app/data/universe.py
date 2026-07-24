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
