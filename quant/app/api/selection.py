"""选股相关接口:每日 Top N 候选池、条件筛选器。"""
from __future__ import annotations

from datetime import date as Date
from typing import Any, Literal

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import AliasChoices, BaseModel, Field
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from ..db import get_db
from ..auth import require_client, user_id_from_claims
from ..models import Pick
from ..selection.screener import InvalidFilterError, screen, structured_screen
from ..stock_repository import StockRepository

router = APIRouter(prefix="/api/selection", tags=["selection"])


class FilterCondition(BaseModel):
    id: str | None = None
    field: str
    operator: str
    value: Any | None = None
    value_to: Any | None = Field(
        default=None,
        validation_alias=AliasChoices("value_to", "value2"),
    )
    enabled: bool = True


class FilterGroup(BaseModel):
    id: str | None = None
    logic: Literal["and", "or"] = "and"
    conditions: list[FilterCondition] = Field(default_factory=list)


class StructuredScreenerRequest(BaseModel):
    date: Date | None = None
    logic: Literal["and", "or"] = "and"
    conditions: list[FilterCondition] = Field(default_factory=list)
    groups: list[FilterGroup] = Field(default_factory=list)
    limit: int = Field(default=100, ge=1, le=500)
    # 股票池。缺省落系统默认池(全A),与前端 pools.ts 的 defaultPool 同口径。
    pool_id: int | None = None
    # 自选不是池而是用户关系:把它做成池会引入「自选变化时池成员如何同步」
    # 的新问题,故保留为独立开关。与 pool_id 互斥,置 true 时优先。
    watchlist_only: bool = False


def _prev_pick_date(db: Session, day: Date) -> Date | None:
    return db.execute(
        select(func.max(Pick.date)).where(Pick.date < day)
    ).scalar()


@router.get("/picks")
def get_picks(date_: Date | None = Query(None, alias="date"),
              db: Session = Depends(get_db)):
    """某日 Top N 选股池,标注新进/调出(对比前一有池交易日)"""
    day = date_ or db.execute(select(func.max(Pick.date))).scalar()
    if day is None:
        return {"date": None, "items": []}
    rows = db.execute(
        select(Pick).where(Pick.date == day).order_by(Pick.rank)
    ).scalars().all()

    prev_day = _prev_pick_date(db, day)
    prev_codes: set[str] = set()
    if prev_day:
        prev_codes = {r[0] for r in db.execute(
            select(Pick.code).where(Pick.date == prev_day)).all()}
    cur_codes = {r.code for r in rows}

    dropped = sorted(prev_codes - cur_codes)
    stocks = StockRepository(db).by_codes(
        [row.code for row in rows] + dropped)
    names = {code: stock.name for code, stock in stocks.items()}
    items = [
        {
            "rank": r.rank,
            "code": r.code,
            "name": names.get(r.code, ""),
            "score": r.score,
            "factors": r.factors,
            "change": "new" if r.code not in prev_codes else "keep",
        }
        for r in rows
    ]
    return {
        "date": str(day),
        "prev_date": str(prev_day) if prev_day else None,
        "items": items,
        "dropped": [{"code": c, "name": names.get(c, "")} for c in dropped],
    }


@router.get("/screener")
def get_screener(date_: Date | None = Query(None, alias="date"),
                 pct_chg_min: float | None = None,
                 pct_chg_max: float | None = None,
                 vol_ratio_min: float | None = None,
                 ma_bull: bool = False,
                 high_dist_max: float | None = None,
                 high_window: int = Query(60, ge=1, le=750),
                 amount_min: float | None = None,
                 limit: int = Query(100, ge=1, le=500),
                 db: Session = Depends(get_db)):
    """条件筛选:涨幅区间/量比下限/均线多头/距 N 日新高/成交额下限"""
    return screen(db, day=date_, pct_chg_min=pct_chg_min, pct_chg_max=pct_chg_max,
                  vol_ratio_min=vol_ratio_min, ma_bull=ma_bull,
                  high_dist_max=high_dist_max, high_window=high_window,
                  amount_min=amount_min, limit=limit)


@router.post("/screener")
def post_screener(request: StructuredScreenerRequest,
                  db: Session = Depends(get_db),
                  claims: dict = Depends(require_client)):
    """结构化组合筛选；组内及组间均支持 AND/OR。"""
    try:
        return structured_screen(
            db, request.model_dump(), user_id=user_id_from_claims(claims),
        )
    except InvalidFilterError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
