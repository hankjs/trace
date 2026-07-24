"""选股相关接口:每日 Top N 候选池、条件筛选器。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from ..db import get_db
from ..models import Pick, Stock
from ..selection.screener import screen

router = APIRouter(prefix="/api/selection", tags=["selection"])


def _prev_pick_date(db: Session, day: date) -> date | None:
    return db.execute(
        select(func.max(Pick.date)).where(Pick.date < day)
    ).scalar()


@router.get("/picks")
def get_picks(date_: date | None = Query(None, alias="date"),
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

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
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
    dropped = sorted(prev_codes - cur_codes)
    return {
        "date": str(day),
        "prev_date": str(prev_day) if prev_day else None,
        "items": items,
        "dropped": [{"code": c, "name": names.get(c, "")} for c in dropped],
    }


@router.get("/screener")
def get_screener(date_: date | None = Query(None, alias="date"),
                 pct_chg_min: float | None = None,
                 pct_chg_max: float | None = None,
                 vol_ratio_min: float | None = None,
                 ma_bull: bool = False,
                 high_dist_max: float | None = None,
                 high_window: int = 60,
                 amount_min: float | None = None,
                 limit: int = Query(100, le=500),
                 db: Session = Depends(get_db)):
    """条件筛选:涨幅区间/量比下限/均线多头/距 N 日新高/成交额下限"""
    return screen(db, day=date_, pct_chg_min=pct_chg_min, pct_chg_max=pct_chg_max,
                  vol_ratio_min=vol_ratio_min, ma_bull=ma_bull,
                  high_dist_max=high_dist_max, high_window=high_window,
                  amount_min=amount_min, limit=limit)
