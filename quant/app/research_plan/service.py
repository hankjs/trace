"""研究计划持久化、版本链、回测绑定和对外序列化。"""
from __future__ import annotations

import hashlib
from datetime import date, timedelta
from typing import Any, Mapping

import pandas as pd
from sqlalchemy import or_, select
from sqlalchemy.orm import Session

from ..data.clock import naive_now_cst, today_cst
from ..data.calendar import is_trading_day
from ..data.ingest import load_bars_df, required_snapshot_fields
from ..models import (BacktestRun, DailyBar, Pool, ResearchPlan,
                      ResearchPlanItem, Signal, Snapshot, Stock, Strategy,
                      TradeCalendar)
from ..strategy.compiler import compile_portfolio
from ..strategy.runtime import strategy_spec_for
from .domain import (PLAN_TYPE_NAMES, PRODUCT_BOUNDARY, STATUS_NAMES,
                     build_portfolio_snapshot, build_single_snapshot,
                     evaluate_single_spec_condition,
                     is_portfolio_rebalance_day,
                     portfolio_spec_score_details)


def next_trading_day(db: Session, day: date) -> date | None:
    """查询交易日历中的下一交易日；未覆盖未来时返回未知。"""
    next_day = db.execute(
        select(TradeCalendar.date).where(
            TradeCalendar.date > day, TradeCalendar.is_open.is_(True))
        .order_by(TradeCalendar.date).limit(1)
    ).scalar_one_or_none()
    if next_day is not None:
        return next_day
    return None


def _unverified_evidence(costs: dict | None = None) -> tuple[None, dict]:
    evidence = {
        "status": "unverified", "status_name": "尚无匹配回测",
        "reason": (
            "未找到当前用户下策略版本、参数、覆盖层、费用和研究范围完全一致的回测。"
            "这只表示缺少可对照的模拟记录,不代表策略无效或有效。"
        ),
    }
    if costs is not None:
        evidence["costs"] = dict(costs)
    return None, evidence


def _code_signature(codes) -> str:
    canonical = "\n".join(sorted({str(code) for code in codes}))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _backtest_evidence(
    db: Session,
    *,
    strategy_id: int,
    strategy_spec_hash: str | None,
    params_snapshot: dict,
    viewer_user_id: str | None,
    plan_code: str | None = None,
    pool_id: int | None = None,
    pool_signature: str | None = None,
    cache: dict | None = None,
) -> tuple[int | None, dict]:
    """只绑定查看者本人、规格哈希、费用和研究范围完全一致的回测。"""
    from ..backtest.engine import DEFAULT_COSTS

    expected_costs = dict(
        params_snapshot.get("simulation_costs") or DEFAULT_COSTS
    )
    if viewer_user_id is None or not strategy_spec_hash:
        return _unverified_evidence(expected_costs)
    cache_key = (
        viewer_user_id, strategy_id, strategy_spec_hash, plan_code, pool_id,
        pool_signature,
        tuple(sorted(expected_costs.items())),
    )
    if cache is not None and cache_key in cache:
        return cache[cache_key]
    candidates = db.execute(
        select(BacktestRun).where(
            BacktestRun.strategy_id == strategy_id,
            BacktestRun.user_id == viewer_user_id,
        ).order_by(BacktestRun.created_at.desc(), BacktestRun.id.desc())
    ).scalars().all()
    for run in candidates:
        if run.strategy_spec_hash != strategy_spec_hash:
            continue
        if (run.costs or {}) != expected_costs:
            continue
        if plan_code is not None:
            if list(run.codes or []) != [plan_code] or run.pool_id is not None:
                continue
        if pool_id is not None and run.pool_id != pool_id:
            continue
        if pool_signature is not None \
                and _code_signature(run.codes or []) != pool_signature:
            continue
        metrics = run.metrics or {}
        persisted_evidence = metrics.get("evidence") or {}
        result = run.id, {
            "status": "verified", "status_name": "已有同配置历史回测",
            "run_id": run.id, "start": str(run.start), "end": str(run.end),
            "costs": run.costs or {},
            "strategy_spec_hash": run.strategy_spec_hash,
            "compiler_version": run.compiler_version,
            "component_versions": dict(run.component_versions or {}),
            "metrics": {
                key: metrics.get(key)
                for key in ("total_return", "max_drawdown", "win_rate", "trade_count")
            },
            "parameter_snapshot": persisted_evidence.get(
                "parameter_snapshot", run.params or {}),
            "fee_assumptions": persisted_evidence.get(
                "fee_assumptions", run.costs or {}),
            "exit_reason_distribution": persisted_evidence.get(
                "exit_reason_distribution", {}),
        }
        if cache is not None:
            cache[cache_key] = result
        return result
    result = _unverified_evidence(expected_costs)
    if cache is not None:
        cache[cache_key] = result
    return result


