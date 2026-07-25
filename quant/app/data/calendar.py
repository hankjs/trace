"""交易日历:baostock query_trade_dates 落 quant_trade_calendar,并提供查询。

替代 `scheduler._is_weekday`。原实现把法定节假日当交易日,靠「当日无数据」
兜底,兜底不干净:节假日会拿节前旧 bar 去和 akshare 对账(`ingest.py`
`last.iloc[-1]`),刷出成批假告警(REVIEW §3.4)。

表结构由 agent-migrate 负责(`quant_trade_calendar`),本模块只写采集与查询;
在其落地前由 `app/data/compat.py` 提供等价映射。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from . import baostock_client
from .clock import today_cst
from .compat import TradeCalendar

logger = logging.getLogger(__name__)

# 库中没有覆盖到目标日期时的兜底判断窗口
_SYNC_LOOKBACK_DAYS = 30


def sync_trade_calendar(db: Session, start: date | None = None,
                        end: date | None = None) -> dict:
    """同步 [start, end] 交易日历。默认同步今年至明年初。

    baostock 返回空结果按异常处理:不写库、不清库,避免把整年误判为休市。
    """
    today = today_cst()
    start = start or date(today.year, 1, 1)
    end = end or date(today.year + 1, 1, 31)
    if start > end:
        raise ValueError(f"start({start}) 不能晚于 end({end})")

    df = baostock_client.fetch_trade_dates(start, end)
    if df.empty:
        logger.error("交易日历同步 [%s, %s]: 远端返回空结果,跳过本次同步", start, end)
        return {"start": str(start), "end": str(end), "days": 0,
                "open_days": 0, "skipped": True}

    existing = {
        row[0]: row[1]
        for row in db.execute(
            select(TradeCalendar.date, TradeCalendar.is_open).where(
                TradeCalendar.date >= start, TradeCalendar.date <= end)
        ).all()
    }
    changed = 0
    for row in df.itertuples():
        if row.date in existing:
            if bool(existing[row.date]) != bool(row.is_open):
                db.execute(
                    TradeCalendar.__table__.update()
                    .where(TradeCalendar.__table__.c.date == row.date)
                    .values(is_open=bool(row.is_open), source="baostock")
                )
                changed += 1
        else:
            db.add(TradeCalendar(date=row.date, is_open=bool(row.is_open),
                                 source="baostock"))
            changed += 1
    db.commit()
    open_days = int(df["is_open"].sum())
    logger.info("交易日历同步 [%s, %s]: %d 天(交易日 %d),变更 %d 行",
                start, end, len(df), open_days, changed)
    return {"start": str(start), "end": str(end), "days": len(df),
            "open_days": open_days, "changed": changed, "skipped": False}


def has_calendar(db: Session, day: date) -> bool:
    """库中是否已有 day 的日历行"""
    return db.execute(
        select(func.count()).select_from(TradeCalendar.__table__)
        .where(TradeCalendar.__table__.c.date == day)
    ).scalar_one() > 0


def is_trading_day(db: Session, day: date | None = None) -> bool:
    """day 是否交易日。

    日历缺失该日时降级为「工作日」判断并告警 —— 宁可多跑一次采集,
    也不要因日历未同步而静默停掉整条 pipeline。
    """
    day = day or today_cst()
    row = db.execute(
        select(TradeCalendar.is_open).where(TradeCalendar.date == day)
    ).scalar()
    if row is None:
        logger.warning("交易日历缺少 %s,降级为工作日判断(请检查日历同步任务)", day)
        return day.weekday() < 5
    return bool(row)


def last_trading_day(db: Session, day: date | None = None) -> date | None:
    """<= day 的最近一个交易日。日历缺失时返回 None 由调用方决定降级。"""
    day = day or today_cst()
    return db.execute(
        select(func.max(TradeCalendar.date)).where(
            TradeCalendar.date <= day, TradeCalendar.is_open.is_(True))
    ).scalar()


def ensure_calendar_loaded(db: Session, day: date | None = None) -> bool:
    """确保 day 附近的日历已入库;缺失时触发一次同步。返回是否可用。"""
    day = day or today_cst()
    if has_calendar(db, day):
        return True
    try:
        sync_trade_calendar(db, start=day - timedelta(days=_SYNC_LOOKBACK_DAYS),
                            end=day + timedelta(days=_SYNC_LOOKBACK_DAYS))
    except Exception:  # noqa: BLE001 - 日历同步失败不应阻断采集
        logger.exception("交易日历按需同步失败 %s", day)
        return False
    return has_calendar(db, day)
