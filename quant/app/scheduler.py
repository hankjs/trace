"""APScheduler 定时任务(3.x 稳定 API)。

调度链(交易日判断走 quant_trade_calendar,Asia/Shanghai):
- 16:30  晚间流水线(单个串行作业,保证下游用到的数据完整):
  池内+自选 日线增量(池内只走 baostock,单登录会话;自选另做 akshare 对账)
  -> 因子计算(quant_factor_daily)+ 选股池 Top 30(quant_pick)
  -> 信号引擎(自选+选股池股票 × 全部**启用的**单标的策略,含 watch)
  -> 组合策略在系统默认池生成计划调仓/资格变化研究计划
  -> 周五再加批量策略评估(quant_strategy_eval,同样跑全部启用的策略)
  历史拆分的 17:00/17:05/17:30 独立定时,在行情任务超时时会读到
  "部分股票已更新、部分未更新"的数据,故合并为顺序作业。
- 每月 1 日 08:30  交易日历同步(quant_trade_calendar)
- 每月 1 日 09:00  成分股名录同步(quant_index_member)
- 每周六 08:00    全市场名录同步(改名/ST/退市标记)
- 交易日 18:30  全市场估值快照(东财千行小页，通常约 6 次请求)
- 每周六 09:00    全市场最近 5 个报告期财务指标(通常约 40 次请求)
- 盘中 9:30-15:00 每 30 分钟:akshare 快照落 quant_snapshot(仍只采自选)
"""
from __future__ import annotations

import logging
from datetime import date, datetime, time as dtime

from apscheduler.schedulers.background import BackgroundScheduler
from sqlalchemy import select
from sqlalchemy.orm import Session

from .backtest.evaluate import run_evaluation
from .config import settings
from .data import baostock_client, fundamentals, ingest, universe
from .data import calendar as trade_calendar
from .data.clock import SHANGHAI_TZ, now_cst
from .data.quality import refresh_data_quality_cache
from .db import SessionLocal
from .models import Pick, WatchlistItem
from .selection.pipeline import run_selection
from .research_plan.pipeline import run_portfolio_plans
from .strategy.engine import run_signals

logger = logging.getLogger(__name__)

scheduler = BackgroundScheduler(timezone="Asia/Shanghai")

# 每批开一个新 Session:单只 flush 失败只污染本批,rollback 后继续下一批
INGEST_BATCH_SIZE = 50


def _now() -> datetime:
    return now_cst()


def _is_trading_day(d: date | None = None) -> bool:
    """交易日判断:走 quant_trade_calendar(节假日不再被当成交易日)。

    日历缺该日时 `calendar.is_trading_day` 内部降级为工作日判断并告警。
    """
    d = d or _now().date()
    with SessionLocal() as db:
        trade_calendar.ensure_calendar_loaded(db, d)
        return trade_calendar.is_trading_day(db, d)


def _refresh_data_quality() -> None:
    """重算 data-quality 旁路缓存(源表只读,失败不影响主链路)。"""
    with SessionLocal() as db:
        try:
            report = refresh_data_quality_cache(db)
            summary = report.get("summary") or {}
            logger.info(
                "data-quality 缓存已刷新: alert=%s latest_bar=%s",
                summary.get("alert_level"),
                summary.get("latest_bar_date"),
            )
        except Exception:  # noqa: BLE001
            db.rollback()
            logger.exception("data-quality 缓存刷新失败")


def _watch_codes() -> list[str]:
    with SessionLocal() as db:
        return [r[0] for r in db.execute(
            select(WatchlistItem.code).distinct()).all()]


def _ingest_batch(codes: list[str], watch_set: set[str],
                  day: date) -> tuple[int, list[str], list[str]]:
    """按批开 Session 采集,返回 (成功数, 异常失败, 无当日数据)。

    单只失败立即 rollback 并隔离:原实现一个 Session 跑完 800 只,某只 flush
    失败后 Session 进入 PendingRollbackError,后续每只都失败(REVIEW §3.2)。
    """
    succeeded = 0
    failed: list[str] = []
    empty: list[str] = []
    for i in range(0, len(codes), INGEST_BATCH_SIZE):
        batch = codes[i:i + INGEST_BATCH_SIZE]
        with SessionLocal() as db:
            for code in batch:
                try:
                    res = ingest.ingest_daily(
                        db, code, day=day, reconcile=code in watch_set)
                    succeeded += 1
                    if not res.get("has_day_bar"):
                        # 返回空帧/无当日 bar 不算失败,但也不能算成功入库
                        empty.append(code)
                except Exception:  # noqa: BLE001 - 单只失败不影响其他
                    db.rollback()  # 关键:清掉失效事务,后续股票才能继续
                    failed.append(code)
                    logger.exception("盘后日线失败 %s", code)
    return succeeded, failed, empty


