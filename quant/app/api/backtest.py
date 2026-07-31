"""回测:发起(默认同步函数调用 / HTTP 异步)、参数扫描、批量评估排行、结果查询。"""
from __future__ import annotations

import logging
from copy import deepcopy
from datetime import date, datetime
from typing import Annotated

from fastapi import (
    APIRouter, BackgroundTasks, Depends, HTTPException, Query,
)
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field, model_validator
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import (
    DEFAULT_COSTS, run_backtest, run_sweep, validate_strategy_params,
)
from ..backtest.evaluate import leaderboard
from ..backtest.jobs import execute_backtest_run, pending_payload
from ..backtest.listing import list_runs
from ..backtest.validation import (
    evaluate_declared_sweep,
    validate_backtest_window,
    validate_sweep_grid,
)
from ..auth import require_client, user_id_from_claims
from ..api.pools import (get_pool_or_404, pool_ref_out,
                         resolve_pool_codes, resolve_pool_codes_during)
from ..api.strategies import get_strategy_or_404
from ..config import settings
from ..db import get_db
from ..catalog import STRATEGY_TEMPLATES
from ..models import BacktestEquity, BacktestRun, Pool, Strategy, Task
from ..stock_repository import StockRepository
from ..strategy.evidence import advance_after_backtest
from ..strategy.multiple_testing import multiplicity_report
from ..strategy.runtime import strategy_spec_for
from ..strategy.spec import (
    CapabilityStatus,
    StrategyCapabilityError,
    resolve_capabilities,
    strategy_spec_hash,
)
from ..strategy.compiler import COMPILER_VERSION, component_versions_for_spec
from ..tasks import (
    TaskConflictError, register_handler, submit_task, task_payload,
    user_active_task,
)

TASK_CONFLICT_MESSAGE = "已有进行中的任务,请等待完成后再提交(可在任务中心查看)"

router = APIRouter(prefix="/api/backtest", tags=["backtest"])
plural_router = APIRouter(prefix="/api/backtests", tags=["backtest"])

logger = logging.getLogger(__name__)


class BacktestIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(default_factory=list, max_length=800)  # 可留空用动态池
    start: date
    end: date
    pool_id: int | None = None  # 组合策略可临时覆盖 spec.universe.pool_id;单标的策略也可用它定义研究范围(与 codes 互斥)
    # 仅兼容旧客户端；结构化策略应先保存完整规格再运行。
    params: dict = Field(default_factory=dict)
    costs: dict = Field(default_factory=dict)  # 可选覆盖费用

    @model_validator(mode="after")
    def _check_window(self) -> "BacktestIn":
        validate_backtest_window(self.start, self.end)
        return self


class SweepIn(BaseModel):
    strategy_id: int
    codes: list[str] = Field(max_length=800)
    start: date
    end: date
    param_grid: dict = Field(default_factory=dict)  # {"$.受控路径": [候选值]},笛卡尔积逐组回测
    costs: dict = Field(default_factory=dict)
    # true 时忽略 param_grid,按规格 validation.parameter_scans 声明执行扫描
    declared: bool = False

    @model_validator(mode="after")
    def _check_window_and_grid(self) -> "SweepIn":
        validate_backtest_window(self.start, self.end)
        if self.param_grid:
            validate_sweep_grid(self.param_grid)
        return self


class SensitivityIn(BaseModel):
    """成本敏感性:同一冻结规格下对滑点倍数扫一组。"""
    strategy_id: int
    codes: list[str] = Field(default_factory=list, max_length=800)
    start: date
    end: date
    pool_id: int | None = None
    costs: dict = Field(default_factory=dict)
    # 相对基准滑点的倍数,如 [0.5, 1.0, 2.0]
    slippage_multipliers: list[float] = Field(
        default_factory=lambda: [0.5, 1.0, 2.0], min_length=1, max_length=8,
    )

    @model_validator(mode="after")
    def _check_window(self) -> "SensitivityIn":
        validate_backtest_window(self.start, self.end)
        return self


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


