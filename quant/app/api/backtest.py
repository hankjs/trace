"""回测:发起(同步执行)、参数扫描、批量评估排行、结果查询。"""
from __future__ import annotations

import logging
from copy import deepcopy
from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import run_backtest, run_sweep, validate_strategy_params
from ..backtest.evaluate import leaderboard
from ..backtest.validation import evaluate_declared_sweep
from ..auth import require_client, user_id_from_claims
from ..api.pools import (get_pool_or_404, pool_ref_out,
                         resolve_pool_codes, resolve_pool_codes_during)
from ..api.strategies import get_strategy_or_404
from ..db import get_db
from ..catalog import STRATEGY_TEMPLATES
from ..models import BacktestEquity, BacktestRun, Pool, Strategy
from ..stock_repository import StockRepository
from ..strategy.evidence import advance_after_backtest
from ..strategy.runtime import strategy_spec_for
from ..strategy.spec import (
    CapabilityStatus,
    StrategyCapabilityError,
    resolve_capabilities,
    strategy_spec_hash,
)

router = APIRouter(prefix="/api/backtest", tags=["backtest"])
plural_router = APIRouter(prefix="/api/backtests", tags=["backtest"])

logger = logging.getLogger(__name__)


class BacktestIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(default_factory=list, max_length=800)  # 可留空用动态池
    start: date
    end: date
    pool_id: int | None = None  # 组合策略可临时覆盖 spec.universe.pool_id
    # 仅兼容旧客户端；结构化策略应先保存完整规格再运行。
    params: dict = Field(default_factory=dict)
    costs: dict = Field(default_factory=dict)  # 可选覆盖费用


class SweepIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(max_length=800)
    start: date
    end: date
    param_grid: dict = Field(default_factory=dict)  # {"$.受控路径": [候选值]},笛卡尔积逐组回测
    costs: dict = Field(default_factory=dict)
    # true 时忽略 param_grid,按规格 validation.parameter_scans 声明执行扫描
    declared: bool = False


def _decorate_result(result: dict, db: Session, pool: Pool | None = None) -> dict:
    # strategy_name / template 由引擎从策略行带出,这里不再补
    codes = result.get("codes")
    if isinstance(codes, list):
        result["stocks"] = StockRepository(db).items(codes)
    if pool is not None:
        result["pool"] = pool_ref_out(pool)
    return result


# 原 GET /api/backtest/strategies 已删除:它返回的策略列表与 GET /api/strategies
# 完全重复(只少了参数字段)。前端共用一份策略缓存,选择器、参数表单和「另存为」
# 读同一个来源,留着这条路径只会多一个会漂移的真相。


