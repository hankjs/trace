"""数据入库:历史回填、盘后增量、快照落库、双源对账。"""
from __future__ import annotations

import logging
from datetime import date, datetime, timedelta

import pandas as pd
from sqlalchemy import delete, select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.orm import Session

from ..models import DailyBar, Snapshot, Stock
from . import akshare_client, baostock_client

logger = logging.getLogger(__name__)


def upsert_stock(db: Session, code: str, name: str = "", industry: str = "",
                 is_watch: bool | None = None) -> Stock:
    stock = db.get(Stock, code)
    if stock is None:
        stock = Stock(code=code, name=name, industry=industry,
                      is_watch=bool(is_watch))
        db.add(stock)
    else:
        if name:
            stock.name = name
        if industry:
            stock.industry = industry
        if is_watch is not None:
            stock.is_watch = is_watch
    db.commit()
    return stock


def import_stock_list(db: Session) -> int:
    """从 akshare 导入全市场股票列表(不改动已设为自选的记录)"""
    df = akshare_client.fetch_stock_list()
    existing = {r[0] for r in db.execute(select(Stock.code)).all()}
    n = 0
    for row in df.itertuples():
        if row.code not in existing:
            db.add(Stock(code=row.code, name=row.name))
            n += 1
    db.commit()
    return n


def upsert_bars(db: Session, code: str, df: pd.DataFrame) -> int:
    """日线 upsert(code+date 唯一键)"""
    if df.empty:
        return 0
    rows = [
        {
            "code": code,
            "date": r.date,
            "open": float(r.open),
            "high": float(r.high),
            "low": float(r.low),
            "close": float(r.close),
            "raw_close": None if pd.isna(r.raw_close) else float(r.raw_close),
            "volume": 0.0 if pd.isna(r.volume) else float(r.volume),
            "amount": 0.0 if pd.isna(r.amount) else float(r.amount),
        }
        for r in df.itertuples()
    ]
    stmt = mysql_insert(DailyBar).values(rows)
    stmt = stmt.on_duplicate_key_update(
        open=stmt.inserted.open,
        high=stmt.inserted.high,
        low=stmt.inserted.low,
        close=stmt.inserted.close,
        raw_close=stmt.inserted.raw_close,
        volume=stmt.inserted.volume,
        amount=stmt.inserted.amount,
    )
    db.execute(stmt)
    db.commit()
    return len(rows)


def backfill(db: Session, code: str, start: date | str, end: date | str | None = None) -> int:
    """历史回填:baostock 拉 [start, end] 全量日线并 upsert"""
    end = end or date.today()
    df = baostock_client.fetch_daily_bars(code, start, end)
    n = upsert_bars(db, code, df)
    logger.info("回填 %s [%s, %s]: %d 条", code, start, end, n)
    return n


def ingest_daily(db: Session, code: str, day: date | None = None,
                 reconcile: bool = True) -> dict:
    """盘后日线增量:baostock 拉最近几天的数据 upsert,
    再用 akshare 对账当日收盘(差异记日志告警)。"""
    day = day or date.today()
    start = day - timedelta(days=10)  # 多拉几天覆盖节假日/补漏
    df = baostock_client.fetch_daily_bars(code, start, day)
    n = upsert_bars(db, code, df)

    result: dict = {"code": code, "upserted": n, "reconcile": None}
    if reconcile and not df.empty:
        last = df[df["date"] <= day]
        if not last.empty:
            bs_row = last.iloc[-1]
            try:
                ak_bar = akshare_client.fetch_daily_bar(code, bs_row["date"])
            except Exception as e:  # noqa: BLE001
                logger.warning("akshare 对账查询失败 %s: %s", code, e)
                ak_bar = None
            if ak_bar is not None:
                diff = abs(ak_bar["close"] - float(bs_row["close"]))
                pct = diff / float(bs_row["close"]) * 100 if bs_row["close"] else 0
                rec = {
                    "date": str(bs_row["date"]),
                    "baostock_close": float(bs_row["close"]),
                    "akshare_close": ak_bar["close"],
                    "diff_pct": round(pct, 4),
                }
                result["reconcile"] = rec
                if pct > 1.0:  # 差异超 1% 告警(前复权口径差异属正常,仅提示)
                    logger.warning("日线对账差异 %s %s: %s", code, day, rec)
    return result


def ingest_snapshot(db: Session, codes: list[str] | None = None) -> int:
    """盘中快照落库。codes 为 None 时落自选股。"""
    df = akshare_client.fetch_spot_snapshot()
    if codes:
        df = df[df["code"].isin(codes)]
    else:
        watch = {r[0] for r in db.execute(
            select(Stock.code).where(Stock.is_watch.is_(True))).all()}
        df = df[df["code"].isin(watch)]
    rows = [
        {"code": r.code, "ts": r.ts, "price": float(r.price),
         "pct_chg": None if pd.isna(r.pct_chg) else float(r.pct_chg),
         "volume": None if pd.isna(r.volume) else float(r.volume),
         "amount": None if pd.isna(r.amount) else float(r.amount)}
        for r in df.itertuples()
    ]
    if rows:
        db.execute(Snapshot.__table__.insert(), rows)
        db.commit()
    return len(rows)


def cleanup_snapshots(db: Session, retention_days: int) -> int:
    cutoff = datetime.now() - timedelta(days=retention_days)
    res = db.execute(delete(Snapshot).where(Snapshot.ts < cutoff))
    db.commit()
    return res.rowcount or 0


def load_bars_df(db: Session, code: str, start: date | None = None,
                 end: date | None = None) -> pd.DataFrame:
    """从库里读日线为 DataFrame(按日期升序),供指标/回测用。"""
    q = select(DailyBar).where(DailyBar.code == code).order_by(DailyBar.date)
    if start:
        q = q.where(DailyBar.date >= start)
    if end:
        q = q.where(DailyBar.date <= end)
    rows = db.execute(q).scalars().all()
    return pd.DataFrame(
        [
            {"date": r.date, "open": r.open, "high": r.high, "low": r.low,
             "close": r.close, "raw_close": r.raw_close,
             "volume": r.volume, "amount": r.amount}
            for r in rows
        ]
    )
