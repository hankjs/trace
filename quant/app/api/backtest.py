"""回测:发起(同步执行)、参数扫描、批量评估排行、结果查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import run_backtest, run_sweep
from ..backtest.evaluate import leaderboard
from ..auth import require_client, user_id_from_claims
from ..api.pools import (default_pool, get_pool_or_404, pool_ref_out,
                         resolve_pool_codes, resolve_pool_codes_during)
from ..api.strategies import get_strategy_or_404
from ..db import get_db
from ..models import BacktestEquity, BacktestRun, Pool, Stock, Strategy

router = APIRouter(prefix="/api/backtest", tags=["backtest"])


class BacktestIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(default_factory=list, max_length=800)  # 可留空用动态池
    start: date
    end: date
    pool_id: int | None = None  # 组合策略留空 codes 时的股票池,缺省落默认池
    # 临时覆盖策略行的参数(回测页调参),不改策略本身
    params: dict = Field(default_factory=dict)
    costs: dict = Field(default_factory=dict)  # 可选覆盖费用


class SweepIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(max_length=800)
    start: date
    end: date
    param_grid: dict  # {参数名: [候选值]},笛卡尔积逐组回测
    costs: dict = Field(default_factory=dict)


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


def _decorate_result(result: dict, db: Session, pool: Pool | None = None) -> dict:
    # strategy_name / template 由引擎从策略行带出,这里不再补
    codes = result.get("codes")
    if isinstance(codes, list):
        result["stocks"] = _stock_items(db, codes)
    if pool is not None:
        result["pool"] = pool_ref_out(pool)
    return result


# 原 GET /api/backtest/strategies 已删除:它返回的策略列表与 GET /api/strategies
# 完全重复(只少了参数字段)。前端共用一份策略缓存,选择器、参数表单和「另存为」
# 读同一个来源,留着这条路径只会多一个会漂移的真相。


@router.post("/sweep")
def sweep(body: SweepIn, db: Session = Depends(get_db),
          claims: dict = Depends(require_client)):
    """参数扫描:逐组参数批量回测,返回各组 metrics(不落库)"""
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    strategy = get_strategy_or_404(
        db, body.strategy_id, user_id_from_claims(claims))
    try:
        result = run_sweep(db, strategy, [c.lower() for c in body.codes],
                           body.start, body.end, body.param_grid, body.costs)
        return _decorate_result(result, db)
    except ValueError as e:
        raise HTTPException(400, str(e))


@router.get("/leaderboard")
def get_leaderboard(db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    """策略排行:最近一轮批量评估(quant_strategy_eval)汇总。

    只出我可见的策略 —— 评估跑所有用户启用的策略,但别人的策略不该出现在
    我的排行榜里(过滤在 leaderboard() 内)。
    """
    return leaderboard(db, user_id_from_claims(claims))


@router.post("", status_code=201)
def create_backtest(body: BacktestIn, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, body.strategy_id, user_id)
    codes = list(dict.fromkeys(c.lower() for c in body.codes))

    # 组合策略且未显式指定 codes 时按股票池解析成分。
    # pool_id 缺省落系统默认池(全A),与前端 pools.ts 的 defaultPool 同口径。
    pool: Pool | None = None
    if body.pool_id is not None:
        pool = get_pool_or_404(db, body.pool_id, user_id)
    use_pool = strategy.kind == "portfolio" and not codes
    if use_pool:
        if pool is None:
            pool = default_pool(db)
        if pool is None:
            raise HTTPException(400, "系统尚未初始化股票池，请先执行数据库迁移")
        if pool.kind == "index":
            # 指数口径:逐日 eligibility 掩码依赖成分历史,缺回填直接拒绝
            if not resolve_pool_codes(db, pool, body.start):
                raise HTTPException(
                    400, "回测起点缺少历史指数成分，请先运行成分历史回填",
                )
        codes = resolve_pool_codes_during(db, pool, body.start, body.end)
        if not codes:
            raise HTTPException(
                400, f"股票池「{pool.name}」在回测区间内没有成分股",
            )
    if not codes:
        raise HTTPException(400, "codes 不能为空")
    try:
        result = run_backtest(db, strategy, codes,
                              body.start, body.end, body.params, body.costs,
                              # 成分变动的逐日掩码只对指数口径有意义
                              dynamic_universe=use_pool and pool.kind == "index",
                              user_id=user_id,
                              pool_id=pool.id if use_pool else None)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _decorate_result(result, db, pool if use_pool else None)


@router.get("/{run_id}")
def get_backtest(run_id: int, db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    user_id = user_id_from_claims(claims)
    run = db.execute(select(BacktestRun).where(
        BacktestRun.id == run_id,
        BacktestRun.user_id == user_id,
    )).scalar_one_or_none()
    if run is None:
        raise HTTPException(404, f"回测 {run_id} 不存在")
    equity = db.execute(
        select(BacktestEquity).where(BacktestEquity.run_id == run_id)
        .order_by(BacktestEquity.date)
    ).scalars().all()
    # 策略可能已被改名或(停用后)删除。历史回测的 params 是当时的快照,
    # 名字则只能回显当前值;策略已删时留 None,由前端显示为「策略已删除」
    strategy = db.get(Strategy, run.strategy_id)
    result = {
        "run_id": run.id,
        "strategy_id": run.strategy_id,
        "strategy_name": strategy.name if strategy else None,
        "template": strategy.template if strategy else None,
        "params": run.params,
        "codes": run.codes,
        "stocks": _stock_items(db, run.codes),
        "start": str(run.start),
        "end": str(run.end),
        "metrics": run.metrics,
        "created_at": run.created_at.isoformat(sep=" "),
        "equity": [{"date": str(e.date), "equity": e.equity} for e in equity],
    }
    # 回显当时所用的池:按编号查历史回测时前端没有本地选择状态,
    # 幸存者偏差标注只能靠这里带回的 kind 判断。池被删则回显 None。
    pool_id = getattr(run, "pool_id", None)
    if pool_id is not None:
        pool = db.execute(select(Pool).where(Pool.id == pool_id)).scalar_one_or_none()
        result["pool"] = pool_ref_out(pool) if pool is not None else None
    return result
