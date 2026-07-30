"""管理接口:手动触发回填、信号计算、快照、股票列表导入,定时任务查看/触发。

baostock / akshare 均为同步阻塞调用,统一在线程中执行以免卡住事件循环。
"""
from __future__ import annotations

import asyncio
import logging
import threading
from collections.abc import Callable
from datetime import date, datetime
from typing import TypeVar

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.orm import Session

from .. import job_log, scheduler_lock
from ..auth import require_admin
from ..config import settings
from ..backtest.evaluate import run_evaluation
from ..data import fundamentals, ingest, universe
from ..data import calendar as trade_calendar
from ..data.quality import (
    clear_quality_cache,
    data_quality_report,
    refresh_data_quality_cache,
)
from ..db import SessionLocal
from ..scheduler import JOB_DEFS, job_def, scheduler
from ..selection.pipeline import run_selection
from ..strategy.engine import run_signals

router = APIRouter(prefix="/api/admin", tags=["admin"])

logger = logging.getLogger(__name__)

T = TypeVar("T")


async def run_db_job(job: Callable[[Session], T]) -> T:
    """在线程中执行同步数据库任务，并保证独立 Session 及时关闭。"""
    def invoke() -> T:
        with SessionLocal() as db:
            return job(db)

    return await asyncio.to_thread(invoke)


def _invalidate_data_quality_cache(db: Session) -> None:
    """源数据变更后作废旁路缓存;下次读接口会现算写回。不碰源表。"""
    clear_quality_cache(db)


@router.post("/backfill")
async def backfill(code: str = Query(..., description="如 sh.600519"),
                   start: date = Query(None, description="默认取配置 backfill_start"),
                   end: date | None = None,
                   force_rescale: bool = Query(
                       False, description="强制重拉,忽略重锚检查结果")):
    """手动历史回填(baostock,线程池执行)。

    走 safe_backfill:库中已有历史时先校验前复权尺度,尺度错乱会自动
    从最早日期起全量重拉,不会把新尺度 bar 接到旧尺度历史上。
    """
    start = start or date.fromisoformat(settings.backfill_start)

    try:
        def _job(db: Session) -> int:
            n = ingest.safe_backfill(
                db, code.lower(), start, end, force=force_rescale)
            _invalidate_data_quality_cache(db)
            return n

        n = await run_db_job(_job)
    except Exception as e:  # noqa: BLE001
        logger.exception("手动回填失败")
        raise HTTPException(502, "回填失败，请查看服务日志") from e
    return {"code": code.lower(), "start": str(start),
            "end": str(end or date.today()), "bars": n}


@router.post("/run-signals")
async def run_signals_now(date_: date | None = Query(None, alias="date")):
    """手动触发指定日期的信号计算(默认今天;该日无日线数据则无信号)"""
    try:
        return await run_db_job(lambda db: run_signals(db, day=date_))
    except Exception as e:  # noqa: BLE001
        logger.exception("手动信号计算失败")
        raise HTTPException(500, "信号计算失败，请查看服务日志") from e


@router.post("/snapshot")
async def snapshot_now():
    """手动抓取一次自选股盘中快照"""
    try:
        n = await run_db_job(ingest.ingest_snapshot)
    except Exception as e:  # noqa: BLE001
        logger.exception("手动快照抓取失败")
        raise HTTPException(502, "快照抓取失败，请查看服务日志") from e
    return {"snapshots": n}


@router.post("/import-stocks")
async def import_stocks():
    """从 akshare + baostock 同步全市场股票名录(含改名/ST/上市退市标记)"""
    try:
        return await run_db_job(ingest.import_stock_list)
    except Exception as e:  # noqa: BLE001
        logger.exception("手动股票列表导入失败")
        raise HTTPException(502, "股票列表导入失败，请查看服务日志") from e


@router.post("/sync-trade-calendar")
async def sync_trade_calendar_now(
    start: date | None = Query(None, description="默认今年 1 月 1 日"),
    end: date | None = Query(None, description="默认明年 1 月 31 日"),
):
    """手动同步交易日历(baostock query_trade_dates -> quant_trade_calendar)"""
    try:
        return await run_db_job(
            lambda db: trade_calendar.sync_trade_calendar(
                db, start=start, end=end))
    except ValueError as e:
        raise HTTPException(422, str(e)) from e
    except Exception as e:  # noqa: BLE001
        logger.exception("手动交易日历同步失败")
        raise HTTPException(502, "交易日历同步失败，请查看服务日志") from e