# 空帧占比超此比例视为「整体异常」(例如误在非交易日跑、数据源整体故障)
EMPTY_RATIO_ABORT = 0.5


def _ingest_market_day(day: date, codes: list[str]) -> tuple[int, list[str], list[str]]:
    """按日批量采集(开关 bulk_daily_bars 开启时),返回 (成功数, 失败, 无当日数据)。

    一次 Session 即可:批量链路只有 2 次 baostock 调用 + 整行 upsert,
    不存在按 code 循环的 Session 中毒面。因子按日同步(P3)在
    ingest.ingest_market_day 内部完成(同一次批量因子请求,不重复调用)。
    北交所不在批量结果中,不计入 empty(它们走新浪源,见 ingest.sync_bj_market)。
    """
    with SessionLocal() as db:
        try:
            res = ingest.ingest_market_day(
                db, day, codes=set(codes),
                backfill_start=date.fromisoformat(settings.backfill_start))
        except Exception:  # noqa: BLE001
            db.rollback()
            logger.exception("盘后批量日线失败 %s", day)
            return 0, sorted(codes), []
    written = set(res["written_codes"])
    empty = [c for c in codes
             if c not in written and not ingest.is_bj_code(c)]
    return res["codes"], res["failed"], sorted(empty)


def job_daily_bars(day: date | None = None) -> dict:
    """盘后:池内日线增量(baostock 单登录会话)+ 自选股增量并对账

    开关 bulk_daily_bars 开启时改走按日批量链路(ingest.ingest_market_day,
    含因子按日同步);默认关闭,走原有按 code 路径,逻辑不变。
    """
    day = day or _now().date()
    if not _is_trading_day(day):
        logger.info("非交易日,跳过盘后任务")
        return {"skipped": True, "succeeded": 0, "failed": [], "empty": []}
    with SessionLocal() as db:
        pool = universe.current_pool(db)
        watch = _watch_codes()
    watch_set = set(watch)
    codes = sorted(set(pool) | watch_set)
    logger.info("盘后日线开始: 池内 %d 只,自选 %d 只,批量=%s",
                len(pool), len(watch), settings.bulk_daily_bars)
    if settings.bulk_daily_bars:
        succeeded, failed, empty = _ingest_market_day(day, codes)
    else:
        # 全程复用一次 baostock 登录(可重入),Session 则按批切分
        with baostock_client.login_session():
            succeeded, failed, empty = _ingest_batch(codes, watch_set, day)
    with SessionLocal() as db:
        try:
            n = ingest.cleanup_snapshots(db, settings.snapshot_retention_days)
            if n:
                logger.info("清理过期快照 %d 条", n)
        except Exception:  # noqa: BLE001
            db.rollback()
            logger.exception("快照清理失败")
    empty_ratio = len(empty) / len(codes) if codes else 0.0
    if empty_ratio > EMPTY_RATIO_ABORT:
        logger.error("盘后日线: %d/%d 只无 %s 当日 bar(占比 %.0f%%),疑似数据源异常",
                     len(empty), len(codes), day, empty_ratio * 100)
    return {
        "skipped": False,
        "requested": len(codes),
        "succeeded": succeeded,
        "failed": sorted(set(failed)),
        "empty": sorted(set(empty)),
        "empty_ratio": round(empty_ratio, 4),
    }


def job_factors_and_selection() -> dict | None:
    """17:00 因子 + 选股"""
    if not _is_trading_day():
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
    if not _is_trading_day():
        return None
    with SessionLocal() as db:
        try:
            picks = [r[0] for r in db.execute(
                select(Pick.code).where(Pick.date == _now().date())).all()]
            codes = sorted(set(_watch_codes()) | set(picks))
            result = run_signals(db, codes=codes)
            result["portfolio_plans"] = run_portfolio_plans(
                db, day=_now().date())
            logger.info("信号计算完成: %s", result)
            return result
        except Exception:  # noqa: BLE001
            logger.exception("信号计算失败")
            return None


