"""自选股管理。"""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.ingest import upsert_stock
from ..db import get_db
from ..models import Stock

router = APIRouter(prefix="/api/watchlist", tags=["watchlist"])


class WatchIn(BaseModel):
    code: str  # 如 sh.600519
    name: str = ""
    industry: str = ""


@router.get("")
def list_watchlist(db: Session = Depends(get_db)):
    rows = db.execute(
        select(Stock).where(Stock.is_watch.is_(True)).order_by(Stock.code)
    ).scalars().all()
    return {
        "count": len(rows),
        "items": [{"code": r.code, "name": r.name, "industry": r.industry}
                  for r in rows],
    }


@router.post("", status_code=201)
def add_watch(body: WatchIn, db: Session = Depends(get_db)):
    code = body.code.strip().lower()
    if "." not in code:
        raise HTTPException(400, "code 格式应为 sh.600519 / sz.000001")
    stock = upsert_stock(db, code, name=body.name, industry=body.industry,
                         is_watch=True)
    return {"code": stock.code, "name": stock.name,
            "industry": stock.industry, "is_watch": stock.is_watch}


@router.delete("/{code}")
def remove_watch(code: str, db: Session = Depends(get_db)):
    stock = db.get(Stock, code.lower())
    if stock is None or not stock.is_watch:
        raise HTTPException(404, f"{code} 不在自选中")
    stock.is_watch = False
    db.commit()
    return {"code": stock.code, "is_watch": False}