def _new_plan(strategy: Strategy, snapshot: dict, *, code: str | None,
              pool_id: int | None, signal_price: float | None,
              backtest_run_id: int | None, backtest_evidence: dict,
              supersedes: ResearchPlan | None = None) -> ResearchPlan:
    return ResearchPlan(
        owner_id=strategy.owner_id,
        strategy_is_system=bool(strategy.is_system),
        strategy_id=strategy.id,
        strategy_name=strategy.name,
        template=strategy.template,
        strategy_kind=snapshot["strategy_spec_snapshot"]["kind"],
        strategy_version=snapshot["strategy_version"],
        params_snapshot=snapshot["params_snapshot"],
        strategy_spec_snapshot=snapshot["strategy_spec_snapshot"],
        strategy_spec_hash=snapshot["strategy_spec_hash"],
        plan_type=snapshot["plan_type"],
        code=code,
        pool_id=pool_id,
        data_date=snapshot["data_date"],
        generated_at=naive_now_cst(),
        next_execution_date=snapshot["next_execution_date"],
        valid_until=snapshot["valid_until"],
        signal_type=snapshot["signal_type"],
        status=snapshot["status"],
        status_reason=snapshot["status_reason"],
        price_adjustment="forward",
        signal_price=signal_price,
        entry_observation=snapshot["entry_observation"],
        risk_rules=snapshot["risk_rules"],
        take_profit=snapshot["take_profit"],
        native_exit=snapshot["native_exit"],
        exit_hits=snapshot["exit_hits"],
        portfolio_summary=snapshot["portfolio_summary"],
        backtest_run_id=backtest_run_id,
        backtest_evidence=backtest_evidence,
        product_boundary=PRODUCT_BOUNDARY,
        revision=(supersedes.revision + 1 if supersedes else 1),
        supersedes_plan_id=supersedes.id if supersedes else None,
    )


def create_single_plan(db: Session, strategy: Strategy, signal: Signal,
                       df: pd.DataFrame) -> ResearchPlan:
    """为已落库的单标的信号新建一版计划，并让信号指向最新版。"""
    if strategy_spec_for(strategy).kind != "single":
        raise ValueError("组合策略不能生成单标的研究计划")
    if not is_trading_day(db, signal.date):
        raise ValueError("非交易日不生成研究计划")
    next_day = next_trading_day(db, signal.date)
    reason = signal.reason or {}
    raw_hits = (reason.get("all_exit_reasons") or reason.get("exit_hits")
                or reason.get("exit_reasons") or [])
    hits = [item if isinstance(item, dict) else {"code": str(item)} for item in raw_hits]
    snapshot = build_single_snapshot(
        strategy, df, side=signal.side, data_date=signal.date,
        next_execution_date=next_day,
        entry_price=reason.get("simulated_entry_price"),
        exit_hits=hits,
        overlay_state_rules=reason.get("overlay_state_rules") or [],
    )
    previous = db.get(ResearchPlan, signal.plan_id) if signal.plan_id else None
    if previous is None:
        previous = db.execute(select(ResearchPlan).where(
            ResearchPlan.strategy_id == strategy.id,
            ResearchPlan.plan_type == "single",
            ResearchPlan.code == signal.code,
        ).order_by(ResearchPlan.id.desc()).limit(1)).scalar_one_or_none()
    backtest_run_id, evidence = _backtest_evidence(
        db, strategy_id=strategy.id,
        strategy_spec_hash=snapshot["strategy_spec_hash"],
        params_snapshot=snapshot["params_snapshot"],
        viewer_user_id=None if strategy.is_system else strategy.owner_id,
        plan_code=signal.code)
    plan = _new_plan(
        strategy, snapshot, code=signal.code, pool_id=None,
        signal_price=signal.price, backtest_run_id=backtest_run_id,
        backtest_evidence=evidence, supersedes=previous)
    db.add(plan)
    db.flush()
    signal.plan_id = plan.id
    return plan


