"""回测:发起(同步执行)与结果查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import run_backtest
from ..db import get_db
from ..models import BacktestEquity, BacktestRun
from ..strategy.strategies import REGISTRY

router = APIRouter(prefix="/api/backtest", tags=["backtest"])


class BacktestIn(BaseModel):
    strategy: str
    codes: list[str]
    start: date
    end: date
    params: dict = {}
    costs: dict = {}  # 可选覆盖 commission / stamp_tax / slippage


@router.get("/strategies")
def list_strategies():
    return {"strategies": sorted(REGISTRY.keys())}


@router.post("", status_code=201)
def create_backtest(body: BacktestIn, db: Session = Depends(get_db)):
    if body.strategy not in REGISTRY:
        raise HTTPException(400, f"未知策略 {body.strategy},可选: {sorted(REGISTRY)}")
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    try:
        result = run_backtest(db, body.strategy, [c.lower() for c in body.codes],
                              body.start, body.end, body.params, body.costs)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return result


@router.get("/{run_id}")
def get_backtest(run_id: int, db: Session = Depends(get_db)):
    run = db.get(BacktestRun, run_id)
    if run is None:
        raise HTTPException(404, f"回测 {run_id} 不存在")
    equity = db.execute(
        select(BacktestEquity).where(BacktestEquity.run_id == run_id)
        .order_by(BacktestEquity.date)
    ).scalars().all()
    return {
        "run_id": run.id,
        "strategy": run.strategy,
        "params": run.params,
        "codes": run.codes,
        "start": str(run.start),
        "end": str(run.end),
        "metrics": run.metrics,
        "created_at": run.created_at.isoformat(sep=" "),
        "equity": [{"date": str(e.date), "equity": e.equity} for e in equity],
    }
