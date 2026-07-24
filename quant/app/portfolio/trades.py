"""手工成交记账。"""
from __future__ import annotations

from datetime import date

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import Trade


def add_trade(db: Session, code: str, trade_date: date, side: str,
              price: float, qty: float, fee: float = 0.0,
              note: str = "") -> Trade:
    if side not in ("buy", "sell"):
        raise ValueError("side 必须是 buy 或 sell")
    if price <= 0 or qty <= 0:
        raise ValueError("price / qty 必须为正数")
    if fee < 0:
        raise ValueError("fee 不能为负")
    t = Trade(code=code, trade_date=trade_date, side=side, price=price,
              qty=qty, fee=fee, note=note)
    db.add(t)
    db.commit()
    db.refresh(t)
    return t


def list_trades(db: Session, code: str | None = None) -> list[Trade]:
    q = select(Trade).order_by(Trade.trade_date, Trade.id)
    if code:
        q = q.where(Trade.code == code)
    return list(db.execute(q).scalars().all())


def delete_trade(db: Session, trade_id: int) -> bool:
    t = db.get(Trade, trade_id)
    if t is None:
        return False
    db.delete(t)
    db.commit()
    return True