def create_holding_plan(
    db: Session,
    strategy: Strategy,
    *,
    code: str,
    data_date: date,
    df: pd.DataFrame,
    simulated_entry_price: float | None = None,
    overlay_state_rules: list[dict] | None = None,
) -> ResearchPlan:
    """为持续模拟持有状态生成每日风险/退出快照，不制造新的买卖信号。"""
    if strategy_spec_for(strategy).kind != "single":
        raise ValueError("组合策略不能生成单标的持有计划")
    if not is_trading_day(db, data_date):
        raise ValueError("非交易日不生成研究计划")
    snapshot = build_single_snapshot(
        strategy, df, side="hold", data_date=data_date,
        next_execution_date=next_trading_day(db, data_date),
        entry_price=simulated_entry_price,
        overlay_state_rules=overlay_state_rules,
    )
    previous = db.execute(select(ResearchPlan).where(
        ResearchPlan.strategy_id == strategy.id,
        ResearchPlan.plan_type == "single",
        ResearchPlan.code == code,
    ).order_by(ResearchPlan.id.desc()).limit(1)).scalar_one_or_none()
    backtest_run_id, evidence = _backtest_evidence(
        db, strategy_id=strategy.id,
        strategy_spec_hash=snapshot["strategy_spec_hash"],
        params_snapshot=snapshot["params_snapshot"],
        viewer_user_id=None if strategy.is_system else strategy.owner_id,
        plan_code=code,
    )
    plan = _new_plan(
        strategy, snapshot, code=code, pool_id=None,
        signal_price=None, backtest_run_id=backtest_run_id,
        backtest_evidence=evidence, supersedes=previous,
    )
    db.add(plan)
    db.flush()
    return plan


def _portfolio_dates(pool_dfs: Mapping[str, pd.DataFrame],
                     data_date: date) -> pd.DatetimeIndex:
    values = sorted({
        pd.Timestamp(day)
        for frame in pool_dfs.values()
        for day in frame.get("date", [])
        if day <= data_date
    })
    return pd.DatetimeIndex(values)


