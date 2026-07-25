"""回测:发起(同步执行)、参数扫描、批量评估排行、结果查询。"""
from __future__ import annotations

from copy import deepcopy
from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import run_backtest, run_sweep
from ..backtest.evaluate import leaderboard
from ..catalog import STRATEGIES, strategy_name
from ..data.universe import current_pool
from ..db import get_db
from ..models import BacktestEquity, BacktestRun, Stock
from ..strategy.strategies import PORTFOLIO_STRATEGIES, REGISTRY, SINGLE_STRATEGIES

router = APIRouter(prefix="/api/backtest", tags=["backtest"])


class BacktestIn(BaseModel):
    strategy: str
    codes: list[str] = []  # 组合策略可留空(默认当前股票池)
    start: date
    end: date
    params: dict = {}
    costs: dict = {}  # 可选覆盖 commission / stamp_tax / slippage


class SweepIn(BaseModel):
    strategy: str
    codes: list[str]
    start: date
    end: date
    param_grid: dict  # {参数名: [候选值]},笛卡尔积逐组回测
    costs: dict = {}


def _stock_items(db: Session, codes: list[str] | None) -> list[dict]:
    codes = codes or []
    if not codes:
        return []
    unique_codes = list(dict.fromkeys(codes))
    rows = db.execute(select(Stock).where(Stock.code.in_(unique_codes))).scalars().all()
    stocks = {row.code: row for row in rows}
    return [
        {
            "code": code,
            "name": stocks[code].name if code in stocks else "",
            "industry": stocks[code].industry if code in stocks else "",
        }
        for code in codes
    ]


def _decorate_result(result: dict, db: Session) -> dict:
    strategy = result.get("strategy")
    if isinstance(strategy, str):
        result["strategy_name"] = strategy_name(strategy)
    codes = result.get("codes")
    if isinstance(codes, list):
        result["stocks"] = _stock_items(db, codes)
    return result


@router.get("/strategies")
def list_strategies():
    return {"strategies": sorted(REGISTRY.keys()),
            "single": sorted(SINGLE_STRATEGIES),
            "portfolio": sorted(PORTFOLIO_STRATEGIES),
            "items": [deepcopy(STRATEGIES[name]) for name in sorted(REGISTRY)]}


@router.post("/sweep")
def sweep(body: SweepIn, db: Session = Depends(get_db)):
    """参数扫描:逐组参数批量回测,返回各组 metrics(不落库)"""
    if body.strategy not in REGISTRY:
        raise HTTPException(400, f"未知策略 {body.strategy},可选: {sorted(REGISTRY)}")
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    try:
        result = run_sweep(db, body.strategy, [c.lower() for c in body.codes],
                           body.start, body.end, body.param_grid, body.costs)
        return _decorate_result(result, db)
    except ValueError as e:
        raise HTTPException(400, str(e))


@router.get("/leaderboard")
def get_leaderboard(db: Session = Depends(get_db)):
    """策略排行:最近一轮批量评估(quant_strategy_eval)汇总"""
    result = leaderboard(db)
    for item in result.get("items", []):
        item["strategy_name"] = strategy_name(item["strategy"])
    return result


@router.post("", status_code=201)
def create_backtest(body: BacktestIn, db: Session = Depends(get_db)):
    if body.strategy not in REGISTRY:
        raise HTTPException(400, f"未知策略 {body.strategy},可选: {sorted(REGISTRY)}")
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    codes = [c.lower() for c in body.codes]
    if body.strategy in PORTFOLIO_STRATEGIES and not codes:
        codes = current_pool(db)
    if not codes:
        raise HTTPException(400, "codes 不能为空")
    try:
        result = run_backtest(db, body.strategy, codes,
                              body.start, body.end, body.params, body.costs)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _decorate_result(result, db)


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
        "strategy_name": strategy_name(run.strategy),
        "params": run.params,
        "codes": run.codes,
        "stocks": _stock_items(db, run.codes),
        "start": str(run.start),
        "end": str(run.end),
        "metrics": run.metrics,
        "created_at": run.created_at.isoformat(sep=" "),
        "equity": [{"date": str(e.date), "equity": e.equity} for e in equity],
    }
