"""APScheduler 定时任务(3.x 稳定 API)。

调度链(交易日,Asia/Shanghai;非交易日由"当日无数据"兜底自然空跑):
- 16:30  池内+自选 日线增量(池内只走 baostock;自选股另做 akshare 对账)
- 17:00  因子计算(quant_factor_daily)→ 选股池 Top 30(quant_pick)
- 17:05  信号引擎(自选+选股池股票 × 全部单标的策略,含 watch)
- 周五 17:30  批量策略评估(quant_strategy_eval)
- 每月 1 日 09:00  成分股名录同步(quant_index_member)
- 盘中 9:30-15:00 每 30 分钟:akshare 快照落 quant_snapshot(仍只采自选)
"""
from __future__ import annotations

import logging
from datetime import date, datetime, time as dtime

from apscheduler.schedulers.background import BackgroundScheduler
from sqlalchemy import select

from .backtest.evaluate import run_evaluation
from .config import settings
from .data import ingest, universe
from .db import SessionLocal
from .models import Pick, Stock
from .selection.pipeline import run_selection
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


def job_daily_bars() -> None:
    """16:30 盘后:池内日线增量(baostock)+ 自选股增量并对账"""
    if not _is_weekday():
        logger.info("非工作日,跳过盘后任务")
        return
    with SessionLocal() as db:
        pool = universe.current_pool(db)
        watch = _watch_codes()
        watch_set = set(watch)
        logger.info("盘后日线开始: 池内 %d 只,自选 %d 只", len(pool), len(watch))
        for code in pool:
            try:
                # 池内只走 baostock(逐只 akshare 对账太慢);自选才对账
                ingest.ingest_daily(db, code, reconcile=code in watch_set)
            except Exception:  # noqa: BLE001 - 单只失败不影响其他
                logger.exception("盘后日线失败 %s", code)
        for code in watch_set - set(pool):
            try:
                ingest.ingest_daily(db, code, reconcile=True)
            except Exception:  # noqa: BLE001
                logger.exception("盘后日线失败 %s", code)
        try:
            n = ingest.cleanup_snapshots(db, settings.snapshot_retention_days)
            if n:
                logger.info("清理过期快照 %d 条", n)
        except Exception:  # noqa: BLE001
            logger.exception("快照清理失败")


def job_factors_and_selection() -> None:
    """17:00 因子 + 选股"""
    if not _is_weekday():
        return
    with SessionLocal() as db:
        try:
            result = run_selection(db)
            logger.info("选股完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("选股任务失败")


def job_signals() -> None:
    """17:05 信号:自选股 + 当日选股池股票"""
    if not _is_weekday():
        return
    with SessionLocal() as db:
        try:
            picks = [r[0] for r in db.execute(
                select(Pick.code).where(Pick.date == date.today())).all()]
            codes = sorted(set(_watch_codes()) | set(picks))
            result = run_signals(db, codes=codes)
            logger.info("信号计算完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("信号计算失败")


def job_weekly_eval() -> None:
    """周五 17:30 批量策略评估"""
    if not _is_weekday():
        return
    with SessionLocal() as db:
        try:
            result = run_evaluation(db)
            logger.info("批量评估完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("批量评估失败")


def job_sync_index_members() -> None:
    """每月 1 日:成分股名录同步"""
    with SessionLocal() as db:
        try:
            result = universe.sync_all_indices(db)
            logger.info("成分股同步完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("成分股同步失败")


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
        job_daily_bars, "cron",
        day_of_week="mon-fri", hour=16, minute=30,
        id="daily_bars", replace_existing=True,
    )
    scheduler.add_job(
        job_factors_and_selection, "cron",
        day_of_week="mon-fri", hour=17, minute=0,
        id="factors_and_selection", replace_existing=True,
    )
    scheduler.add_job(
        job_signals, "cron",
        day_of_week="mon-fri", hour=17, minute=5,
        id="signals", replace_existing=True,
    )
    scheduler.add_job(
        job_weekly_eval, "cron",
        day_of_week="fri", hour=17, minute=30,
        id="weekly_eval", replace_existing=True,
    )
    scheduler.add_job(
        job_sync_index_members, "cron",
        day=1, hour=9, minute=0,
        id="sync_index_members", replace_existing=True,
    )
    scheduler.add_job(
        job_intraday_snapshot, "cron",
        day_of_week="mon-fri", minute="*/30",
        id="intraday_snapshot", replace_existing=True,
    )
    scheduler.start()
    logger.info("scheduler 已启动: 16:30 日线 -> 17:00 因子+选股 -> 17:05 信号; "
                "周五 17:30 评估;每月 1 日成分股同步;盘中每 30 分钟快照")
    return scheduler


def stop_scheduler() -> None:
    if scheduler.running:
        scheduler.shutdown(wait=False)
