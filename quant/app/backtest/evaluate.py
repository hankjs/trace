"""批量策略评估(每周五跑):全部**启用的**策略 × 各自参数 × 近 1 年。

- 单标的策略:对池内评分 Top 50 批量回测,汇总中位数/均值年化、最大回撤、胜率,
  scope="pool_top50";
- 组合策略:池级回测(target_weights),scope="pool";
- 结果落 quant_strategy_eval;leaderboard() 供排行页查询。

评估对象是 `quant_strategy` 的行(公共 + 用户自建),不再是代码里的模板列表:
用户存了自己的参数组合,排行榜要能拿它和公共策略比。每行按自己的 params 跑,
`enabled=0` 的行跳过。用户策略的评估结果只有属主能在排行榜看到(见 leaderboard)。
"""
from __future__ import annotations

import logging
import uuid
from datetime import date, timedelta

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.universe import current_pool, pool_at, pool_during
from ..models import FactorDaily, Strategy, StrategyEval
from ..selection.pipeline import score_cross_section
from ..strategy.store import enabled_strategies, visible_to
from ..strategy.runtime import strategy_spec_for
from ..strategy.evidence import candidate_spec_hashes
from ..strategy.spec import strategy_spec_hash
from .engine import _median_or_none, _mean_or_none, run_backtest

logger = logging.getLogger(__name__)

EVAL_DAYS = 365
TOP_SAMPLE = 50


def top_scored_codes(db: Session, n: int = TOP_SAMPLE,
                     as_of: date | None = None) -> list[str]:
    """按 as_of(含)之前最近一个因子日的评分取 Top N 代码。

    as_of 必须取回测起点:用回测结束时的最新因子选股再回看历史是前视偏差。
    选股打分现在跟随当前 active 的 SelectionConfig,保证回测与运行时一致。
    """
    from ..selection.config import load_selection_config

    config = load_selection_config(db)
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
    scores = score_cross_section(
        [{"code": r.code, "values": (r.values or {})} for r in rows],
        weights=config.score_weights or {},
        vol_confirm=config.vol_confirm,
    )
    scored = [(s, code) for code, s in scores.items() if s is not None]
    scored.sort(key=lambda t: (-t[0], t[1]))
    return [c for _, c in scored[:n]]


def _eval_single(db: Session, strategy: Strategy, codes: list[str],
                 start: date, end: date) -> tuple[dict, dict]:
    """单标的批量评估与普通回测共享同一个 StrategySpec 编译入口。

    返回 (聚合指标, 完整回测结果):完整结果里的 validation 报告(基线/OOS/
    否决)与 spec_hash 供证据状态推进复用。
    """
    result = run_backtest(db, strategy, codes, start, end, save=False)
    per_code = result["metrics"].get("per_code", {})
    per = list(per_code.values())
    if not per:
        return {"error": "数据不足"}, result
    return {
        "codes": len(per_code),
        # 短序列指标为 None(engine.MIN_STAT_BARS),聚合统一走 None 容忍的 helper
        "annual_return_median": _median_or_none([m["annual_return"] for m in per]),
        "annual_return_mean": _mean_or_none([m["annual_return"] for m in per]),
        "total_return_median": _median_or_none([m["total_return"] for m in per]),
        "max_drawdown_median": _median_or_none([m["max_drawdown"] for m in per]),
        "sharpe_median": _median_or_none([m["sharpe"] for m in per]),
        "win_rate_mean": _mean_or_none([m["win_rate"] for m in per]),
        "trade_count": sum(m["trade_count"] for m in per),
        "per_code": per_code,
    }, result