def create_portfolio_plan(
    db: Session,
    strategy: Strategy,
    *,
    data_date: date,
    pool_id: int,
    pool_name: str,
    pool_dfs: Mapping[str, pd.DataFrame],
    eligibility: pd.DataFrame | None = None,
) -> ResearchPlan:
    """编译组合 StrategySpec 并保存调仓计划及逐股变化原因。

    调用方负责按历史口径解析股票池；本函数拒绝基准日没有行情的数据，防止用
    当前成分或旧行情冒充历史调仓计划。
    """
    if strategy_spec_for(strategy).kind != "portfolio":
        raise ValueError("单标的策略不能生成组合调仓计划")
    if not is_trading_day(db, data_date):
        raise ValueError("非交易日不生成研究计划")
    dates = _portfolio_dates(pool_dfs, data_date)
    if not len(dates) or dates[-1].date() != data_date:
        raise ValueError("非交易日或行情未更新到组合计划基准日")
    spec = strategy_spec_for(strategy)
    if spec.kind != "portfolio":
        raise ValueError("策略行 kind 与 StrategySpec 不一致")
    supersedes = db.execute(
        select(ResearchPlan).where(
            ResearchPlan.strategy_id == strategy.id,
            ResearchPlan.plan_type == "portfolio_rebalance",
            ResearchPlan.pool_id == pool_id,
        ).order_by(ResearchPlan.id.desc()).limit(1)
    ).scalar_one_or_none()
    baseline = db.execute(
        select(ResearchPlan).where(
            ResearchPlan.strategy_id == strategy.id,
            ResearchPlan.plan_type == "portfolio_rebalance",
            ResearchPlan.pool_id == pool_id,
            ResearchPlan.data_date < data_date,
        ).order_by(ResearchPlan.data_date.desc(), ResearchPlan.id.desc()).limit(1)
    ).scalar_one_or_none()
    persisted_previous = {}
    if baseline is not None:
        persisted_previous = {
            item.code: float(item.target_weight)
            for item in db.execute(select(ResearchPlanItem).where(
                ResearchPlanItem.plan_id == baseline.id,
            )).scalars()
        }
    compilation = compile_portfolio(
        spec, dates, dict(pool_dfs), eligibility=eligibility,
    )
    weights = compilation.weights
    target = {code: float(value) for code, value in weights.iloc[-1].items()}
    calculated_previous = (
        {code: float(value) for code, value in weights.iloc[-2].items()}
        if len(weights) > 1 else {})
    previous = persisted_previous or calculated_previous
    score_details = portfolio_spec_score_details(
        spec, dates, pool_dfs, data_date,
    )
    scores = {code: detail.get("total") for code, detail in score_details.items()}
    current_reasons = {
        code: reasons
        for (day, code), reasons in compilation.reasons.items()
        if day == pd.Timestamp(data_date)
    }
    eligible_now = (
        {code: bool(value) for code, value in eligibility.reindex(
            index=dates, columns=weights.columns).iloc[-1].fillna(False).items()}
        if eligibility is not None else {code: True for code in weights.columns})
    for code, reasons in current_reasons.items():
        for item in reasons:
            if isinstance(item, dict) and "eligible" in item:
                eligible_now[code] = bool(item["eligible"])
    eligible_now.update({
        code: False for code in previous if code not in weights.columns
    })
    snapshot, item_snapshots = build_portfolio_snapshot(
        strategy, data_date=data_date,
        next_execution_date=next_trading_day(db, data_date),
        pool_name=pool_name, target_weights=target,
        previous_weights=previous, scores=scores,
        eligible=eligible_now,
        score_details=score_details,
        compiler_reasons=current_reasons,
        component_versions=compilation.component_versions,
    )
    pool = db.get(Pool, pool_id)
    if pool is not None and pool.kind == "static":
        snapshot["portfolio_summary"]["pool_signature"] = _code_signature(
            pool_dfs.keys()
        )
    plan_exit_hits: list[dict] = []
    for code, reasons in current_reasons.items():
        for reason in reasons:
            if not isinstance(reason, dict):
                continue
            for flag, reason_code in (
                ("risk_blocked", "risk_filter"),
                ("native_exit", "native_exit"),
            ):
                if reason.get(flag):
                    plan_exit_hits.append({
                        "code": code,
                        "reason_code": reason_code,
                        "compiler_reason": reason,
                    })
            for hit in reason.get("all_reasons") or []:
                if isinstance(hit, dict):
                    plan_exit_hits.append({"code": code, **hit})
    snapshot["exit_hits"] = plan_exit_hits
    scheduled = is_portfolio_rebalance_day(spec, dates)
    changed = any(
        abs(item["target_weight"] - item["previous_weight"]) > 1e-10
        for item in item_snapshots)
    if not scheduled and not changed:
        raise ValueError("基准日不是计划调仓日，且目标权重没有资格或风险变化")
    if not scheduled:
        snapshot["signal_type"] = "qualification_change"
    if plan_exit_hits:
        snapshot["status"] = "exit_triggered"
        snapshot["status_reason"] = {
            "code": "portfolio_exit_condition_met",
            "text": "基准日收盘已满足逐股风险、退出或调仓调出条件，最早按下一交易日开盘模拟调整。",
        }
    elif not scheduled:
        snapshot["status"] = "current"
        snapshot["status_reason"] = {
            "code": "native_qualification_change",
            "text": "规格中的日常资格或风险条件发生变化，逐股目标权重已更新。",
        }
    backtest_run_id, evidence = _backtest_evidence(
        db, strategy_id=strategy.id,
        strategy_spec_hash=snapshot["strategy_spec_hash"],
        params_snapshot=snapshot["params_snapshot"],
        viewer_user_id=None if strategy.is_system else strategy.owner_id,
        pool_id=pool_id,
        pool_signature=snapshot["portfolio_summary"].get("pool_signature"))
    plan = _new_plan(
        strategy, snapshot, code=None, pool_id=pool_id,
        signal_price=None, backtest_run_id=backtest_run_id,
        backtest_evidence=evidence, supersedes=supersedes)
    db.add(plan)
    db.flush()
    db.add_all([
        ResearchPlanItem(plan_id=plan.id, **item)
        for item in item_snapshots
    ])
    return plan


