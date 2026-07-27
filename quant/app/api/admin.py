"""管理接口:手动触发回填、信号计算、快照、股票列表导入。

baostock / akshare 均为同步阻塞调用,统一在线程中执行以免卡住事件循环。
"""
from __future__ import annotations

import asyncio
from collections.abc import Callable
from datetime import date
from typing import TypeVar

from fastapi import APIRouter, HTTPException, Query
from sqlalchemy.orm import Session

from ..config import settings
from ..backtest.evaluate import run_evaluation
from ..data import fundamentals, ingest, universe
from ..data import calendar as trade_calendar
from ..db import SessionLocal
from ..selection.pipeline import run_selection
from ..strategy.engine import run_signals

router = APIRouter(prefix="/api/admin", tags=["admin"])

T = TypeVar("T")


async def run_db_job(job: Callable[[Session], T]) -> T:
    """在线程中执行同步数据库任务，并保证独立 Session 及时关闭。"""
    def invoke() -> T:
        with SessionLocal() as db:
            return job(db)

    return await asyncio.to_thread(invoke)


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
        n = await run_db_job(
            lambda db: ingest.safe_backfill(
                db, code.lower(), start, end, force=force_rescale))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"回填失败: {e}")
    return {"code": code.lower(), "start": str(start),
            "end": str(end or date.today()), "bars": n}


@router.post("/run-signals")
async def run_signals_now(date_: date | None = Query(None, alias="date")):
    """手动触发指定日期的信号计算(默认今天;该日无日线数据则无信号)"""
    try:
        return await run_db_job(lambda db: run_signals(db, day=date_))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"信号计算失败: {e}")


@router.post("/snapshot")
async def snapshot_now():
    """手动抓取一次自选股盘中快照"""
    try:
        n = await run_db_job(ingest.ingest_snapshot)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"快照抓取失败: {e}")
    return {"snapshots": n}


@router.post("/import-stocks")
async def import_stocks():
    """从 akshare + baostock 同步全市场股票名录(含改名/ST/上市退市标记)"""
    try:
        return await run_db_job(ingest.import_stock_list)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"股票列表导入失败: {e}")


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
        raise HTTPException(502, f"交易日历同步失败: {e}")


@router.post("/backfill-list-dates")
async def backfill_list_dates_now():
    """回填 quant_stock.list_date(全A 池 point-in-time 解析的前置)"""
    try:
        return await run_db_job(ingest.backfill_list_dates)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"list_date 回填失败: {e}")


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
        return await run_db_job(
            lambda db: ingest.sync_adjust_factors(
                db, codes=code_list, start=start,
                sleep_per_code=sleep_per_code))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"复权因子采集失败: {e}")


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
        return await run_db_job(
            lambda db: fundamentals.sync_fundamental_universe(
                db,
                universe=universe_,
                codes=explicit_codes,
                max_codes=max_codes,
                include_valuation=include_valuation,
                include_financials=include_financials,
                valuation_history=valuation_history,
            ))
    except fundamentals.FundamentalSyncInProgressError as exc:
        raise HTTPException(409, str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(422, str(exc)) from exc
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(500, f"基本面同步失败: {exc}") from exc


@router.post("/sync-index-members")
async def sync_index_members():
    """手动同步成分股名录(hs300 + zz500)"""
    try:
        return await run_db_job(universe.sync_all_indices)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"成分股同步失败: {e}")


@router.post("/run-selection")
async def run_selection_now(date_: date | None = Query(None, alias="date"),
                            top_n: int = 30):
    """手动触发指定日期的因子计算 + 选股(默认今天;该日无数据则空结果)"""
    try:
        return await run_db_job(
            lambda db: run_selection(db, day=date_, top_n=top_n))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"选股失败: {e}")


@router.post("/run-eval")
async def run_eval_now(date_: date | None = Query(None, alias="date"),
                       period_days: int = 365):
    """手动触发批量策略评估(落 quant_strategy_eval)"""
    try:
        return await run_db_job(
            lambda db: run_evaluation(
                db, day=date_, period_days=period_days))
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"批量评估失败: {e}")