@router.post("/sweep")
def sweep(body: SweepIn, db: Session = Depends(get_db),
          claims: dict = Depends(require_client)):
    """参数扫描:逐组参数批量回测,返回各组 metrics(不落库)。

    declared=true 时按规格 validation.parameter_scans 的声明执行,响应附带
    spec_hash(与回测结果同一关联键)与参数稳定性评估;扫描仍是探索性动作,
    不推进证据状态。
    """
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    strategy = get_strategy_or_404(
        db, body.strategy_id, user_id_from_claims(claims))
    param_grid = body.param_grid
    spec = strategy_spec_for(strategy)
    if body.declared:
        scans = spec.validation.parameter_scans
        if not scans:
            raise HTTPException(400, "该策略规格未声明 parameter_scans")
        param_grid = {scan.path: list(scan.values) for scan in scans}
    try:
        result = run_sweep(db, strategy, [c.lower() for c in body.codes],
                           body.start, body.end, param_grid, body.costs)
        result["strategy_spec_hash"] = strategy_spec_hash(spec)
        result["declared"] = body.declared
        if body.declared:
            result["declared_scans"] = [
                {"path": scan.path, "values": list(scan.values)}
                for scan in spec.validation.parameter_scans
            ]
            result["stability"] = evaluate_declared_sweep(
                spec, result["results"],
            )
        return _decorate_result(result, db)
    except StrategyCapabilityError as exc:
        raise HTTPException(400, {
            "message": str(exc),
            "capability": exc.report.model_dump(mode="json"),
        })
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
@plural_router.post("", status_code=201)
def create_backtest(body: BacktestIn, db: Session = Depends(get_db),
                    claims: dict = Depends(require_client)):
    if body.start >= body.end:
        raise HTTPException(400, "start 必须早于 end")
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, body.strategy_id, user_id)
    # 请求一进入就冻结规格。后续解析股票池和加载行情时即使策略被原地修改，
    # 本次执行也只能使用这个不可变快照。
    if body.params:
        if strategy.template not in STRATEGY_TEMPLATES:
            raise HTTPException(
                400, "结构化策略不支持临时模板参数，请先保存规格后再回测",
            )
        from ..strategy.presets import get_preset_spec

        try:
            effective_params = validate_strategy_params(
                strategy.template,
                {**(strategy.params or {}), **body.params},
            )
            execution_spec = get_preset_spec(
                strategy.template, effective_params,
            ).model_copy(deep=True)
        except ValueError as exc:
            raise HTTPException(400, str(exc))
    else:
        execution_spec = strategy_spec_for(strategy).model_copy(deep=True)
    capability = resolve_capabilities(execution_spec)
    if capability.status != CapabilityStatus.SUPPORTED:
        raise HTTPException(400, {
            "message": "该策略当前不能回测",
            "capability": capability.model_dump(mode="json"),
        })
    codes = list(dict.fromkeys(c.lower() for c in body.codes))

    # 组合策略且未显式指定 codes 时按股票池解析成分。请求可临时选择范围，
    # 未指定时必须使用本次冻结 StrategySpec 中的 pool_id。
    pool: Pool | None = None
    if body.pool_id is not None:
        pool = get_pool_or_404(db, body.pool_id, user_id)
    use_pool = strategy.kind == "portfolio" and not codes
    if use_pool:
        if pool is None:
            pool = get_pool_or_404(
                db, execution_spec.universe.pool_id, user_id,
            )
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
                              dynamic_universe=use_pool,
                              user_id=user_id,
                              pool_id=pool.id if use_pool else None,
                              execution_spec=execution_spec)
    except StrategyCapabilityError as exc:
        raise HTTPException(400, {
            "message": str(exc),
            "capability": exc.report.model_dump(mode="json"),
        })
    except ValueError as e:
        raise HTTPException(400, str(e))
    # 回测完成即按身份哈希推进证据状态(strategy/evidence.py);
    # 推进失败不影响本次结果返回
    transition = None
    try:
        transition = advance_after_backtest(db, strategy, result)
        if transition:
            db.commit()
    except Exception:  # noqa: BLE001
        logger.exception("证据状态推进失败 strategy=%s", strategy.id)
        db.rollback()
    result["evidence_transition"] = transition
    return _decorate_result(result, db, pool if use_pool else None)


@router.get("/{run_id}")
@plural_router.get("/{run_id}")
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
    metrics = run.metrics or {}
    evidence = metrics.get("evidence") if isinstance(metrics.get("evidence"), dict) else {}
    result = {
        "run_id": run.id,
        "strategy_id": run.strategy_id,
        "strategy_name": strategy.name if strategy else None,
        "template": strategy.template if strategy else None,
        "params": run.params,
        "parameter_snapshot": evidence.get("parameter_snapshot", run.params),
        "strategy_spec_snapshot": deepcopy(run.strategy_spec_snapshot),
        "strategy_spec_hash": run.strategy_spec_hash,
        "compiler_version": run.compiler_version,
        "component_versions": deepcopy(run.component_versions),
        "data_fingerprint": run.data_fingerprint,
        "universe_fingerprint": run.universe_fingerprint,
        "cost_fingerprint": run.cost_fingerprint,
        "execution_fingerprint": run.execution_fingerprint,
        "codes": run.codes,
        "stocks": StockRepository(db).items(run.codes),
        "start": str(run.start),
        "end": str(run.end),
        "costs": run.costs or {},
        "fee_assumptions": evidence.get("fee_assumptions", {}),
        "metrics": metrics,
        "evidence": evidence or None,
        "validation": metrics.get("validation"),
        "trade_details": evidence.get("trade_details", []),
        "exit_reason_distribution": evidence.get(
            "exit_reason_distribution", {"by_primary": {}, "all_hits": {}},
        ),
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
