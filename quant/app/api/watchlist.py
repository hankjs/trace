"""自选股管理。"""
from __future__ import annotations

import re

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..models import Stock, WatchlistItem

router = APIRouter(prefix="/api/watchlist", tags=["watchlist"])
_CODE_RE = re.compile(r"^(sh|sz|bj)\.\d{6}$")


class WatchIn(BaseModel):
    code: str  # 如 sh.600519
    name: str = ""
    industry: str = ""


@router.get("")
def list_watchlist(db: Session = Depends(get_db),
                   claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    rows = db.execute(
        select(Stock).join(WatchlistItem, WatchlistItem.code == Stock.code)
        .where(WatchlistItem.user_id == user_id).order_by(Stock.code)
    ).scalars().all()
    return {
        "count": len(rows),
        "items": [{"code": r.code, "name": r.name, "industry": r.industry}
                  for r in rows],
    }


@router.post("", status_code=201)
def add_watch(body: WatchIn, db: Session = Depends(get_db),
              claims: dict = Depends(require_client)):
    code = body.code.strip().lower()
    if not _CODE_RE.fullmatch(code):
        raise HTTPException(400, "code 格式应为 sh.600519 / sz.000001")
    stock = db.get(Stock, code)
    if stock is None:
        raise HTTPException(422, f"股票代码 {code} 不存在，请先同步股票资料")
    user_id = user_id_from_claims(claims)
    if db.get(WatchlistItem, (user_id, code)) is None:
        db.add(WatchlistItem(user_id=user_id, code=code))
        db.commit()
    return {"code": stock.code, "name": stock.name,
            "industry": stock.industry, "is_watch": True}


@router.delete("/{code}")
def remove_watch(code: str, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    item = db.get(WatchlistItem, (user_id, code.lower()))
    if item is None:
        raise HTTPException(404, f"{code} 不在自选中")
    db.delete(item)
    db.commit()
    return {"code": code.lower(), "is_watch": False}
