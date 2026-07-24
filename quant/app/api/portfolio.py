"""记账与持仓:手工成交录入、持仓/盈亏查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from ..db import get_db
from ..portfolio import positions as pos_svc
from ..portfolio import trades as trade_svc

router = APIRouter(prefix="/api/portfolio", tags=["portfolio"])


class TradeIn(BaseModel):
    code: str
    trade_date: date
    side: str = Field(..., pattern="^(buy|sell)$")
    price: float = Field(..., gt=0)
    qty: float = Field(..., gt=0)
    fee: float = Field(0.0, ge=0)
    note: str = ""


def _trade_out(t) -> dict:
    return {"id": t.id, "code": t.code, "trade_date": str(t.trade_date),
            "side": t.side, "price": t.price, "qty": t.qty,
            "fee": t.fee, "note": t.note}


@router.get("/trades")
def list_trades(code: str | None = None, db: Session = Depends(get_db)):
    rows = trade_svc.list_trades(db, code)
    return {"count": len(rows), "items": [_trade_out(t) for t in rows]}


@router.post("/trades", status_code=201)
def add_trade(body: TradeIn, db: Session = Depends(get_db)):
    t = trade_svc.add_trade(db, body.code.lower(), body.trade_date, body.side,
                            body.price, body.qty, body.fee, body.note)
    return _trade_out(t)


@router.delete("/trades/{trade_id}")
def delete_trade(trade_id: int, db: Session = Depends(get_db)):
    if not trade_svc.delete_trade(db, trade_id):
        raise HTTPException(404, f"成交记录 {trade_id} 不存在")
    return {"deleted": trade_id}


@router.get("/positions")
def get_positions(db: Session = Depends(get_db)):
    """持仓:均价法成本 + 最新价浮动盈亏 + 汇总"""
    return pos_svc.portfolio_summary(db)