def _run_sweep_compute(
    db: Session, strategy: Strategy, codes: list[str],
    start: date, end: date, param_grid: dict, costs: dict, declared: bool,
) -> dict:
    """参数扫描计算主体:HTTP 同步路径与任务 worker 共用。

    declared=true 时按规格 validation.parameter_scans 的声明执行,响应附带
    spec_hash(与回测结果同一关联键)与参数稳定性评估;扫描仍是探索性动作,
    不推进证据状态。
    """
    spec = strategy_spec_for(strategy)
    if declared:
        scans = spec.validation.parameter_scans
        if not scans:
            raise ValueError("该策略规格未声明 parameter_scans")
        param_grid = {scan.path: list(scan.values) for scan in scans}
    validate_sweep_grid(param_grid)
    result = run_sweep(db, strategy, [c.lower() for c in codes],
                       start, end, param_grid, costs)
    result["strategy_spec_hash"] = strategy_spec_hash(spec)
    result["declared"] = declared
    result["multiplicity"] = multiplicity_report(result.get("results") or [])
    if declared:
        result["declared_scans"] = [
            {"path": scan.path, "values": list(scan.values)}
            for scan in spec.validation.parameter_scans
        ]
        result["stability"] = evaluate_declared_sweep(
            spec, result["results"],
        )
    return _decorate_result(result, db)


@router.post("/sweep", response_model=None)
def sweep(body: SweepIn, db: Session = Depends(get_db),
          claims: dict = Depends(require_client)):
    """参数扫描:逐组参数批量回测,返回各组 metrics。

    - ``settings.task_async=True``(默认):提交全局任务系统,202 返回任务,
      结果落 task.result(不落 BacktestRun);同一用户已有进行中任务时 409
    - ``settings.task_async=False``(单测默认关):同步执行直接返回结果
    """
    user_id = user_id_from_claims(claims)
    strategy = get_strategy_or_404(db, body.strategy_id, user_id)
    if body.declared:
        # declared 的声明检查放提交时,让这类 400 立即返回而不是事后任务失败
        spec = strategy_spec_for(strategy)
        if not spec.validation.parameter_scans:
            raise HTTPException(400, "该策略规格未声明 parameter_scans")
    if settings.task_async:
        try:
            task = submit_task(
                db,
                user_id=user_id,
                type="sweep",
                title=f"参数扫描 · {strategy.name} · {body.start}~{body.end}",
                params={
                    "strategy_id": strategy.id,
                    "codes": body.codes,
                    "start": body.start.isoformat(),
                    "end": body.end.isoformat(),
                    "param_grid": body.param_grid,
                    "declared": body.declared,
                    "costs": body.costs,
                },
            )
        except TaskConflictError:
            raise HTTPException(409, TASK_CONFLICT_MESSAGE)
        return JSONResponse(status_code=202, content=task_payload(task))
    try:
        return _run_sweep_compute(
            db, strategy, body.codes, body.start, body.end,
            body.param_grid, body.costs, body.declared,
        )
    except StrategyCapabilityError as exc:
        raise HTTPException(400, {
            "message": str(exc),
            "capability": exc.report.model_dump(mode="json"),
        })
    except ValueError as e:
        raise HTTPException(400, str(e))