def visible_to(user_id: str):
    return or_(ResearchPlan.strategy_is_system.is_(True),
               ResearchPlan.owner_id == user_id)


def _execution_market_issue(
    db: Session,
    *,
    code: str,
    execution_date: date,
    direction: str,
    as_of: date,
) -> dict | None:
    """按与回测一致的开盘约束检查单个计划变化。"""
    execution_bar = db.get(DailyBar, (code, execution_date))
    if execution_bar is None:
        if as_of >= execution_date:
            return {
                "code": "next_day_untradable",
                "text": f"{code} 下一模拟成交日没有可用行情，可能停牌，需要重新评估。",
            }
        return None
    execution_open = float(execution_bar.open)
    execution_volume = float(execution_bar.volume)
    if (
        not pd.notna(execution_open)
        or execution_open <= 0
        or not pd.notna(execution_volume)
        or execution_volume <= 0
    ):
        return {
            "code": "next_day_untradable",
            "text": f"{code} 下一模拟成交日停牌或缺少有效开盘行情，需要重新评估。",
        }
    prior_bar = db.execute(
        select(DailyBar).where(
            DailyBar.code == code,
            DailyBar.date < execution_date,
        ).order_by(DailyBar.date.desc()).limit(1)
    ).scalar_one_or_none()
    if prior_bar is None or prior_bar.close <= 0:
        return None

    from ..backtest.engine import limit_pct

    ratio = execution_open / float(prior_bar.close) - 1
    threshold = limit_pct(
        code, is_st=execution_bar.is_st is True) * 0.995
    if direction == "buy" and ratio >= threshold:
        return {
            "code": "open_limit_up",
            "text": f"{code} 下一模拟成交日开盘触及涨停约束，不能按回测假设成交。",
        }
    if direction == "sell" and ratio <= -threshold:
        return {
            "code": "open_limit_down",
            "text": f"{code} 下一模拟成交日开盘触及跌停约束，不能按回测假设成交。",
        }
    return None


def _reevaluation_from_market(
    db: Session,
    plan: ResearchPlan,
    as_of: date,
) -> dict | None:
    """用已知行情判断计划是否需要重评，不把盘中快照当成确认信号。"""
    if plan.status not in {"current", "exit_triggered"}:
        return None

    execution_bar = None
    if plan.next_execution_date is not None and plan.code:
        issue = _execution_market_issue(
            db, code=plan.code, execution_date=plan.next_execution_date,
            direction="sell" if plan.signal_type == "sell" else "buy",
            as_of=as_of,
        )
        if issue is not None:
            return issue
        execution_bar = db.get(DailyBar, (plan.code, plan.next_execution_date))
    elif plan.next_execution_date is not None \
            and plan.plan_type == "portfolio_rebalance":
        items = db.execute(select(ResearchPlanItem).where(
            ResearchPlanItem.plan_id == plan.id,
        )).scalars()
        for item in items:
            delta = float(item.target_weight) - float(item.previous_weight)
            if abs(delta) <= 1e-12:
                continue
            issue = _execution_market_issue(
                db, code=item.code, execution_date=plan.next_execution_date,
                direction="buy" if delta > 0 else "sell", as_of=as_of,
            )
            if issue is not None:
                return issue

    if not plan.code:
        return None

    observation = plan.entry_observation or {}
    if observation.get("kind") != "range":
        return None
    lower, upper = observation.get("lower"), observation.get("upper")
    if not isinstance(lower, (int, float)) or not isinstance(upper, (int, float)):
        return None

    checked_price = float(execution_bar.open) if execution_bar is not None else None
    checked_at = plan.next_execution_date if execution_bar is not None else None
    if checked_price is None:
        snapshot = db.execute(
            select(Snapshot).where(Snapshot.code == plan.code)
            .order_by(Snapshot.ts.desc()).limit(1)
        ).scalar_one_or_none()
        if snapshot is not None and snapshot.ts.date() >= plan.data_date:
            checked_price, checked_at = float(snapshot.price), snapshot.ts.date()
    if checked_price is not None and not float(lower) <= checked_price <= float(upper):
        return {
            "code": "price_outside_entry_range",
            "text": (
                f"{checked_at} 行情 {checked_price:.4f} 已越过进场观察区间"
                f" {float(lower):.4f} 至 {float(upper):.4f}，需要重新评估。"
            ),
        }
    return None


