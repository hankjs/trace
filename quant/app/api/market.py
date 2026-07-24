"""行情查询:K线、自选股最新快照/收盘。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..db import get_db
from ..models import DailyBar, Snapshot, Stock

router = APIRouter(prefix="/api/market", tags=["market"])


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
    return {
        "code": code,
        "count": len(rows),
        "bars": [
            {"date": str(r.date), "open": r.open, "high": r.high,
             "low": r.low, "close": r.close, "raw_close": r.raw_close,
             "volume": r.volume, "amount": r.amount}
            for r in rows
        ],
    }


@router.get("/snapshot")
def get_snapshot(db: Session = Depends(get_db)):
    """自选股最新价:优先盘中快照,否则最近收盘"""
    watch = db.execute(
        select(Stock).where(Stock.is_watch.is_(True)).order_by(Stock.code)
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
                "code": s.code, "name": s.name, "source": "snapshot",
                "ts": snap.ts.isoformat(sep=" "),
                "price": snap.price, "pct_chg": snap.pct_chg,
            })
        elif bar is not None:
            items.append({
                "code": s.code, "name": s.name, "source": "close",
                "ts": str(bar.date),
                "price": bar.close, "pct_chg": None,
            })
        else:
            items.append({"code": s.code, "name": s.name, "source": None,
                          "ts": None, "price": None, "pct_chg": None})
    return {"count": len(items), "items": items}
