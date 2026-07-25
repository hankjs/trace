"""APScheduler 定时任务(3.x 稳定 API)。

调度链(交易日,Asia/Shanghai;非交易日由"当日无数据"兜底自然空跑):
- 16:30  晚间流水线(单个串行作业,保证下游用到的数据完整):
  池内+自选 日线增量(池内只走 baostock,单登录会话;自选另做 akshare 对账)
  -> 因子计算(quant_factor_daily)+ 选股池 Top 30(quant_pick)
  -> 信号引擎(自选+选股池股票 × 全部单标的策略,含 watch)
  -> 周五再加批量策略评估(quant_strategy_eval)
  历史拆分的 17:00/17:05/17:30 独立定时,在行情任务超时时会读到
  "部分股票已更新、部分未更新"的数据,故合并为顺序作业。
- 每月 1 日 09:00  成分股名录同步(quant_index_member)
- 交易日 18:30  自选+最近候选估值快照(最多 30 只，独立于主流水线)
- 每月 2 日 19:00  同一范围财务报告同步
- 盘中 9:30-15:00 每 30 分钟:akshare 快照落 quant_snapshot(仍只采自选)
"""
from __future__ import annotations

import logging
from datetime import date, datetime, time as dtime
from zoneinfo import ZoneInfo

from apscheduler.schedulers.background import BackgroundScheduler
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from .backtest.evaluate import run_evaluation
from .config import settings
from .data import baostock_client, fundamentals, ingest, universe
from .db import SessionLocal
from .models import Pick, WatchlistItem
from .selection.pipeline import run_selection
from .strategy.engine import run_signals

logger = logging.getLogger(__name__)

scheduler = BackgroundScheduler(timezone="Asia/Shanghai")
SHANGHAI_TZ = ZoneInfo("Asia/Shanghai")


def _now() -> datetime:
    return datetime.now(SHANGHAI_TZ)


def _is_weekday(d: date | None = None) -> bool:
    d = d or _now().date()
    return d.weekday() < 5  # 周一~周五;法定节假日由"当日无数据"兜底


def _watch_codes() -> list[str]:
    with SessionLocal() as db:
        return [r[0] for r in db.execute(
            select(WatchlistItem.code).distinct()).all()]


def job_daily_bars() -> dict:
    """盘后:池内日线增量(baostock 单登录会话)+ 自选股增量并对账"""
    if not _is_weekday():
        logger.info("非工作日,跳过盘后任务")
        return {"skipped": True, "succeeded": 0, "failed": []}
    with SessionLocal() as db:
        pool = universe.current_pool(db)
        watch = _watch_codes()
        watch_set = set(watch)
        logger.info("盘后日线开始: 池内 %d 只,自选 %d 只", len(pool), len(watch))
        succeeded = 0
        failed: list[str] = []
        # 800 只复用一次 baostock 登录,避免逐只 login/logout 拖慢任务
        with baostock_client.login_session():
            for code in pool:
                try:
                    # 池内只走 baostock(逐只 akshare 对账太慢);自选才对账
                    ingest.ingest_daily(db, code, reconcile=code in watch_set)
                    succeeded += 1
                except Exception:  # noqa: BLE001 - 单只失败不影响其他
                    failed.append(code)
                    logger.exception("盘后日线失败 %s", code)
        for code in watch_set - set(pool):
            try:
                ingest.ingest_daily(db, code, reconcile=True)
                succeeded += 1
            except Exception:  # noqa: BLE001
                failed.append(code)
                logger.exception("盘后日线失败 %s", code)
        try:
            n = ingest.cleanup_snapshots(db, settings.snapshot_retention_days)
            if n:
                logger.info("清理过期快照 %d 条", n)
        except Exception:  # noqa: BLE001
            logger.exception("快照清理失败")
        return {
            "skipped": False,
            "requested": len(set(pool) | watch_set),
            "succeeded": succeeded,
            "failed": sorted(set(failed)),
        }


def job_factors_and_selection() -> dict | None:
    """17:00 因子 + 选股"""
    if not _is_weekday():
        return None
    with SessionLocal() as db:
        try:
            result = run_selection(db)
            logger.info("选股完成: %s", result)
            return result
        except Exception:  # noqa: BLE001
            logger.exception("选股任务失败")
            return None


