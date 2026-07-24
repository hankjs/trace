"""条件筛选器:基于 quant_factor_daily + 当日 K 线的实时筛选(不落库)。

条件:当日涨幅区间、量比下限、均线多头(close>ma20>ma60)、
距 N 日新高幅度上限、20 日日均成交额下限。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..models import FactorDaily, Stock

logger = logging.getLogger(__name__)


def _latest_factor_date(db: Session, day: date | None) -> date | None:
    q = select(FactorDaily.date)
    if day:
        q = q.where(FactorDaily.date <= day)
    return db.execute(q.order_by(FactorDaily.date.desc()).limit(1)).scalar()


def screen(db: Session, day: date | None = None,
           pct_chg_min: float | None = None,
           pct_chg_max: float | None = None,
           vol_ratio_min: float | None = None,
           ma_bull: bool = False,
           high_dist_max: float | None = None,
           high_window: int = 60,
           amount_min: float | None = None,
           limit: int = 100) -> dict:
    """条件筛选。返回 {date, total, items:[{code, name, 因子..., pct_chg, high_dist}]}"""
    fdate = _latest_factor_date(db, day)
    if fdate is None:
        return {"date": None, "total": 0, "items": []}

    q = select(FactorDaily).where(FactorDaily.date == fdate)
    if vol_ratio_min is not None:
        q = q.where(FactorDaily.vol_ratio5 >= vol_ratio_min)
    if amount_min is not None:
        q = q.where(FactorDaily.amount_avg20 >= amount_min)
    rows = db.execute(q).scalars().all()

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
    start = fdate - timedelta(days=max(high_window, 60) * 2 + 30)

    items = []
    for r in rows:
        df = load_bars_df(db, r.code, start=start, end=fdate)
        if len(df) < 2 or df["date"].iat[-1] != fdate:
            continue
        close = float(df["close"].iat[-1])
        prev = float(df["close"].iat[-2])
        pct_chg = close / prev - 1 if prev else None
        if pct_chg_min is not None and (pct_chg is None or pct_chg < pct_chg_min):
            continue
        if pct_chg_max is not None and (pct_chg is None or pct_chg > pct_chg_max):
            continue
        ma20 = df["close"].rolling(20).mean().iat[-1]
        ma60 = df["close"].rolling(60).mean().iat[-1]
        if ma_bull and not (close > ma20 > ma60):
            continue
        high_n = float(df["high"].tail(high_window).max())
        high_dist = close / high_n - 1 if high_n else None  # <=0,0 表示创新高
        if high_dist_max is not None and (
                high_dist is None or high_dist < -abs(high_dist_max)):
            continue
        items.append({
            "code": r.code,
            "name": names.get(r.code, ""),
            "close": round(close, 3),
            "pct_chg": None if pct_chg is None else round(pct_chg, 4),
            "high_dist": None if high_dist is None else round(high_dist, 4),
            "mom20": r.mom20, "mom60": r.mom60, "rsi14": r.rsi14,
            "atr_pct": r.atr_pct, "vol_ratio5": r.vol_ratio5,
            "ma20_slope": r.ma20_slope, "amount_avg20": r.amount_avg20,
        })

    items.sort(key=lambda x: (-(x["mom20"] or -9), x["code"]))
    items = items[:limit]
    return {"date": str(fdate), "total": len(items), "items": items}