def _native_condition_reevaluation(
    db: Session,
    plan: ResearchPlan,
    as_of: date,
) -> tuple[str, dict] | None:
    """有新确认收盘后按不可变计划快照重算原生入场/观察条件。"""
    if (
        plan.plan_type != "single"
        or plan.status != "current"
        or not plan.code
        or plan.signal_type not in {"buy", "watch"}
    ):
        return None
    latest_date = db.execute(
        select(DailyBar.date).where(
            DailyBar.code == plan.code,
            DailyBar.date > plan.data_date,
            DailyBar.date <= as_of,
        ).order_by(DailyBar.date.desc()).limit(1)
    ).scalar_one_or_none()
    if latest_date is None:
        return None

    if not plan.strategy_spec_snapshot:
        return "reevaluate", {
            "code": "strategy_spec_snapshot_missing",
            "text": "历史计划没有完整 StrategySpec 快照，不能可靠重算原生条件。",
        }
    # 重评帧与编译入口同口径:按快照规格的 data_requirements 补齐估值/财务
    # 字段(PIT join,不用未来数据),否则用这些字段的策略永远缺列。
    from ..strategy.spec import parse_strategy_spec

    history_start = plan.data_date - timedelta(days=550)
    extra_fields = required_snapshot_fields(
        parse_strategy_spec(plan.strategy_spec_snapshot),
    )
    frame = load_bars_df(
        db, plan.code, start=history_start, end=latest_date,
        extra_fields=extra_fields,
    )
    result = evaluate_single_spec_condition(
        plan.strategy_spec_snapshot, frame, plan.signal_type,
    )
    if result["satisfied"] is None:
        return "reevaluate", {
            "code": "snapshot_rule_data_insufficient",
            "text": f"{latest_date} 已有新收盘数据，但{result['text']}",
        }
    if not result["satisfied"]:
        return "invalid", {
            "code": "native_entry_condition_lost",
            "text": f"{latest_date} 最新确认收盘显示：{result['text']} 原研究计划已失效。",
        }
    return None


def effective_status(
    plan: ResearchPlan,
    as_of: date | None = None,
    *,
    db: Session | None = None,
    read_context: dict | None = None,
) -> tuple[str, dict]:
    """有效期和市场偏离是读取状态，不改写不可变的生成快照。"""
    as_of = as_of or today_cst()
    if db is not None:
        superseded_ids = (
            read_context.get("superseded_plan_ids")
            if read_context is not None else None
        )
        is_superseded = plan.id in superseded_ids if superseded_ids is not None else (
            db.execute(select(ResearchPlan.id).where(
                ResearchPlan.supersedes_plan_id == plan.id,
                ResearchPlan.data_date <= as_of,
            ).limit(1)).scalar_one_or_none() is not None
        )
        if is_superseded:
            return "expired", {
                "code": "superseded_by_new_plan",
                "text": "该计划已被更新的数据快照替代，请查看后继版本。",
            }
        native_result = _native_condition_reevaluation(db, plan, as_of)
        if native_result is not None and native_result[0] == "invalid":
            return native_result
        reason = _reevaluation_from_market(db, plan, as_of)
        if reason is not None:
            return "reevaluate", reason
        if native_result is not None:
            return native_result
    if plan.status == "current" and plan.valid_until and as_of > plan.valid_until:
        return "expired", {
            "code": "validity_elapsed",
            "text": "计划已超过有效期限，且尚未基于新数据刷新。",
        }
    return plan.status, plan.status_reason


