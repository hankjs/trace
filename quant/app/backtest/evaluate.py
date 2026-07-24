"""批量策略评估(每周五跑):全部注册策略 × 默认参数 × 近 1 年。

- 单标的策略:对池内评分 Top 50 批量回测,汇总中位数/均值年化、最大回撤、胜率,
  scope="pool_top50";
- 组合策略:池级回测(target_weights),scope="pool";
- 结果落 quant_strategy_eval;leaderboard() 供排行页查询。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

import numpy as np
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.universe import current_pool
from ..models import FactorDaily, StrategyEval
from ..selection.pipeline import score_row
from ..strategy.strategies import (PORTFOLIO_STRATEGIES, REGISTRY,
                                   SINGLE_STRATEGIES)
from .engine import (DEFAULT_COSTS, _batch_single, _median_or_none,
                     _mean_or_none, run_backtest)
from ..data.ingest import load_bars_df

logger = logging.getLogger(__name__)

EVAL_DAYS = 365
TOP_SAMPLE = 50


def top_scored_codes(db: Session, n: int = TOP_SAMPLE) -> list[str]:
    """按最近一个因子日的评分取 Top N 代码"""
    fdate = db.execute(
        select(FactorDaily.date).order_by(FactorDaily.date.desc()).limit(1)
    ).scalar()
    if fdate is None:
        return current_pool(db)[:n]
    rows = db.execute(
        select(FactorDaily).where(FactorDaily.date == fdate)
    ).scalars().all()
    scored = []
    for r in rows:
        s = score_row({k: getattr(r, k) for k in
                       ("mom20", "mom60", "ma20_slope", "vol_ratio5")})
        if s is not None:
            scored.append((s, r.code))
    scored.sort(key=lambda t: (-t[0], t[1]))
    return [c for _, c in scored[:n]]


def _eval_single(db: Session, strategy: str, codes: list[str],
                 start: date, end: date) -> dict:
    mod = REGISTRY[strategy]
    dfs, positions = {}, {}
    for code in codes:
        df = load_bars_df(db, code, start=start, end=end)
        if len(df) < 60:
            continue
        dfs[code] = df
        positions[code] = mod.positions(df, None)
    if not dfs:
        return {"error": "数据不足"}
    results = _batch_single(dfs, positions, DEFAULT_COSTS)
    per = [r["metrics"] for r in results.values()]
    return {
        "codes": len(dfs),
        "annual_return_median": round(float(np.median(
            [m["annual_return"] for m in per])), 4),
        "annual_return_mean": round(float(np.mean(
            [m["annual_return"] for m in per])), 4),
        "total_return_median": round(float(np.median(
            [m["total_return"] for m in per])), 4),
        "max_drawdown_median": round(float(np.median(
            [m["max_drawdown"] for m in per])), 4),
        "sharpe_median": _median_or_none([m["sharpe"] for m in per]),
        "win_rate_mean": _mean_or_none([m["win_rate"] for m in per]),
        "trade_count": sum(m["trade_count"] for m in per),
        "per_code": {c: r["metrics"] for c, r in results.items()},
    }


def run_evaluation(db: Session, day: date | None = None,
                   period_days: int = EVAL_DAYS) -> dict:
    """跑一轮批量评估并落库 quant_strategy_eval"""
    end = day or date.today()
    start = end - timedelta(days=period_days)
    top_codes = top_scored_codes(db, TOP_SAMPLE)
    pool = current_pool(db)
    logger.info("批量评估 [%s, %s]: 单标的样本 %d 只,组合池 %d 只",
                start, end, len(top_codes), len(pool))

    saved = []
    for name in SINGLE_STRATEGIES:
        try:
            metrics = _eval_single(db, name, top_codes, start, end)
            scope = "pool_top50"
        except Exception as e:  # noqa: BLE001
            logger.exception("评估失败 %s", name)
            metrics, scope = {"error": str(e)}, "pool_top50"
        db.add(StrategyEval(strategy=name, params={}, scope=scope,
                            start=start, end=end, metrics=metrics))
        saved.append({"strategy": name, "scope": scope})
        logger.info("评估 %s: %s", name,
                    {k: v for k, v in metrics.items() if k != "per_code"})

    for name in PORTFOLIO_STRATEGIES:
        try:
            res = run_backtest(db, name, pool, start, end, save=False)
            metrics = res["metrics"]
        except Exception as e:  # noqa: BLE001
            logger.exception("评估失败 %s", name)
            metrics = {"error": str(e)}
        db.add(StrategyEval(strategy=name, params={}, scope="pool",
                            start=start, end=end, metrics=metrics))
        saved.append({"strategy": name, "scope": "pool"})
        logger.info("评估 %s: %s", name, metrics)

    db.commit()
    return {"start": str(start), "end": str(end), "evaluated": saved}


def leaderboard(db: Session, limit: int = 50) -> dict:
    """每个策略最近一轮评估结果,按年化中位数/组合年化排序"""
    latest_run = db.execute(
        select(StrategyEval.run_at).order_by(StrategyEval.run_at.desc()).limit(1)
    ).scalar()
    if latest_run is None:
        return {"run_at": None, "items": []}
    rows = db.execute(
        select(StrategyEval).where(StrategyEval.run_at == latest_run)
    ).scalars().all()

    def sort_key(r: StrategyEval) -> float:
        m = r.metrics or {}
        v = m.get("annual_return_median", m.get("annual_return"))
        return v if isinstance(v, (int, float)) else -9

    items = [
        {
            "strategy": r.strategy,
            "scope": r.scope,
            "start": str(r.start),
            "end": str(r.end),
            "metrics": {k: v for k, v in (r.metrics or {}).items()
                        if k != "per_code"},
            "run_at": r.run_at.isoformat(sep=" "),
        }
        for r in sorted(rows, key=sort_key, reverse=True)[:limit]
    ]
    return {"run_at": latest_run.isoformat(sep=" "), "items": items}