@router.post("/backfill-list-dates")
async def backfill_list_dates_now():
    """回填 quant_stock.list_date(全A 池 point-in-time 解析的前置)"""
    try:
        return await run_db_job(ingest.backfill_list_dates)
    except Exception as e:  # noqa: BLE001
        logger.exception("手动 list_date 回填失败")
        raise HTTPException(502, "list_date 回填失败，请查看服务日志") from e


@router.post("/sync-adjust-factors")
async def sync_adjust_factors_now(
    codes: str | None = Query(
        None, description="可选，逗号分隔代码；缺省为全市场"),
    start: date = Query(date(2015, 1, 1), description="因子起始日"),
    sleep_per_code: float = Query(0.0, ge=0, le=2,
                                  description="每只间隔秒数，全市场采集建议 0.2"),
):
    """采集复权因子权威值(quant_adjust_factor)。

    因子按除权日稀疏返回,整轮比日线回填轻得多。用途是给重锚检测一个
    **独立**基准——从 close/raw_close 反推只能反推出库里已有的数据,
    历史若已错乱,反推值会继承错误,拿它当基准是循环论证。
    """
    code_list = ([c.strip() for c in codes.split(",") if c.strip()]
                 if codes else None)

    try:
        def _job(db: Session):
            result = ingest.sync_adjust_factors(
                db, codes=code_list, start=start,
                sleep_per_code=sleep_per_code)
            _invalidate_data_quality_cache(db)
            return result

        return await run_db_job(_job)
    except Exception as exc:  # noqa: BLE001
        logger.exception("手动复权因子采集失败")
        raise HTTPException(502, "复权因子采集失败，请查看服务日志") from exc


