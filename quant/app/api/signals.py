"""信号查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..catalog import (render_signal_reason, signal_reason_type,
                       signal_side_name, strategy_name)
from ..db import get_db
from ..models import Signal, Stock

router = APIRouter(prefix="/api/signals", tags=["signals"])


@router.get("")
def list_signals(date_: date | None = Query(None, alias="date"),
                 code: str | None = None, strategy: str | None = None,
                 side: str | None = None,
                 limit: int = Query(200, ge=1, le=1000),
                 db: Session = Depends(get_db)):
    q = (select(Signal, Stock)
         .outerjoin(Stock, Stock.code == Signal.code)
         .order_by(Signal.date.desc(), Signal.id.desc()).limit(limit))
    if date_:
        q = q.where(Signal.date == date_)
    if code:
        q = q.where(Signal.code == code)
    if strategy:
        q = q.where(Signal.strategy == strategy)
    if side:
        q = q.where(Signal.side == side)
    rows = db.execute(q).all()
    return {
        "count": len(rows),
        "items": [
            {
                "id": signal.id,
                "code": signal.code,
                "name": stock.name if stock else "",
                "industry": stock.industry if stock else "",
                "date": str(signal.date),
                "strategy": signal.strategy,
                "strategy_name": strategy_name(signal.strategy),
                "side": signal.side,
                "side_name": signal_side_name(signal.side),
                "price": signal.price,
                "reason": signal.reason,
                "reason_type": signal_reason_type(signal.reason),
                "reason_text": render_signal_reason(
                    signal.strategy, signal.side, signal.reason
                ),
            }
            for signal, stock in rows
        ],
    }
