"""信号查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..db import get_db
from ..models import Signal

router = APIRouter(prefix="/api/signals", tags=["signals"])


@router.get("")
def list_signals(date_: date | None = Query(None, alias="date"),
                 code: str | None = None, strategy: str | None = None,
                 side: str | None = None, limit: int = Query(200, le=1000),
                 db: Session = Depends(get_db)):
    q = select(Signal).order_by(Signal.date.desc(), Signal.id.desc()).limit(limit)
    if date_:
        q = q.where(Signal.date == date_)
    if code:
        q = q.where(Signal.code == code)
    if strategy:
        q = q.where(Signal.strategy == strategy)
    if side:
        q = q.where(Signal.side == side)
    rows = db.execute(q).scalars().all()
    return {
        "count": len(rows),
        "items": [
            {"id": r.id, "code": r.code, "date": str(r.date),
             "strategy": r.strategy, "side": r.side,
             "price": r.price, "reason": r.reason}
            for r in rows
        ],
    }
