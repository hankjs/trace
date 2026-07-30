"""记账与持仓:手工成交录入、持仓/盈亏查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field, field_validator
from sqlalchemy.orm import Session

from ..catalog import manual_trade_side_name
from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..models import Stock
from ..portfolio import positions as pos_svc
from ..portfolio import trades as trade_svc
from ..stock_repository import StockRepository

router = APIRouter(prefix="/api/portfolio", tags=["portfolio"])


class TradeIn(BaseModel):
    code: str
    trade_date: date
    side: str = Field(..., pattern="^(buy|sell)$")
    price: float = Field(..., gt=0)
    qty: int = Field(..., gt=0)
    fee: float = Field(0.0, ge=0)
    note: str = ""

    @field_validator("qty", mode="before")
    @classmethod
    def _qty_must_be_positive_integer(cls, value):
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError("qty 必须是正整数")
        if isinstance(value, float) and not value.is_integer():
            raise ValueError("qty 必须是正整数")
        return int(value)


def _trade_out(t, stock: Stock | None = None) -> dict:
    return {
        "id": t.id,
        "code": t.code,
        "name": stock.name if stock else "",
        "industry": stock.industry if stock else "",
        "trade_date": str(t.trade_date),
        "side": t.side,
        "side_name": manual_trade_side_name(t.side),
        "price": t.price,
        "qty": t.qty,
        "fee": t.fee,
        "note": t.note,
    }


@router.get("/trades")
def list_trades(code: str | None = None, db: Session = Depends(get_db),
                claims: dict = Depends(require_client)):
    rows = trade_svc.list_trades(db, user_id_from_claims(claims), code)
    stocks = StockRepository(db).by_codes(t.code for t in rows)
    return {
        "count": len(rows),
        "items": [_trade_out(t, stocks.get(t.code)) for t in rows],
    }


@router.post("/trades", status_code=201)
def add_trade(body: TradeIn, db: Session = Depends(get_db),
              claims: dict = Depends(require_client)):
    code = body.code.strip().lower()
    stock = StockRepository(db).by_codes([code]).get(code)
    if stock is None:
        raise HTTPException(
            422, f"股票代码 {code or '（空）'} 不存在，请先从股票搜索结果中选择",
        )
    try:
        t = trade_svc.add_trade(db, user_id_from_claims(claims), code,
                                body.trade_date, body.side,
                                body.price, body.qty, body.fee, body.note)
    except trade_svc.OversellError as exc:
        raise HTTPException(422, str(exc)) from exc
    return _trade_out(t, stock)


@router.delete("/trades/{trade_id}")
def delete_trade(trade_id: int, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    try:
        deleted = trade_svc.delete_trade(
            db, user_id_from_claims(claims), trade_id)
    except trade_svc.OversellError as exc:
        # 删掉这笔买入会让后续卖出透支,拒绝并提示
        raise HTTPException(422, str(exc)) from exc
    if not deleted:
        raise HTTPException(404, f"成交记录 {trade_id} 不存在")
    return {"deleted": trade_id}


@router.get("/positions")
def get_positions(db: Session = Depends(get_db),
                  claims: dict = Depends(require_client)):
    """持仓:均价法成本 + 最新价浮动盈亏 + 汇总"""
    summary = pos_svc.portfolio_summary(db, user_id_from_claims(claims))
    stocks = StockRepository(db).by_codes(
        item["code"] for item in summary["positions"])
    for item in summary["positions"]:
        stock = stocks.get(item["code"])
        item["name"] = stock.name if stock else ""
        item["industry"] = stock.industry if stock else ""
    return summary
