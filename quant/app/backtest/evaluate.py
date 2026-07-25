"""批量策略评估(每周五跑):全部注册策略 × 默认参数 × 近 1 年。

- 单标的策略:对池内评分 Top 50 批量回测,汇总中位数/均值年化、最大回撤、胜率,
  scope="pool_top50";
- 组合策略:池级回测(target_weights),scope="pool";
- 结果落 quant_strategy_eval;leaderboard() 供排行页查询。
"""
from __future__ import annotations

import logging
import uuid
from datetime import date, timedelta

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.universe import current_pool, pool_at, pool_during
from ..models import FactorDaily, StrategyEval
from ..selection.pipeline import score_cross_section
from ..strategy.strategies import (PORTFOLIO_STRATEGIES, REGISTRY,
                                   SINGLE_STRATEGIES)
from .engine import (DEFAULT_COSTS, SINGLE_WARMUP_DAYS, _batch_single,
                     _median_or_none, _mean_or_none, run_backtest)
from ..data.ingest import load_bars_df

logger = logging.getLogger(__name__)

EVAL_DAYS = 365
TOP_SAMPLE = 50


def top_scored_codes(db: Session, n: int = TOP_SAMPLE,
                     as_of: date | None = None) -> list[str]:
    """按 as_of(含)之前最近一个因子日的评分取 Top N 代码。

    as_of 必须取回测起点:用回测结束时的最新因子选股再回看历史是前视偏差。
    """
    q = select(FactorDaily.date)
    if as_of is not None:
        q = q.where(FactorDaily.date <= as_of)
    fdate = db.execute(
        q.order_by(FactorDaily.date.desc()).limit(1)
    ).scalar()
    if fdate is None:
        pool = pool_at(db, as_of) if as_of is not None else current_pool(db)
        return pool[:n]
    rows = db.execute(
        select(FactorDaily).where(FactorDaily.date == fdate)
    ).scalars().all()
    # 截面标准化打分:量纲统一,单因子缺失按截面中位数填充而不丢整只
    scores = score_cross_section([
        {"code": r.code,
         **{k: getattr(r, k) for k in
            ("mom20", "mom60", "ma20_slope", "vol_ratio5")}}
        for r in rows
    ])
    scored = [(s, code) for code, s in scores.items() if s is not None]
    scored.sort(key=lambda t: (-t[0], t[1]))
    return [c for _, c in scored[:n]]


def _eval_single(db: Session, strategy: str, codes: list[str],
                 start: date, end: date) -> dict:
    """单标的策略批量评估。与普通回测同一口径:预热 + 完整序列算信号。"""
    mod = REGISTRY[strategy]
    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    dfs, positions = {}, {}
    for code in codes:
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) == 0 or int((df["date"] >= start).sum()) < 60:
            continue
        dfs[code] = df
        positions[code] = mod.positions(df, None)
    if not dfs:
        return {"error": "数据不足"}
    results = _batch_single(dfs, positions, DEFAULT_COSTS, start)
    per = [r["metrics"] for r in results.values()]
    return {
        "codes": len(dfs),
        # 短序列指标为 None(engine.MIN_STAT_BARS),聚合统一走 None 容忍的 helper
        "annual_return_median": _median_or_none([m["annual_return"] for m in per]),
        "annual_return_mean": _mean_or_none([m["annual_return"] for m in per]),
        "total_return_median": _median_or_none([m["total_return"] for m in per]),
        "max_drawdown_median": _median_or_none([m["max_drawdown"] for m in per]),
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
    # 一轮评估共用一个 batch_id:run_at 是 default=datetime.now,在每个对象
    # **实例化**时求值,而循环每次迭代要跑完整回测(数分钟),十几行的 run_at
    # 会相差数分钟。leaderboard 按 batch_id 查才能整批捞回。
    batch_id = uuid.uuid4().hex
    # 单标的样本取起点时点；组合池使用区间成分并集和逐日 eligibility。
    top_codes = top_scored_codes(db, TOP_SAMPLE, as_of=start)
    pool = pool_during(db, start, end)
    logger.info("批量评估 [%s, %s] batch=%s: 单标的样本 %d 只,组合池 %d 只",
                start, end, batch_id, len(top_codes), len(pool))

    saved = []
    for name in SINGLE_STRATEGIES:
        try:
            metrics = _eval_single(db, name, top_codes, start, end)
            scope = "pool_top50"
        except Exception as e:  # noqa: BLE001
            logger.exception("评估失败 %s", name)
            metrics, scope = {"error": str(e)}, "pool_top50"
        db.add(StrategyEval(strategy=name, params={}, scope=scope,
                            start=start, end=end, metrics=metrics,
                            batch_id=batch_id))
        saved.append({"strategy": name, "scope": scope})
        logger.info("评估 %s: %s", name,
                    {k: v for k, v in metrics.items() if k != "per_code"})

    for name in PORTFOLIO_STRATEGIES:
        try:
            res = run_backtest(
                db, name, pool, start, end, save=False, dynamic_universe=True,
            )
            metrics = res["metrics"]
        except Exception as e:  # noqa: BLE001
            logger.exception("评估失败 %s", name)
            metrics = {"error": str(e)}
        db.add(StrategyEval(strategy=name, params={}, scope="pool",
                            start=start, end=end, metrics=metrics,
                            batch_id=batch_id))
        saved.append({"strategy": name, "scope": "pool"})
        logger.info("评估 %s: %s", name, metrics)

    db.commit()
    return {"start": str(start), "end": str(end), "batch_id": batch_id,
            "evaluated": saved}


def leaderboard(db: Session, limit: int = 50) -> dict:
    """最近一轮评估结果,按年化中位数/组合年化排序。

    按 batch_id 整批取,不用 run_at 精确相等:同一轮里各行的 run_at 相差数
    分钟(每行落库前要跑完一次回测),精确匹配只能捞回最后一条。
    """
    if limit <= 0:
        return {"run_at": None, "batch_id": None, "items": []}
    latest = db.execute(
        select(StrategyEval.batch_id, StrategyEval.run_at)
        .order_by(StrategyEval.run_at.desc()).limit(1)
    ).first()
    if latest is None:
        return {"run_at": None, "batch_id": None, "items": []}
    batch_id, latest_run = latest
    if batch_id is None:
        # 历史数据(batch_id 落地前写入的行)没有批次号,退回按 run_at 取单行
        rows = db.execute(
            select(StrategyEval).where(StrategyEval.run_at == latest_run)
        ).scalars().all()
    else:
        rows = db.execute(
            select(StrategyEval).where(StrategyEval.batch_id == batch_id)
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
            "batch_id": r.batch_id,
        }
        for r in sorted(rows, key=sort_key, reverse=True)[:limit]
    ]
    return {"run_at": latest_run.isoformat(sep=" "), "batch_id": batch_id,
            "items": items}
