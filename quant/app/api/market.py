"""行情查询:K线、自选股最新快照/收盘。"""
from __future__ import annotations

import re
from datetime import date

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import case, select
from sqlalchemy.orm import Session

from ..data.akshare_client import symbol_to_code
from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..models import DailyBar, Snapshot, Stock, WatchlistItem

router = APIRouter(prefix="/api/market", tags=["market"])

_FULL_CODE_RE = re.compile(r"^(sh|sz|bj)\.(\d{6})$", re.IGNORECASE)
_PARTIAL_CODE_RE = re.compile(r"^(sh|sz|bj)\.(\d{1,5})$", re.IGNORECASE)
_DIGITS_RE = re.compile(r"^\d{1,6}$")


def _stock_out(stock: Stock, is_watch: bool = False) -> dict:
    return {
        "code": stock.code,
        "name": stock.name,
        "industry": stock.industry,
        "is_watch": is_watch,
    }


@router.get("/stocks")
def search_stocks(
    q: str = Query("", max_length=64, description="中文名、六位代码或 sh.600519"),
    limit: int = Query(20, ge=1, le=100),
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """搜索股票基础信息；空查询只返回自选股，不下发全市场列表。"""
    query = q.strip()
    normalized = query.lower()
    user_id = user_id_from_claims(claims)
    watch_codes = set(db.execute(
        select(WatchlistItem.code).where(WatchlistItem.user_id == user_id)
    ).scalars().all())
    stmt = select(Stock)
    watch_order = case((Stock.code.in_(watch_codes), 0), else_=1)

    full_code = _FULL_CODE_RE.fullmatch(normalized)
    partial_code = _PARTIAL_CODE_RE.fullmatch(normalized)
    if not query:
        if not watch_codes:
            return {"query": query, "count": 0, "items": []}
        stmt = stmt.where(Stock.code.in_(watch_codes)).order_by(Stock.code)
    elif full_code:
        code = f"{full_code.group(1).lower()}.{full_code.group(2)}"
        stmt = stmt.where(Stock.code == code).order_by(Stock.code)
    elif _DIGITS_RE.fullmatch(normalized) and len(normalized) == 6:
        code = symbol_to_code(normalized)
        stmt = stmt.where(Stock.code == code).order_by(Stock.code)
    elif partial_code:
        prefix = f"{partial_code.group(1).lower()}.{partial_code.group(2)}"
        stmt = stmt.where(Stock.code.like(f"{prefix}%")).order_by(
            watch_order, Stock.code,
        )
    elif _DIGITS_RE.fullmatch(normalized):
        stmt = stmt.where(Stock.code.like(f"__.{normalized}%")).order_by(
            watch_order, Stock.code,
        )
    else:
        # autoescape 让用户输入的 %/_ 作为普通名称字符处理；值仍通过绑定参数传递。
        stmt = stmt.where(Stock.name.contains(query, autoescape=True)).order_by(
            case(
                (Stock.name == query, 0),
                (Stock.name.startswith(query, autoescape=True), 1),
                else_=2,
            ),
            watch_order,
            Stock.code,
        )

    rows = db.execute(stmt.limit(limit)).scalars().all()
    return {
        "query": query,
        "count": len(rows),
        "items": [_stock_out(stock, stock.code in watch_codes) for stock in rows],
    }


@router.get("/kline")
def get_kline(code: str = Query(..., description="如 sh.600519"),
              start: date | None = None, end: date | None = None,
              db: Session = Depends(get_db)):
    """K线数据(前复权,raw_close 为不复权收盘)"""
    q = (select(DailyBar).where(DailyBar.code == code)
         .order_by(DailyBar.date))
    if start:
        q = q.where(DailyBar.date >= start)
    if end:
        q = q.where(DailyBar.date <= end)
    rows = db.execute(q).scalars().all()
    if not rows:
        raise HTTPException(404, f"无 {code} 的日线数据,请先回填")
    stock = db.get(Stock, code)
    return {
        "code": code,
        "name": stock.name if stock else "",
        "industry": stock.industry if stock else "",
        "count": len(rows),
        "bars": [
            {"date": str(r.date), "open": r.open, "high": r.high,
             "low": r.low, "close": r.close, "raw_close": r.raw_close,
             "volume": r.volume, "amount": r.amount}
            for r in rows
        ],
    }


@router.get("/snapshot")
def get_snapshot(db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    """自选股最新价:优先盘中快照,否则最近收盘"""
    watch = db.execute(
        select(Stock).join(WatchlistItem, WatchlistItem.code == Stock.code)
        .where(WatchlistItem.user_id == user_id_from_claims(claims))
        .order_by(Stock.code)
    ).scalars().all()
    items = []
    for s in watch:
        snap = db.execute(
            select(Snapshot).where(Snapshot.code == s.code)
            .order_by(Snapshot.ts.desc()).limit(1)
        ).scalar_one_or_none()
        bar = db.execute(
            select(DailyBar).where(DailyBar.code == s.code)
            .order_by(DailyBar.date.desc()).limit(1)
        ).scalar_one_or_none()
        if snap is not None and (bar is None or snap.ts.date() >= bar.date):
            items.append({
                "code": s.code, "name": s.name, "industry": s.industry,
                "source": "snapshot",
                "ts": snap.ts.isoformat(sep=" "),
                "price": snap.price, "pct_chg": snap.pct_chg,
            })
        elif bar is not None:
            items.append({
                "code": s.code, "name": s.name, "industry": s.industry,
                "source": "close",
                "ts": str(bar.date),
                "price": bar.close, "pct_chg": None,
            })
        else:
            items.append({
                "code": s.code, "name": s.name, "industry": s.industry,
                "source": None, "ts": None, "price": None, "pct_chg": None,
            })
    return {"count": len(items), "items": items}