@router.post("/sync-fundamentals")
async def sync_fundamentals_now(
    codes: str | None = Query(
        None, description="可选，逗号分隔代码；如 600519,sz.000001",
    ),
    universe_: str = Query("watchlist", alias="universe"),
    max_codes: int = Query(
        30, ge=1, le=30,
        description="逐股手动同步最多 30 只；全市场初始化使用低频批量脚本",
    ),
    include_valuation: bool = True,
    include_financials: bool = True,
    valuation_history: bool = Query(
        False, description="回填东财历史估值；单股数据量较大，默认关闭",
    ),
):
    """手动同步估值与财务指标；单只失败会在 failures 中返回，不中断任务。"""
    explicit_codes = None
    if codes is not None:
        explicit_codes = [code for code in codes.split(",") if code.strip()]

    try:
        def _job(db: Session):
            result = fundamentals.sync_fundamental_universe(
                db,
                universe=universe_,
                codes=explicit_codes,
                max_codes=max_codes,
                include_valuation=include_valuation,
                include_financials=include_financials,
                valuation_history=valuation_history,
            )
            _invalidate_data_quality_cache(db)
            return result

        return await run_db_job(_job)
    except fundamentals.FundamentalSyncInProgressError as exc:
        raise HTTPException(409, str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(422, str(exc)) from exc
    except Exception as exc:  # noqa: BLE001
        logger.exception("手动基本面同步失败")
        raise HTTPException(500, "基本面同步失败，请查看服务日志") from exc


@router.post("/sync-index-members")
async def sync_index_members():
    """手动同步成分股名录(hs300 + zz500)"""
    try:
        return await run_db_job(universe.sync_all_indices)
    except Exception as e:  # noqa: BLE001
        logger.exception("手动成分股同步失败")
        raise HTTPException(502, "成分股同步失败，请查看服务日志") from e


@router.post("/run-selection")
async def run_selection_now(date_: date | None = Query(None, alias="date"),
                            top_n: int = Query(30, ge=1, le=200)):
    """手动触发指定日期的因子计算 + 选股(默认今天;该日无数据则空结果)"""
    try:
        return await run_db_job(
            lambda db: run_selection(db, day=date_, top_n=top_n))
    except Exception as e:  # noqa: BLE001
        logger.exception("手动选股失败")
        raise HTTPException(500, "选股失败，请查看服务日志") from e


@router.post("/run-eval")
async def run_eval_now(date_: date | None = Query(None, alias="date"),
                       period_days: int = 365):
    """手动触发批量策略评估(落 quant_strategy_eval)"""
    try:
        return await run_db_job(
            lambda db: run_evaluation(
                db, day=date_, period_days=period_days))
    except Exception as e:  # noqa: BLE001
        logger.exception("手动批量评估失败")
        raise HTTPException(500, "批量评估失败，请查看服务日志") from e


@router.get("/data-quality")
async def data_quality_now(
    force: bool = Query(
        False,
        description="true=忽略旁路缓存现算并写回;默认读 quant_data_quality_cache",
    ),
):
    """全库数据信任报告(ST/估值/财务/复权因子覆盖)。

    源表只读;结果落旁路表 quant_data_quality_cache。force 时强制重算。
    """
    if force:
        return await run_db_job(lambda db: refresh_data_quality_cache(db))
    return await run_db_job(lambda db: data_quality_report(db))


# ---- 定时任务查看与手动触发 ----
#
# 执行记录落 quant_job_run(见 app/job_log.py):手动触发先写 running 行,
# 后台线程完成后更新;进程崩溃遗留的 running 行在下次触发时收尾为 failed。
# 每 job 一把内存锁防止同一进程内重复触发;执行放后台线程并立即返回,
# 晚间流水线这类长任务不会把 HTTP 请求挂到超时。

_manual_locks: dict[str, threading.Lock] = {
    j["id"]: threading.Lock() for j in JOB_DEFS}


def _run_job_thread(job: dict, run_id: int | None, db_lock) -> None:
    try:
        result = job["func"]()
        job_log.finish_run(run_id, job_log.STATUS_FINISHED,
                           datetime.now(), result=result)
    except Exception as e:  # noqa: BLE001 - job 内部已各自隔离,这里兜底记录
        job_log.finish_run(run_id, job_log.STATUS_FAILED,
                           datetime.now(), error=str(e))
    finally:
        scheduler_lock.release_job_lock(job["id"])
        _manual_locks[job["id"]].release()


def _serialize_job(job: dict, latest: dict[tuple[str, str], dict]) -> dict:
    next_run = None
    if scheduler.running:
        scheduled = scheduler.get_job(job["id"])
        if scheduled is not None and scheduled.next_run_time is not None:
            next_run = scheduled.next_run_time.isoformat()
    return {
        "id": job["id"],
        "name": job["name"],
        "description": job["description"],
        "schedule": job["schedule"],
        "next_run_time": next_run,
        "last_system_run": latest.get((job["id"], job_log.TRIGGER_SYSTEM)),
        "manual_run": latest.get((job["id"], job_log.TRIGGER_MANUAL)),
    }


@router.get("/jobs")
async def list_jobs():
    """定时任务列表:调度信息、下次执行时间与最近一次系统/手动执行。

    scheduler_running=false 表示本进程不负责调度(dev 环境或未抢到
    互斥锁),此时 next_run_time 为空,但手动执行仍然可用。
    """
    latest = await asyncio.to_thread(job_log.latest_runs)
    return {
        "scheduler_running": scheduler.running,
        "jobs": [_serialize_job(j, latest) for j in JOB_DEFS],
    }


@router.get("/jobs/{job_id}/runs")
async def job_runs(job_id: str, limit: int = Query(20, ge=1, le=100)):
    """单个任务的执行历史(新到旧),含系统调度与手动触发。"""
    if job_def(job_id) is None:
        raise HTTPException(404, f"未知任务: {job_id}")
    return await run_db_job(lambda db: job_log.recent_runs(db, job_id, limit))


@router.post("/jobs/{job_id}/run", status_code=202)
async def run_job(job_id: str, claims: dict = Depends(require_admin)):
    """手动触发定时任务(后台线程执行,立即返回)。

    不绕过任务内部守卫:非交易日、盘中时间窗外等场景任务会自行跳过,
    表现为结果中的 skipped 或无输出。轮询 GET /jobs 查看执行状态。
    """
    job = job_def(job_id)
    if job is None:
        raise HTTPException(404, f"未知任务: {job_id}")
    lock = _manual_locks[job_id]
    if not lock.acquire(blocking=False):
        raise HTTPException(409, "该任务已有手动执行进行中")
    try:
        # 多 worker 环境下用 MySQL 会话级锁保证同任务不并发;SQLite 测试环境退化为
        # 进程内 threading.Lock。
        db_lock = scheduler_lock.acquire_job_lock(job_id)
        if db_lock is None:
            lock.release()
            raise HTTPException(409, "该任务已有其它实例执行中")
        job_log.fail_stale_running(job_id)
        run_id = job_log.record_run(
            job_id, job_log.TRIGGER_MANUAL, job_log.STATUS_RUNNING,
            started_at=datetime.now(),
            operator=str(claims.get("username") or "") or None,
        )
        threading.Thread(
            target=_run_job_thread, args=(job, run_id, db_lock), daemon=True).start()
    except HTTPException:
        raise
    except Exception:
        lock.release()
        scheduler_lock.release_job_lock(job_id)
        raise
    return {"status": "started", "job_id": job_id}