def plan_summary(
    plan: ResearchPlan,
    *,
    as_of: date | None = None,
    db: Session | None = None,
    viewer_user_id: str | None = None,
    evidence_cache: dict | None = None,
    read_context: dict | None = None,
) -> dict:
    status, status_reason = effective_status(
        plan, as_of, db=db, read_context=read_context)
    evidence = (
        plan.backtest_evidence
        or _unverified_evidence(
            (plan.params_snapshot or {}).get("simulation_costs"),
        )[1]
    )
    if db is not None:
        _, evidence = _backtest_evidence(
            db, strategy_id=plan.strategy_id,
            strategy_spec_hash=plan.strategy_spec_hash,
            params_snapshot=plan.params_snapshot,
            viewer_user_id=viewer_user_id,
            plan_code=plan.code,
            pool_id=plan.pool_id,
            pool_signature=(plan.portfolio_summary or {}).get("pool_signature"),
            cache=evidence_cache)
    portfolio_summary = dict(plan.portfolio_summary or {})
    portfolio_summary.pop("pool_signature", None)
    return {
        "plan_id": plan.id,
        "revision": plan.revision,
        "plan_type": plan.plan_type,
        "plan_type_name": PLAN_TYPE_NAMES.get(plan.plan_type, plan.plan_type),
        "status": status,
        "status_name": STATUS_NAMES.get(status, status),
        "status_reason": status_reason,
        "data_date": str(plan.data_date),
        "generated_at": plan.generated_at.isoformat(sep=" "),
        "next_simulated_execution_date": (
            str(plan.next_execution_date) if plan.next_execution_date else None),
        "valid_until": str(plan.valid_until) if plan.valid_until else None,
        "signal_type": plan.signal_type,
        "signal_close_price": plan.signal_price,
        "entry_observation": plan.entry_observation,
        "risk_rules": plan.risk_rules,
        "take_profit": plan.take_profit,
        "native_exit": plan.native_exit,
        "risk_rule_count": len(plan.risk_rules or []),
        "take_profit_enabled": bool((plan.take_profit or {}).get("enabled")),
        "backtest_status": evidence["status"],
        "backtest_evidence": evidence,
        "portfolio_summary": portfolio_summary or None,
        "product_boundary": plan.product_boundary,
    }


def plan_detail(db: Session, plan: ResearchPlan, *, as_of: date | None = None,
                viewer_user_id: str | None = None) -> dict:
    result = plan_summary(
        plan, as_of=as_of, db=db, viewer_user_id=viewer_user_id)
    result.update({
        "strategy": {
            "id": plan.strategy_id, "name": plan.strategy_name,
            "template": plan.template, "kind": plan.strategy_kind,
            "version": plan.strategy_version,
            "spec_hash": plan.strategy_spec_hash,
        },
        "params_snapshot": plan.params_snapshot,
        "strategy_spec_snapshot": plan.strategy_spec_snapshot,
        "strategy_spec_hash": plan.strategy_spec_hash,
        "price_adjustment": plan.price_adjustment,
        "risk_rules": plan.risk_rules,
        "take_profit": plan.take_profit,
        "native_exit": plan.native_exit,
        "exit_hits": plan.exit_hits,
        "supersedes_plan_id": plan.supersedes_plan_id,
    })
    if plan.plan_type == "portfolio_rebalance":
        rows = db.execute(
            select(ResearchPlanItem, Stock)
            .outerjoin(Stock, Stock.code == ResearchPlanItem.code)
            .where(ResearchPlanItem.plan_id == plan.id)
            .order_by(ResearchPlanItem.target_weight.desc(),
                      ResearchPlanItem.code)
        ).all()
        result["portfolio_changes"] = [{
            "code": item.code,
            "name": stock.name if stock else "",
            "previous_weight": item.previous_weight,
            "target_weight": item.target_weight,
            "change_type": item.change_type,
            "score": item.score,
            "score_details": item.score_details,
            "rank": item.rank,
            "eligible": bool(item.eligible),
            "reasons": [
                reason.get("text", reason.get("code", ""))
                if isinstance(reason, dict) else str(reason)
                for reason in (item.reasons or [])
            ],
            "reason_details": item.reasons,
            "risk_snapshot": item.risk_snapshot,
        } for item, stock in rows]
    else:
        result["portfolio_changes"] = []
    return result


__all__ = [
    "create_holding_plan", "create_portfolio_plan", "create_single_plan",
    "effective_status", "next_trading_day", "plan_detail", "plan_summary",
    "visible_to",
]