@router.get("/leaderboard")
def get_leaderboard(
    limit: Annotated[int, Query(ge=1, le=200)] = 200,
    offset: Annotated[int, Query(ge=0)] = 0,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """策略排行:最近一轮批量评估(quant_strategy_eval)汇总。

    只出我可见的策略 —— 评估跑所有用户启用的策略,但别人的策略不该出现在
    我的排行榜里(过滤在 leaderboard() 内)。
    """
    return leaderboard(db, user_id_from_claims(claims), limit=limit, offset=offset)


def _run_sensitivity_compute(
    db: Session, user_id: str, strategy: Strategy, execution_spec,
    codes: list[str], use_pool: bool, pool: Pool | None,
    start: date, end: date, costs_in: dict, multipliers: list[float],
) -> dict:
    """成本敏感性计算主体:HTTP 同步路径与任务 worker 共用。"""
    base_costs = {**DEFAULT_COSTS, **(costs_in or {})}
    base_slip = float(base_costs.get("slippage", DEFAULT_COSTS["slippage"]))
    rows = []
    for mult in multipliers:
        if mult < 0 or mult > 50:
            raise ValueError(f"滑点倍数非法: {mult}")
        costs = {**base_costs, "slippage": base_slip * float(mult)}
        try:
            result = run_backtest(
                db, strategy, codes, start, end,
                params={}, costs=costs, save=False,
                dynamic_universe=use_pool,
                user_id=user_id,
                pool_id=pool.id if use_pool and pool else None,
                execution_spec=execution_spec,
            )
        except (StrategyCapabilityError, ValueError) as exc:
            rows.append({
                "slippage_multiplier": mult,
                "slippage": costs["slippage"],
                "error": str(exc),
            })
            continue
        metrics = result.get("metrics") or {}
        rows.append({
            "slippage_multiplier": mult,
            "slippage": costs["slippage"],
            "metrics": {
                k: metrics.get(k)
                for k in (
                    "total_return", "annual_return", "max_drawdown",
                    "sharpe", "trade_count",
                )
            },
            "execution_attribution": metrics.get("execution_attribution"),
        })
    return {
        "strategy_id": strategy.id,
        "strategy_name": strategy.name,
        "strategy_spec_hash": strategy_spec_hash(execution_spec),
        "base_slippage": base_slip,
        "codes": codes,
        "start": str(start),
        "end": str(end),
        "results": rows,
        "disclaimer": (
            "成本敏感性是探索性模拟,不是冲击成本模型;"
            "仅改变固定比例滑点,不代表真实可成交价格。"
        ),
    }


@router.post("/sensitivity", response_model=None)
def cost_sensitivity(body: SensitivityIn, db: Session = Depends(get_db),
                     claims: dict = Depends(require_client)):
    """成本敏感性:同一冻结规格,对滑点倍数跑多组(不落库、不推进证据)。

    - ``settings.task_async=True``(默认):提交全局任务系统,202 返回任务,
      结果落 task.result;同一用户已有进行中任务时 409
    - ``settings.task_async=False``(单测默认关):同步执行直接返回结果
    """
    user_id = user_id_from_claims(claims)
    strategy, execution_spec, codes, use_pool, pool = _prepare_backtest(
        BacktestIn(
            strategy_id=body.strategy_id,
            codes=body.codes,
            start=body.start,
            end=body.end,
            pool_id=body.pool_id,
            costs=body.costs,
        ),
        db, user_id,
    )
    for mult in body.slippage_multipliers:
        # 提交时校验,让这类 400 立即返回而不是事后任务失败
        if mult < 0 or mult > 50:
            raise HTTPException(400, f"滑点倍数非法: {mult}")
    if settings.task_async:
        try:
            task = submit_task(
                db,
                user_id=user_id,
                type="sensitivity",
                title=f"成本敏感性 · {strategy.name} · {body.start}~{body.end}",
                params={
                    "strategy_id": strategy.id,
                    "codes": body.codes,
                    "start": body.start.isoformat(),
                    "end": body.end.isoformat(),
                    "pool_id": body.pool_id,
                    "costs": body.costs,
                    "slippage_multipliers": body.slippage_multipliers,
                },
            )
        except TaskConflictError:
            raise HTTPException(409, TASK_CONFLICT_MESSAGE)
        return JSONResponse(status_code=202, content=task_payload(task))
    try:
        return _run_sensitivity_compute(
            db, user_id, strategy, execution_spec, codes, use_pool, pool,
            body.start, body.end, body.costs, body.slippage_multipliers,
        )
    except ValueError as e:
        raise HTTPException(400, str(e))


def _prepare_backtest(
    body: BacktestIn, db: Session, user_id: str,
) -> tuple[Strategy, object, list[str], bool, Pool | None]:
    """校验请求并冻结规格,返回 (strategy, execution_spec, codes, use_pool, pool)。"""
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

    pool: Pool | None = None
    if body.pool_id is not None:
        pool = get_pool_or_404(db, body.pool_id, user_id)
    use_pool = strategy.kind == "portfolio" and not codes
    if strategy.kind != "portfolio" and pool is not None:
        # 单标的策略也允许用股票池定义研究范围;与手动 codes 互斥,避免语义含糊
        if codes:
            raise HTTPException(400, "codes 与 pool_id 只能选其一")
        use_pool = True
    if use_pool:
        if pool is None:
            pool = get_pool_or_404(
                db, execution_spec.universe.pool_id, user_id,
            )
        if pool.kind == "index":
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
    return strategy, execution_spec, codes, use_pool, pool


@router.post("", response_model=None)
@plural_router.post("", response_model=None)
def create_backtest(
    body: BacktestIn,
    # 已不使用(异步改走全局任务系统),保留参数兼容既有调用方与单测
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """发起回测。

    - ``settings.backtest_async=True``(默认):插入 pending 行并交给全局任务
      系统(quant_task)后台执行,HTTP 202;同一用户已有进行中任务时 409
    - ``settings.backtest_async=False``(单测默认关):同步执行并返回完整结果
    """
    user_id = user_id_from_claims(claims)
    strategy, execution_spec, codes, use_pool, pool = _prepare_backtest(
        body, db, user_id,
    )
    pool_id = pool.id if use_pool and pool is not None else None

    if settings.backtest_async:
        if user_active_task(db, user_id) is not None:
            raise HTTPException(409, TASK_CONFLICT_MESSAGE)
        versions = component_versions_for_spec(execution_spec)
        run = BacktestRun(
            user_id=user_id,
            strategy_id=strategy.id,
            params=body.params or {},
            costs=body.costs or {},
            pool_id=pool_id,
            codes=codes,
            start=body.start,
            end=body.end,
            metrics=None,
            strategy_spec_snapshot=execution_spec.model_dump(mode="json"),
            strategy_spec_hash=strategy_spec_hash(execution_spec),
            compiler_version=COMPILER_VERSION,
            component_versions=dict(sorted(versions.items())),
            status="pending",
            request_snapshot={
                "codes": codes,
                "params": body.params or {},
                "costs": body.costs or {},
                "dynamic_universe": use_pool,
                "pool_id": pool_id,
            },
            created_at=datetime.now(),
        )
        db.add(run)
        db.commit()
        db.refresh(run)
        try:
            task = submit_task(
                db,
                user_id=user_id,
                type="backtest",
                title=f"回测 · {strategy.name} · {body.start}~{body.end}",
                params={"run_id": run.id},
                ref_id=run.id,
            )
        except TaskConflictError:
            # 并发提交竞态(双窗口同时点):作废刚建的 pending run
            run.status = "cancelled"
            run.error = TASK_CONFLICT_MESSAGE
            run.finished_at = datetime.now()
            db.commit()
            raise HTTPException(409, TASK_CONFLICT_MESSAGE)
        payload = pending_payload(run)
        payload["task_id"] = task.id
        if pool is not None and use_pool:
            payload["pool"] = pool_ref_out(pool)
        return JSONResponse(status_code=202, content=payload)

    try:
        result = run_backtest(
            db, strategy, codes,
            body.start, body.end, body.params, body.costs,
            dynamic_universe=use_pool,
            user_id=user_id,
            pool_id=pool_id,
            execution_spec=execution_spec,
        )
    except StrategyCapabilityError as exc:
        raise HTTPException(400, {
            "message": str(exc),
            "capability": exc.report.model_dump(mode="json"),
        })
    except ValueError as e:
        raise HTTPException(400, str(e))
    transition = None
    try:
        transition = advance_after_backtest(db, strategy, result)
        if transition:
            db.commit()
    except Exception:  # noqa: BLE001
        logger.exception("证据状态推进失败 strategy=%s", strategy.id)
        db.rollback()
    result["evidence_transition"] = transition
    result["status"] = "done"
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
        "status": getattr(run, "status", None) or "done",
        "error": getattr(run, "error", None),
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
        "stocks": StockRepository(db).items(run.codes or []),
        "start": str(run.start),
        "end": str(run.end),
        "costs": run.costs or {},
        "fee_assumptions": evidence.get("fee_assumptions", {}),
        "metrics": metrics,
        "evidence": evidence or None,
        "data_quality": metrics.get("data_quality") if isinstance(metrics, dict) else None,
        "validation": metrics.get("validation"),
        "trade_details": evidence.get("trade_details", []),
        "exit_reason_distribution": evidence.get(
            "exit_reason_distribution", {"by_primary": {}, "all_hits": {}},
        ),
        "created_at": run.created_at.isoformat(sep=" ") if run.created_at else None,
        "started_at": (
            run.started_at.isoformat(sep=" ")
            if getattr(run, "started_at", None) else None
        ),
        "finished_at": (
            run.finished_at.isoformat(sep=" ")
            if getattr(run, "finished_at", None) else None
        ),
        "equity": [{"date": str(e.date), "equity": e.equity} for e in equity],
    }
    # 回显当时所用的池:按编号查历史回测时前端没有本地选择状态,
    # 幸存者偏差标注只能靠这里带回的 kind 判断。池被删则回显 None。
    pool_id = getattr(run, "pool_id", None)
    if pool_id is not None:
        pool = db.execute(select(Pool).where(Pool.id == pool_id)).scalar_one_or_none()
        result["pool"] = pool_ref_out(pool) if pool is not None else None
    return result


@plural_router.get("")
def list_backtests(
    strategy_id: int | None = Query(default=None),
    limit: int = Query(default=20, ge=1, le=50),
    before_run_id: int | None = Query(default=None),
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """列出本人回测 run,summary 字段与 GET /api/backtest/{run_id} 一致。"""
    return list_runs(
        db,
        user_id=user_id_from_claims(claims),
        strategy_id=strategy_id,
        limit=limit,
        before_run_id=before_run_id,
    )


# ---- 全局异步任务系统:handler 注册(app/tasks.py) ----

def _backtest_task_handler(db: Session, task: Task, *, cancel_event=None) -> dict | None:
    """执行已冻结的 BacktestRun,并把它的终态映射到任务上。"""
    execute_backtest_run(task.ref_id, cancel_event=cancel_event)
    # execute_backtest_run 用自己的 Session 提交;先结束当前事务快照,
    # 否则 MySQL REPEATABLE READ 下读到的还是执行前的状态
    db.rollback()
    run = db.get(BacktestRun, task.ref_id)
    if run is None:
        raise RuntimeError("回测记录不存在")
    if run.status not in {"done", "cancelled"}:
        raise RuntimeError(run.error or f"回测未成功(状态 {run.status})")
    return None


def _sweep_task_handler(db: Session, task: Task) -> dict:
    p = task.params or {}
    try:
        strategy = get_strategy_or_404(db, int(p["strategy_id"]), task.user_id)
    except HTTPException as exc:
        raise ValueError(str(exc.detail))
    return _run_sweep_compute(
        db, strategy, list(p.get("codes") or []),
        date.fromisoformat(p["start"]), date.fromisoformat(p["end"]),
        dict(p.get("param_grid") or {}), dict(p.get("costs") or {}),
        bool(p.get("declared")),
    )


def _sensitivity_task_handler(db: Session, task: Task) -> dict:
    p = task.params or {}
    try:
        strategy, execution_spec, codes, use_pool, pool = _prepare_backtest(
            BacktestIn(
                strategy_id=int(p["strategy_id"]),
                codes=list(p.get("codes") or []),
                start=date.fromisoformat(p["start"]),
                end=date.fromisoformat(p["end"]),
                pool_id=p.get("pool_id"),
                costs=dict(p.get("costs") or {}),
            ),
            db, task.user_id,
        )
    except HTTPException as exc:
        raise ValueError(str(exc.detail))
    return _run_sensitivity_compute(
        db, task.user_id, strategy, execution_spec, codes, use_pool, pool,
        date.fromisoformat(p["start"]), date.fromisoformat(p["end"]),
        dict(p.get("costs") or {}),
        [float(m) for m in (p.get("slippage_multipliers") or [])],
    )


register_handler("backtest", _backtest_task_handler, supports_cancel=True)
register_handler("sweep", _sweep_task_handler)
register_handler("sensitivity", _sensitivity_task_handler)