def job_signals() -> dict | None:
    """17:05 信号:自选股 + 当日选股池股票"""
    if not _is_weekday():
        return None
    with SessionLocal() as db:
        try:
            picks = [r[0] for r in db.execute(
                select(Pick.code).where(Pick.date == _now().date())).all()]
            codes = sorted(set(_watch_codes()) | set(picks))
            result = run_signals(db, codes=codes)
            logger.info("信号计算完成: %s", result)
            return result
        except Exception:  # noqa: BLE001
            logger.exception("信号计算失败")
            return None


def job_evening_pipeline() -> None:
    """16:30 盘后流水线:日线 -> 因子+选股 -> 信号 ->(周五)批量评估。

    串行执行,下游任务只在数据完整后才开始;子任务各自带 weekday 判断
    与异常隔离,任一行情更新或下游阶段失败都会中止后续发布。
    """
    if not _is_weekday():
        logger.info("非工作日,跳过盘后流水线")
        return
    bars = job_daily_bars()
    if bars["failed"]:
        logger.error(
            "盘后流水线中止: %d 只行情更新失败，不发布部分选股结果: %s",
            len(bars["failed"]), bars["failed"][:20],
        )
        return
    if job_factors_and_selection() is None:
        logger.error("盘后流水线中止: 选股阶段失败")
        return
    if job_signals() is None:
        logger.error("盘后流水线中止: 信号阶段失败")
        return
    if _now().weekday() == 4:  # 周五
        job_weekly_eval()


def job_weekly_eval() -> None:
    """批量策略评估(由晚间流水线在周五串行调用,也可手动触发)"""
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


def _fundamental_codes(db: Session) -> list[str]:
    watch = [r[0] for r in db.execute(
        select(WatchlistItem.code).distinct().order_by(WatchlistItem.code)
    ).all()]
    latest_pick_date = db.execute(select(func.max(Pick.date))).scalar()
    picks = []
    if latest_pick_date:
        picks = [r[0] for r in db.execute(
            select(Pick.code).where(Pick.date == latest_pick_date)
            .order_by(Pick.rank)
        ).all()]
    return list(dict.fromkeys([*watch, *picks]))[:30]


def job_sync_valuations() -> None:
    """盘后同步有限研究标的的当前估值，不阻塞 16:30 主流水线。"""
    if not _is_weekday():
        return
    with SessionLocal() as db:
        try:
            result = fundamentals.sync_fundamentals(
                db, _fundamental_codes(db), include_financials=False,
            )
            logger.info("估值同步完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("估值同步失败")


def job_sync_fundamentals() -> None:
    """每月同步有限研究标的的财务报告。"""
    with SessionLocal() as db:
        try:
            result = fundamentals.sync_fundamentals(
                db, _fundamental_codes(db), include_valuation=False,
            )
            logger.info("财务指标同步完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("财务指标同步失败")


def job_intraday_snapshot() -> None:
    """盘中快照:仅在交易日的 9:30-15:00 执行"""
    now = _now()
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
        job_evening_pipeline, "cron",
        day_of_week="mon-fri", hour=16, minute=30,
        id="evening_pipeline", replace_existing=True,
        max_instances=1, coalesce=True, misfire_grace_time=3600,
    )
    scheduler.add_job(
        job_sync_index_members, "cron",
        day=1, hour=9, minute=0,
        id="sync_index_members", replace_existing=True,
    )
    scheduler.add_job(
        job_sync_valuations, "cron",
        day_of_week="mon-fri", hour=18, minute=30,
        id="sync_valuations", replace_existing=True,
        max_instances=1, coalesce=True,
    )
    scheduler.add_job(
        job_sync_fundamentals, "cron",
        day=2, hour=19, minute=0,
        id="sync_fundamentals", replace_existing=True,
        max_instances=1, coalesce=True,
    )
    scheduler.add_job(
        job_intraday_snapshot, "cron",
        day_of_week="mon-fri", minute="*/30",
        id="intraday_snapshot", replace_existing=True,
    )
    scheduler.start()
    logger.info("scheduler 已启动: 16:30 盘后流水线(日线->因子+选股->信号,"
                "周五加评估);交易日 18:30 有限估值同步;每月 1 日成分同步;"
                "每月 2 日有限财务同步;"
                "盘中每 30 分钟快照")
    return scheduler


def stop_scheduler() -> None:
    if scheduler.running:
        scheduler.shutdown(wait=False)