def _advance_evidence(db: Session, strategy: Strategy,
                      result: dict | None) -> None:
    """周度评估**不**推进证据状态。

    评估通常 save=False,无持久化 BacktestRun,若推进会造成「幽灵升级」——
    界面显示 oos_passed 却查不到可审计的回测 run。证据只由落库回测推进
    (advance_after_backtest 要求 run_id)。保留本函数为空操作便于调用点统一。
    """
    _ = (db, strategy, result)
    return


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
    for strategy in enabled_strategies(db, kind="single"):
        spec_hash = strategy_spec_hash(strategy_spec_for(strategy))
        result: dict | None = None
        try:
            metrics, result = _eval_single(db, strategy, top_codes, start, end)
        except Exception as e:  # noqa: BLE001 - 单个策略失败不影响本轮其余
            logger.exception("评估失败 %s", strategy.name)
            metrics = {"error": str(e)}
        # params 存实际生效的全量参数,与回测落库同口径:策略行之后被改,
        # 这一轮的评估结果仍能解释是按什么参数跑出来的。
        # validation 报告(基线对比/OOS/否决判定)随 metrics JSON 一并落库。
        if result is not None and result.get("validation"):
            metrics = {**metrics, "validation": result["validation"]}
        db.add(StrategyEval(strategy_id=strategy.id,
                            params=strategy.params or {}, scope="pool_top50",
                            start=start, end=end, metrics=metrics,
                            batch_id=batch_id, spec_hash=spec_hash))
        if "error" not in metrics:
            _advance_evidence(db, strategy, result)
        saved.append({"strategy_id": strategy.id, "strategy": strategy.name,
                      "scope": "pool_top50"})
        logger.info("评估 %s: %s", strategy.name,
                    {k: v for k, v in metrics.items()
                     if k not in {"per_code", "validation"}})

    for strategy in enabled_strategies(db, kind="portfolio"):
        spec_hash = strategy_spec_hash(strategy_spec_for(strategy))
        result = None
        try:
            result = run_backtest(
                db, strategy, pool, start, end, save=False,
                dynamic_universe=True,
            )
            metrics = result["metrics"]
        except Exception as e:  # noqa: BLE001
            logger.exception("评估失败 %s", strategy.name)
            metrics = {"error": str(e)}
        db.add(StrategyEval(strategy_id=strategy.id,
                            params=strategy.params or {}, scope="pool",
                            start=start, end=end, metrics=metrics,
                            batch_id=batch_id, spec_hash=spec_hash))
        if "error" not in metrics:
            _advance_evidence(db, strategy, result)
        saved.append({"strategy_id": strategy.id, "strategy": strategy.name,
                      "scope": "pool"})
        logger.info("评估 %s: %s", strategy.name,
                    {k: v for k, v in metrics.items()
                     if k not in {"per_code", "validation", "evidence"}})

    db.commit()
    return {"start": str(start), "end": str(end), "batch_id": batch_id,
            "evaluated": saved}


def leaderboard(db: Session, user_id: str, limit: int = 200,
                offset: int = 0) -> dict:
    """最近一轮评估结果,按年化中位数/组合年化排序。

    按 batch_id 整批取,不用 run_at 精确相等:同一轮里各行的 run_at 相差数
    分钟(每行落库前要跑完一次回测),精确匹配只能捞回最后一条。

    **按可见性过滤**:评估会跑所有用户启用的策略,但排行榜只能出公共策略和
    我自己的 —— 否则别人的策略名和参数会出现在我的页面上。
    """
    if limit <= 0:
        return {"run_at": None, "batch_id": None, "count": 0, "items": []}
    latest = db.execute(
        select(StrategyEval.batch_id, StrategyEval.run_at)
        .order_by(StrategyEval.run_at.desc()).limit(1)
    ).first()
    if latest is None:
        return {"run_at": None, "batch_id": None, "count": 0, "items": []}
    batch_id, latest_run = latest
    rows = db.execute(
        select(StrategyEval, Strategy)
        .join(Strategy, Strategy.id == StrategyEval.strategy_id)
        .where(
            StrategyEval.batch_id == batch_id,
            visible_to(user_id),
        )
    ).all()
    # 证据状态推进会改变策略 spec_hash(状态是规格的一部分),而评估行记录的是
    # 执行当时的哈希;按身份匹配——同一规则内容在五种状态下的哈希都算数
    # (见 strategy/evidence.py)。
    hash_cache: dict[int, set[str]] = {}

    def identity_hashes(strategy: Strategy) -> set[str]:
        if strategy.id not in hash_cache:
            try:
                hash_cache[strategy.id] = candidate_spec_hashes(
                    strategy_spec_for(strategy),
                )
            except Exception:  # noqa: BLE001 - 规格不可解析时退化为精确匹配
                hash_cache[strategy.id] = {strategy.spec_hash}
        return hash_cache[strategy.id]

    rows = [
        (r, s) for r, s in rows
        if r.spec_hash is not None and r.spec_hash in identity_hashes(s)
    ]

    def sort_key(row) -> float:
        m = row[0].metrics or {}
        v = m.get("annual_return_median", m.get("annual_return"))
        return v if isinstance(v, (int, float)) else -9

    sorted_rows = sorted(rows, key=sort_key, reverse=True)
    total = len(sorted_rows)
    items = [
        {
            "strategy_id": r.strategy_id,
            "strategy": s.name,
            "template": s.template,
            "spec_hash": r.spec_hash,
            "is_system": bool(s.is_system),
            "scope": r.scope,
            "start": str(r.start),
            "end": str(r.end),
            "metrics": {k: v for k, v in (r.metrics or {}).items()
                        if k != "per_code"},
            "run_at": r.run_at.isoformat(sep=" "),
            "batch_id": r.batch_id,
        }
        for r, s in sorted_rows[offset:offset + limit]
    ]
    return {"run_at": latest_run.isoformat(sep=" "), "batch_id": batch_id,
            "count": total, "items": items}
