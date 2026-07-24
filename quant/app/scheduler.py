"""APScheduler 定时任务(3.x 稳定 API)。

- 交易日 16:30:baostock 盘后日线增量 + akshare 对账 + 跑策略出信号
- 交易日盘中 9:30-15:00 每 30 分钟:akshare 快照落 quant_snapshot
- 非交易日自动跳过(简单周一~周五判断;若当日无日线数据,盘后任务自然空跑)
"""
from __future__ import annotations

import logging
from datetime import date, datetime, time as dtime

from apscheduler.schedulers.background import BackgroundScheduler
from sqlalchemy import select

from .config import settings
from .data import ingest
from .db import SessionLocal
from .models import Stock
from .strategy.engine import run_signals

logger = logging.getLogger(__name__)

scheduler = BackgroundScheduler(timezone="Asia/Shanghai")


def _is_weekday(d: date | None = None) -> bool:
    d = d or datetime.now().date()
    return d.weekday() < 5  # 周一~周五;法定节假日由"当日无数据"兜底


def _watch_codes() -> list[str]:
    with SessionLocal() as db:
        return [r[0] for r in db.execute(
            select(Stock.code).where(Stock.is_watch.is_(True))).all()]


def job_daily_bars_and_signals() -> None:
    """盘后:自选股日线增量 + 对账 + 信号"""
    if not _is_weekday():
        logger.info("非工作日,跳过盘后任务")
        return
    codes = _watch_codes()
    if not codes:
        logger.info("自选股为空,跳过盘后任务")
        return
    logger.info("盘后任务开始,自选股 %d 只", len(codes))
    with SessionLocal() as db:
        for code in codes:
            try:
                ingest.ingest_daily(db, code)
            except Exception:  # noqa: BLE001 - 单只失败不影响其他
                logger.exception("盘后日线失败 %s", code)
        try:
            result = run_signals(db)
            logger.info("信号计算完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("信号计算失败")
        try:
            n = ingest.cleanup_snapshots(db, settings.snapshot_retention_days)
            if n:
                logger.info("清理过期快照 %d 条", n)
        except Exception:  # noqa: BLE001
            logger.exception("快照清理失败")


def job_intraday_snapshot() -> None:
    """盘中快照:仅在交易日的 9:30-15:00 执行"""
    now = datetime.now()
    if not _is_weekday(now.date()):
        return
    if not (dtime(9, 30) <= now.time() <= dtime(15, 0)):
        return
    try:
        with SessionLocal() as db:
            n = ingest.ingest_snapshot(db)
        logger.info("盘中快照落库 %d 条", n)
    except Exception:  # noqa: BLE001
        logger.exception("盘中快照失败")


def start_scheduler() -> BackgroundScheduler:
    scheduler.add_job(
        job_daily_bars_and_signals, "cron",
        day_of_week="mon-fri", hour=16, minute=30,
        id="daily_bars_and_signals", replace_existing=True,
    )
    scheduler.add_job(
        job_intraday_snapshot, "cron",
        day_of_week="mon-fri", minute="*/30",
        id="intraday_snapshot", replace_existing=True,
    )
    scheduler.start()
    logger.info("scheduler 已启动: 盘后 16:30 日线+信号;盘中每 30 分钟快照")
    return scheduler


def stop_scheduler() -> None:
    if scheduler.running:
        scheduler.shutdown(wait=False)
