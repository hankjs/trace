"""管理接口:手动触发回填、信号计算、快照、股票列表导入。

baostock / akshare 均为同步阻塞调用,统一 run_in_executor 避免卡住事件循环。
"""
from __future__ import annotations

import asyncio
from datetime import date

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.orm import Session

from ..config import settings
from ..backtest.evaluate import run_evaluation
from ..data import ingest, universe
from ..db import SessionLocal, get_db
from ..selection.pipeline import run_selection
from ..strategy.engine import run_signals

router = APIRouter(prefix="/api/admin", tags=["admin"])


@router.post("/backfill")
async def backfill(code: str = Query(..., description="如 sh.600519"),
                   start: date = Query(None, description="默认取配置 backfill_start"),
                   end: date | None = None):
    """手动历史回填(baostock,线程池执行)"""
    start = start or date.fromisoformat(settings.backfill_start)

    def _job() -> int:
        with SessionLocal() as db:
            return ingest.backfill(db, code.lower(), start, end)

    loop = asyncio.get_running_loop()
    try:
        n = await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"回填失败: {e}")
    return {"code": code.lower(), "start": str(start),
            "end": str(end or date.today()), "bars": n}


@router.post("/run-signals")
async def run_signals_now(date_: date | None = Query(None, alias="date")):
    """手动触发指定日期的信号计算(默认今天;该日无日线数据则无信号)"""
    def _job() -> dict:
        with SessionLocal() as db:
            return run_signals(db, day=date_)

    loop = asyncio.get_running_loop()
    try:
        return await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"信号计算失败: {e}")


@router.post("/snapshot")
async def snapshot_now():
    """手动抓取一次自选股盘中快照"""
    def _job() -> int:
        with SessionLocal() as db:
            return ingest.ingest_snapshot(db)

    loop = asyncio.get_running_loop()
    try:
        n = await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"快照抓取失败: {e}")
    return {"snapshots": n}


@router.post("/import-stocks")
async def import_stocks():
    """从 akshare 导入全市场股票列表"""
    def _job() -> int:
        with SessionLocal() as db:
            return ingest.import_stock_list(db)

    loop = asyncio.get_running_loop()
    try:
        n = await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"股票列表导入失败: {e}")
    return {"imported": n}


@router.post("/sync-index-members")
async def sync_index_members():
    """手动同步成分股名录(hs300 + zz500)"""
    def _job() -> dict:
        with SessionLocal() as db:
            return universe.sync_all_indices(db)

    loop = asyncio.get_running_loop()
    try:
        return await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(502, f"成分股同步失败: {e}")


@router.post("/run-selection")
async def run_selection_now(date_: date | None = Query(None, alias="date"),
                            top_n: int = 30):
    """手动触发指定日期的因子计算 + 选股(默认今天;该日无数据则空结果)"""
    def _job() -> dict:
        with SessionLocal() as db:
            return run_selection(db, day=date_, top_n=top_n)

    loop = asyncio.get_running_loop()
    try:
        return await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"选股失败: {e}")


@router.post("/run-eval")
async def run_eval_now(date_: date | None = Query(None, alias="date"),
                       period_days: int = 365):
    """手动触发批量策略评估(落 quant_strategy_eval)"""
    def _job() -> dict:
        with SessionLocal() as db:
            return run_evaluation(db, day=date_, period_days=period_days)

    loop = asyncio.get_running_loop()
    try:
        return await loop.run_in_executor(None, _job)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(500, f"批量评估失败: {e}")