def job_evening_pipeline() -> None:
    """16:30 盘后流水线:日线 -> 因子+选股 -> 信号 ->(周五)批量评估。

    串行执行,下游任务只在数据完整后才开始;子任务各自带交易日判断
    与异常隔离,任一行情更新或下游阶段失败都会中止后续发布。
    """
    day = _now().date()
    if not _is_trading_day(day):
        logger.info("非交易日,跳过盘后流水线")
        return
    bars = job_daily_bars(day)
    if bars["failed"]:
        logger.error(
            "盘后流水线中止: %d 只行情更新失败，不发布部分选股结果: %s",
            len(bars["failed"]), bars["failed"][:20],
        )
        return
    if bars.get("empty_ratio", 0.0) > EMPTY_RATIO_ABORT:
        logger.error(
            "盘后流水线中止: %d 只无当日 bar(占比 %.0f%%),疑似数据源异常",
            len(bars.get("empty", [])), bars["empty_ratio"] * 100,
        )
        return
    # 日线落库后刷新信任摘要(旁路表);后续选股失败也不丢最新覆盖率
    _refresh_data_quality()
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
    if not _is_trading_day():
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


def job_sync_trade_calendar() -> dict | None:
    """每月同步交易日历(baostock query_trade_dates),覆盖今年至明年初。"""
    with SessionLocal() as db:
        try:
            result = trade_calendar.sync_trade_calendar(db)
            logger.info("交易日历同步完成: %s", result)
            return result
        except Exception:  # noqa: BLE001
            logger.exception("交易日历同步失败")
            return None


def job_sync_stock_list() -> dict | None:
    """每周同步全市场名录:维护改名(*ST)、上市与退市标记。

    ST 过滤依赖 quant_stock.name/is_st,不定期刷新会让改名股永远漏过过滤。
    """
    with SessionLocal() as db:
        try:
            result = ingest.import_stock_list(db)
            logger.info("股票名录同步完成: %s", result)
            return result
        except Exception:  # noqa: BLE001
            logger.exception("股票名录同步失败")
            return None


def job_sync_valuations(day: date | None = None) -> dict | None:
    """盘后以全市场分页接口同步估值，避免逐股请求触发限流。"""
    day = day or _now().date()
    if not _is_trading_day(day):
        return None
    with SessionLocal() as db:
        try:
            result = fundamentals.sync_market_valuations(db, day)
            logger.info("估值同步完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("估值同步失败")
            return None
    _refresh_data_quality()
    return result


def job_sync_fundamentals(day: date | None = None) -> dict | None:
    """每周刷新全市场最近报告期，捕获新披露与财报修订。"""
    day = day or _now().date()
    periods = fundamentals.recent_report_periods(day, count=5)
    with SessionLocal() as db:
        try:
            result = fundamentals.sync_market_financials(db, periods)
            logger.info("财务指标同步完成: %s", result)
        except Exception:  # noqa: BLE001
            logger.exception("财务指标同步失败")
            return None
    _refresh_data_quality()
    return result


def job_intraday_snapshot() -> None:
    """盘中快照:仅在交易日的 9:30-15:00 执行"""
    now = _now()
    if not _is_trading_day(now.date()):
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
        job_sync_trade_calendar, "cron",
        day=1, hour=8, minute=30,
        id="sync_trade_calendar", replace_existing=True,
        max_instances=1, coalesce=True,
    )
    scheduler.add_job(
        job_sync_stock_list, "cron",
        day_of_week="sat", hour=8, minute=0,
        id="sync_stock_list", replace_existing=True,
        max_instances=1, coalesce=True,
    )
    scheduler.add_job(
        job_sync_valuations, "cron",
        day_of_week="mon-fri", hour=18, minute=30,
        id="sync_valuations", replace_existing=True,
        max_instances=1, coalesce=True,
    )
    scheduler.add_job(
        job_sync_fundamentals, "cron",
        day_of_week="sat", hour=9, minute=0,
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
                "周五加评估);交易日 18:30 全市场估值同步;每月 1 日交易日历+"
                "成分同步;每周六名录与全市场财务同步;"
                "盘中每 30 分钟快照")
    return scheduler


def stop_scheduler() -> None:
    if scheduler.running:
        scheduler.shutdown(wait=False)
