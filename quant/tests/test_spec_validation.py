"""策略规格 validation 段的执行侧测试:基线对比、OOS 分段、否决判定与证据状态机。

合成数据 + 内存 SQLite,不依赖 MySQL。覆盖:
1. ValidationSpec 结构化否决规则的校验与序列化兼容(预设哈希不变);
2. 基线净值构造与对比报告(含未知基线记为 unavailable);
3. OOS 切分(最后 20% 交易日)与两段指标;
4. rejection 判定:旧字符串兼容映射命中/未命中/不可评估、结构化规则;
5. evidence_status 状态机:自动推进全路径、身份哈希匹配、编辑回落、人工复位;
6. 按规格声明执行参数扫描的 API。

运行: cd quant && uv run pytest tests/test_spec_validation.py
"""
from __future__ import annotations

import sys
from copy import deepcopy
from datetime import date
from pathlib import Path

import numpy as np
import pandas as pd
import pytest
from fastapi import HTTPException
from pydantic import ValidationError

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.backtest import BacktestIn, SweepIn, create_backtest, sweep
from app.api.strategies import (
    StrategyCreateIn,
    StrategyEvidenceIn,
    StrategyPatchIn,
    create_strategy,
    get_strategy,
    update_evidence_status,
    update_strategy,
)
from app.backtest.validation import (
    MIN_OOS_BARS,
    OOS_FRACTION,
    baseline_equity,
    build_oos_report,
    evaluate_declared_sweep,
    evaluate_rejection,
    segment_metrics,
    split_oos,
)
from app.db import Base
from app.models import BacktestRun, Strategy
from app.strategy.evidence import (
    advance_after_backtest,
    apply_manual_action,
    candidate_spec_hashes,
    resolve_status_on_edit,
    spec_identity_hash,
    with_status,
)
from app.strategy.presets import SYSTEM_STRATEGY_SPECS, get_preset_spec
from app.strategy.spec import (
    StrategySpec,
    canonical_spec_json,
    parse_strategy_spec,
    strategy_spec_hash,
)

USER_A = "11111111-1111-1111-1111-111111111111"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _field(name: str) -> dict:
    return {"op": "field", "name": name}


def _literal(value: float) -> dict:
    return {"op": "literal", "value": value}


def _spec_dict(
    *,
    rejection_criteria: list[str] | None = None,
    rejection_rules: list[dict] | None = None,
    baseline_ids: list[str] | None = None,
    locked_oos: bool = True,
    parameter_scans: list[dict] | None = None,
) -> dict:
    """单标的规格:close>10 入场,close<8 离场,验证段可定制。"""
    validation = {
        "baseline_ids": baseline_ids or ["buy_and_hold", "equal_weight"],
        "locked_oos": locked_oos,
        "rejection_criteria": rejection_criteria or ["capacity_failure"],
        "parameter_scans": parameter_scans or [],
    }
    if rejection_rules:
        validation["rejection_rules"] = rejection_rules
    return {
        "schema_version": 1,
        "kind": "single",
        "metadata": {
            "canonical_id": "USER-VALIDATION-01",
            "sources": [{"book": "测试", "candidate_id": "VALIDATION-01"}],
            "evidence_status": "unverified",
            "hypothesis": "站上 10 元持有,跌破 8 元退出。",
        },
        "universe": {
            "pool_id": 2, "exclude_st": True,
            "min_listing_days": 60, "min_amount_avg20": 0.0,
        },
        "data_requirements": [
            {"field": "close", "availability": "daily_close", "required": True},
        ],
        "entry": {
            "condition": {"op": "gt", "left": _field("close"), "right": _literal(10.0)},
            "reason_code": "close_above_10",
        },
        "positioning": {"type": "binary", "target": 1.0},
        "holding": {
            "allow_add": False, "allow_reduce": False,
            "cooldown_days": 0, "risk_reentry": "native_reset",
        },
        "native_exit": {
            "condition": {"op": "lt", "left": _field("close"), "right": _literal(8.0)},
            "reason_code": "close_below_8",
        },
        "overlays": {
            "risk": {
                "enabled": False, "type": "fixed_pct", "value": 0.08,
                "atr_period": 14, "trailing": False,
            },
            "take_profit": {
                "enabled": False, "type": "fixed_pct", "value": 0.2,
                "atr_period": 14, "trailing": False,
            },
        },
        "portfolio_constraints": {
            "long_only": True, "max_positions": 500,
            "max_single_weight": 1.0, "max_total_weight": 1.0,
        },
        "execution": {
            "signal_time": "close", "execution_time": "next_open",
            "buy_limit_policy": "reject", "sell_limit_policy": "retry",
            "suspension_policy": "reject_entry_retry_exit",
            "missing_bar_policy": "reject_entry_retry_exit",
            "cost_model": "a_share_daily_v1", "max_entry_premium": 0.0,
        },
        "validation": validation,
    }


def _frame(prices: list[float]) -> pd.DataFrame:
    dates = pd.bdate_range(date(2024, 1, 2), periods=len(prices))
    close = np.asarray(prices, dtype=float)
    return pd.DataFrame({
        "date": dates.date,
        "open": close,
        "high": close * 1.01,
        "low": close * 0.99,
        "close": close,
        "raw_close": close,
        "volume": np.full(len(close), 1e6),
        "amount": np.full(len(close), 1e7),
        "is_st": [False] * len(close),
    })


def _frames(prices: list[float], *codes: str) -> dict[str, pd.DataFrame]:
    return {code: _frame(prices) for code in codes}


# 先涨后跌:触发一次完整买卖回合(round_trips=1),OOS 段覆盖下跌段
RISE_THEN_FALL = [10 + i * 0.05 for i in range(100)] + [
    14.95 - i * 0.4 for i in range(1, 21)
]

START = date(2024, 1, 2)
END = pd.bdate_range(START, periods=len(RISE_THEN_FALL))[-1].date()


def _ctx(**overrides) -> dict:
    """evaluate_rejection 的默认上下文:OOS 可用、基线可用、全区间 1 回合。"""
    ctx = {
        "full_metrics": {"annual_return": 0.3, "round_trips": 1},
        "oos_report": {
            "enabled": True, "available": True,
            "oos": {"annual_return": 0.2, "total_return": 0.03},
            "in_sample": {"annual_return": 0.35},
        },
        "baselines": [{
            "baseline_id": "buy_and_hold", "status": "ok",
            "metrics": {"annual_return": 0.1},
            "in_sample_metrics": {"annual_return": 0.12},
            "oos_metrics": {"annual_return": 0.1},
        }],
        "sweep": None,
    }
    ctx.update(overrides)
    return ctx


# ------------------------------------------------------------ 规格层

def test_rejection_rules_omitted_keep_preset_canonical_unchanged():
    """无结构化规则时序列化形状不变,六个预设的规范化 JSON 不含新字段。"""
    for name, spec in SYSTEM_STRATEGY_SPECS.items():
        canonical = canonical_spec_json(spec)
        assert "rejection_rules" not in canonical, name


def test_rejection_rule_validation():
    raw = _spec_dict(rejection_rules=[{
        "metric": "annual_return", "op": "lt", "threshold": 0.0,
        "segment": "oos", "description": "样本外年化为负则否决",
    }])
    spec = parse_strategy_spec(raw)
    dumped = spec.model_dump(mode="json")
    assert dumped["validation"]["rejection_rules"][0]["metric"] == "annual_return"
    # 带规则与不带规则的规范化哈希必须不同
    assert strategy_spec_hash(spec) != strategy_spec_hash(_spec_dict())

    with pytest.raises(ValidationError):
        parse_strategy_spec(_spec_dict(rejection_rules=[{
            "metric": "not_a_metric", "op": "lt", "threshold": 0.0,
        }]))
    with pytest.raises(ValidationError):
        parse_strategy_spec(_spec_dict(rejection_rules=[{
            "metric": "annual_return", "op": "lt", "threshold": float("nan"),
        }]))
    with pytest.raises(ValidationError):
        parse_strategy_spec(_spec_dict(rejection_rules=[{
            "metric": "annual_return", "op": "eq", "threshold": 0.0,
        }]))


# ------------------------------------------------------------ 基线与 OOS

def test_baseline_equity_buy_and_hold_and_equal_weight():
    start, end = date(2024, 1, 2), date(2024, 3, 29)
    rising = [10.0 * 1.01 ** i for i in range(60)]
    flat = [10.0] * 60
    frames = {"A": _frame(rising), "B": _frame(flat)}

    hold = baseline_equity("buy_and_hold", frames, start, end)
    expected = (1.01 ** 59 + 1.0) / 2
    assert hold is not None
    assert hold.iloc[-1] == pytest.approx(expected, rel=1e-6)

    rebalanced = baseline_equity("equal_weight", frames, start, end)
    # 每日等权再平衡:日收益恒为 (1% + 0%) / 2
    assert rebalanced is not None
    assert rebalanced.iloc[-1] == pytest.approx(1.005 ** 59, rel=1e-6)

    assert baseline_equity("unknown_baseline", frames, start, end) is None


def test_split_oos_and_segment_metrics():
    eq = pd.Series(
        np.linspace(1.0, 1.5, 100),
        index=pd.bdate_range(date(2024, 1, 2), periods=100),
    )
    in_sample, oos = split_oos(eq)
    assert len(oos) == int(100 * OOS_FRACTION)
    assert len(in_sample) == 100 - len(oos)

    metrics = segment_metrics(oos)
    assert metrics["total_return"] > 0
    assert metrics["annual_return"] is not None
    # 短序列年化/夏普不可靠,返回 None
    short = segment_metrics(eq.iloc[:5])
    assert short["annual_return"] is None
    assert short["sharpe"] is None


def test_build_oos_report_availability():
    eq = pd.Series(
        np.linspace(1.0, 1.2, 60),
        index=pd.bdate_range(date(2024, 1, 2), periods=60),
    )
    assert build_oos_report(False, eq) == {"enabled": False}

    report = build_oos_report(True, eq)
    assert report["available"] is True
    assert report["oos_bars"] == int(60 * OOS_FRACTION)
    assert report["in_sample"]["total_return"] > 0
    assert report["oos"]["total_return"] > 0

    short = eq.iloc[: MIN_OOS_BARS + 2]
    unavailable = build_oos_report(True, short)
    assert unavailable["available"] is False


# ------------------------------------------------------------ 否决判定

def test_capacity_failure_hit_and_pass():
    spec = parse_strategy_spec(_spec_dict())
    hit = evaluate_rejection(spec, **_ctx(full_metrics={"round_trips": 0}))
    assert hit["verdict"] == "rejected"
    assert hit["hits"][0]["criterion"] == "capacity_failure"

    passed = evaluate_rejection(spec, **_ctx())
    assert passed["verdict"] == "passed"
    assert passed["hits"] == []


def test_no_net_oos_increment_hit_pass_unevaluated():
    spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["no_net_oos_increment"]),
    )
    # 策略 OOS 年化 0.2 > 基线 0.1:通过
    assert evaluate_rejection(spec, **_ctx())["verdict"] == "passed"
    # 策略 OOS 年化 0.05 < 基线 0.1:否决
    weak = _ctx(oos_report={
        "enabled": True, "available": True,
        "oos": {"annual_return": 0.05}, "in_sample": {},
    })
    rejected = evaluate_rejection(spec, **weak)
    assert rejected["verdict"] == "rejected"
    assert rejected["hits"][0]["segment"] == "oos"
    # OOS 不可用 / 基线不可用:如实记为不可评估
    no_oos = _ctx(oos_report={"enabled": True, "available": False})
    assert evaluate_rejection(spec, **no_oos)["verdict"] == "incomplete"
    no_baseline = _ctx(baselines=[{"baseline_id": "x", "status": "unavailable"}])
    assert evaluate_rejection(spec, **no_baseline)["verdict"] == "incomplete"


def test_unstable_parameters_requires_declared_sweep():
    scans = [{"path": "$.native_exit.condition.right.value", "values": [7.0, 8.0, 9.0]}]
    spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["unstable_parameters"],
                   parameter_scans=scans),
    )
    # 无扫描结果:不可评估
    assert evaluate_rejection(spec, **_ctx())["verdict"] == "incomplete"
    # 扫描结果中当前参数处于后半:否决
    sweep = {"status": "evaluated", "unstable": True, "better_share": 0.67,
             "current": 0.1, "median": 0.2}
    assert evaluate_rejection(spec, **_ctx(sweep=sweep))["verdict"] == "rejected"
    # 扫描结果稳定:通过
    stable = {**sweep, "unstable": False, "better_share": 0.33}
    assert evaluate_rejection(spec, **_ctx(sweep=stable))["verdict"] == "passed"
    # 未声明扫描的规格:不可评估而非否决
    no_scans = parse_strategy_spec(
        _spec_dict(rejection_criteria=["unstable_parameters"]),
    )
    assert evaluate_rejection(no_scans, **_ctx())["verdict"] == "incomplete"


def test_unknown_legacy_criterion_is_unevaluated():
    spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["some_future_criterion"]),
    )
    result = evaluate_rejection(spec, **_ctx())
    assert result["verdict"] == "incomplete"
    assert "未知否决条件" in result["unevaluated"][0]["reason"]


def test_structured_rule_hit_miss_unevaluated():
    rules = [{
        "metric": "annual_return", "op": "lt", "threshold": 0.5,
        "segment": "full", "description": "全区间年化低于 50% 则否决",
    }]
    spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["capacity_failure"],
                   rejection_rules=rules),
    )
    # 全区间年化 0.3 < 0.5:命中
    assert evaluate_rejection(spec, **_ctx())["verdict"] == "rejected"
    # 全区间年化 0.6:不命中
    strong = _ctx(full_metrics={"annual_return": 0.6, "round_trips": 1})
    assert evaluate_rejection(spec, **strong)["verdict"] == "passed"

    # OOS 段规则在 OOS 不可用时记为不可评估
    oos_rules = [{
        "metric": "sharpe", "op": "lt", "threshold": 0.0, "segment": "oos",
    }]
    oos_spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["capacity_failure"],
                   rejection_rules=oos_rules),
    )
    no_oos = _ctx(oos_report={"enabled": True, "available": False})
    assert evaluate_rejection(oos_spec, **no_oos)["verdict"] == "incomplete"


def test_structured_rule_excess_vs_best_baseline():
    rules = [{
        "metric": "excess_annual_return_vs_best_baseline",
        "op": "lt", "threshold": 0.05, "segment": "full",
    }]
    spec = parse_strategy_spec(
        _spec_dict(rejection_criteria=["capacity_failure"],
                   rejection_rules=rules),
    )
    # 0.3 - 0.1 = 0.2 >= 0.05:通过
    assert evaluate_rejection(spec, **_ctx())["verdict"] == "passed"
    # 基线不可用:不可评估
    no_baseline = _ctx(baselines=[{"baseline_id": "x", "status": "unavailable"}])
    assert evaluate_rejection(spec, **no_baseline)["verdict"] == "incomplete"


def test_evaluate_declared_sweep():
    scans = [{"path": "$.native_exit.condition.right.value", "values": [7.0, 8.0, 9.0]}]
    spec = parse_strategy_spec(_spec_dict(parameter_scans=scans))
    rows = [
        {"params": {"$.native_exit.condition.right.value": 7.0},
         "metrics": {"annual_return_median": 0.3}},
        {"params": {"$.native_exit.condition.right.value": 8.0},
         "metrics": {"annual_return_median": 0.1}},
        {"params": {"$.native_exit.condition.right.value": 9.0},
         "metrics": {"annual_return_median": 0.2}},
    ]
    result = evaluate_declared_sweep(spec, rows)
    assert result["status"] == "evaluated"
    assert result["current"] == 0.1
    # 3 组中 2 组优于当前:不稳定
    assert result["better_share"] == pytest.approx(2 / 3, abs=1e-3)
    assert result["unstable"] is True

    # 当前参数(8.0)不在扫描结果中:不可评估
    other = parse_strategy_spec(_spec_dict(parameter_scans=[{
        "path": "$.native_exit.condition.right.value", "values": [6.0, 7.0],
    }]))
    without_current = [
        row for row in rows
        if row["params"]["$.native_exit.condition.right.value"] != 8.0
    ]
    assert evaluate_declared_sweep(other, without_current)["status"] == "unevaluated"


# ------------------------------------------------------------ 状态机单元

def _strategy_row(db: Session, spec: dict) -> Strategy:
    strategy = Strategy(
        owner_id=USER_A, is_system=False, name="状态机",
        template="strategy_spec", kind="single", params={},
        spec=deepcopy(spec), spec_hash=strategy_spec_hash(spec), enabled=True,
    )
    db.add(strategy)
    db.commit()
    return strategy


def _run_result(spec: dict, verdict: str = "passed",
                oos_available: bool = True) -> dict:
    return {
        "strategy_spec_hash": strategy_spec_hash(spec),
        "validation": {
            "rejection": {"verdict": verdict, "hits": [], "unevaluated": []},
            "oos": {"enabled": True, "available": oos_available},
        },
    }


def test_identity_hash_ignores_evidence_status():
    spec = parse_strategy_spec(_spec_dict())
    candidates = candidate_spec_hashes(spec)
    assert len(candidates) == 5
    assert spec_identity_hash(spec) == spec_identity_hash(
        with_status(spec, "oos_passed"),
    )
    assert strategy_spec_hash(with_status(spec, "rejected")) in candidates


def test_advance_full_path():
    with _session() as db:
        raw = _spec_dict()
        strategy = _strategy_row(db, raw)

        # unverified --(OOS 回测全过)--> oos_passed(允许跨级前进)
        transition = advance_after_backtest(db, strategy, _run_result(raw))
        assert transition == {"from": "unverified", "to": "oos_passed"}
        assert strategy.spec["metadata"]["evidence_status"] == "oos_passed"

        # 只前进:再来一次同样的回测不再迁移
        assert advance_after_backtest(db, strategy, _run_result(raw)) is None

        # 命中否决 -> rejected(终态)
        transition = advance_after_backtest(
            db, strategy, _run_result(raw, verdict="rejected"),
        )
        assert transition == {"from": "oos_passed", "to": "rejected"}

        # rejected 不自动迁移
        assert advance_after_backtest(db, strategy, _run_result(raw)) is None


def test_advance_backtested_when_incomplete_or_no_oos():
    with _session() as db:
        raw = _spec_dict()
        strategy = _strategy_row(db, raw)
        # verdict incomplete(有未评估条件)只能到 backtested
        transition = advance_after_backtest(
            db, strategy, _run_result(raw, verdict="incomplete"),
        )
        assert transition == {"from": "unverified", "to": "backtested"}

        # OOS 不可用同样只能到 backtested;已 backtested 不再迁移
        assert advance_after_backtest(
            db, strategy, _run_result(raw, oos_available=False),
        ) is None

        # 哈希不匹配(旧规格/临时参数)不推进
        other = _run_result(_spec_dict(locked_oos=False))
        assert advance_after_backtest(db, strategy, other) is None


def test_manual_actions_and_reset():
    with _session() as db:
        raw = _spec_dict()
        strategy = _strategy_row(db, raw)

        transition = apply_manual_action(db, strategy, "mark_design_complete")
        assert transition == {"from": "unverified", "to": "design_complete"}

        # 自动推进的状态不允许手改
        with pytest.raises(ValueError):
            apply_manual_action(db, strategy, "mark_design_complete")
        with pytest.raises(ValueError):
            apply_manual_action(db, strategy, "reset_rejected")

        advance_after_backtest(db, strategy, _run_result(raw, verdict="rejected"))
        assert strategy.spec["metadata"]["evidence_status"] == "rejected"
        transition = apply_manual_action(db, strategy, "reset_rejected")
        assert transition == {"from": "rejected", "to": "design_complete"}


def test_resolve_status_on_edit_fallback():
    old = _spec_dict()
    old_spec = parse_strategy_spec(old)
    # 身份未变:保持原状态
    assert resolve_status_on_edit(old, old_spec) == "unverified"
    # 内容变化 + 高状态:回落到 design_complete
    advanced = with_status(old_spec, "oos_passed").model_dump(mode="json")
    edited = parse_strategy_spec(_spec_dict(locked_oos=False))
    assert resolve_status_on_edit(advanced, edited) == "design_complete"
    # rejected 同样是旧规格的结论,编辑后回落
    rejected = with_status(old_spec, "rejected").model_dump(mode="json")
    assert resolve_status_on_edit(rejected, edited) == "design_complete"
    # 低状态不回落
    design = with_status(old_spec, "design_complete").model_dump(mode="json")
    assert resolve_status_on_edit(design, edited) == "design_complete"
    fresh = with_status(old_spec, "unverified").model_dump(mode="json")
    assert resolve_status_on_edit(fresh, edited) == "unverified"


# ------------------------------------------------------------ API 集成

def _patch_bars(monkeypatch, prices: list[float]) -> None:
    frame = _frame(prices)
    monkeypatch.setattr(
        "app.backtest.engine.load_bars_df",
        lambda db, code, start=None, end=None, **kwargs: frame,
    )


def test_backtest_response_contains_validation_report(monkeypatch):
    """回测完成自动带基线对比 / OOS 分段 / 否决判定,并随 metrics 持久化。"""
    _patch_bars(monkeypatch, RISE_THEN_FALL)
    with _session() as db:
        created = create_strategy(
            StrategyCreateIn(name="验证段执行", spec=_spec_dict()),
            db=db, claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(strategy_id=created["id"], codes=["sh.600519"],
                       start=START, end=END),
            db=db, claims=CLAIMS_A,
        )
        validation = result["validation"]
        # 基线对比:两个内置基线并排 + 差值
        assert [b["baseline_id"] for b in validation["baselines"]] == [
            "buy_and_hold", "equal_weight",
        ]
        for baseline in validation["baselines"]:
            assert baseline["status"] == "ok"
            assert "annual_return" in baseline["metrics"]
            assert "annual_return" in baseline["delta"]
        # OOS 两段指标
        oos = validation["oos"]
        assert oos["available"] is True
        assert oos["in_sample_bars"] + oos["oos_bars"] > 0
        assert "annual_return" in oos["in_sample"]
        assert "annual_return" in oos["oos"]
        # 否决判定:1 个完整回合,capacity_failure 不命中
        assert validation["rejection"]["verdict"] == "passed"
        # 随 metrics JSON 列持久化
        run = db.get(BacktestRun, result["run_id"])
        assert run.metrics["validation"]["rejection"]["verdict"] == "passed"


def test_unknown_baseline_reported_as_unavailable(monkeypatch):
    _patch_bars(monkeypatch, RISE_THEN_FALL)
    with _session() as db:
        created = create_strategy(
            StrategyCreateIn(
                name="未知基线",
                spec=_spec_dict(baseline_ids=["buy_and_hold", "magic_alpha"]),
            ),
            db=db, claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(strategy_id=created["id"], codes=["sh.600519"],
                       start=START, end=END),
            db=db, claims=CLAIMS_A,
        )
        by_id = {b["baseline_id"]: b for b in result["validation"]["baselines"]}
        assert by_id["buy_and_hold"]["status"] == "ok"
        assert by_id["magic_alpha"]["status"] == "unavailable"
        assert "未知基线" in by_id["magic_alpha"]["message"]


def test_create_forces_unverified_and_manual_mark():
    with _session() as db:
        raw = _spec_dict()
        raw["metadata"]["evidence_status"] = "oos_passed"  # 客户端伪造无效
        created = create_strategy(
            StrategyCreateIn(name="状态伪造", spec=raw), db=db, claims=CLAIMS_A,
        )
        assert created["evidence_status"] == "unverified"
        assert created["spec"]["metadata"]["evidence_status"] == "unverified"
        assert created["evidence_actions"] == ["mark_design_complete"]

        updated = update_evidence_status(
            created["id"], StrategyEvidenceIn(action="mark_design_complete"),
            db=db, claims=CLAIMS_A,
        )
        assert updated["evidence_status"] == "design_complete"
        assert updated["evidence_transition"] == {
            "from": "unverified", "to": "design_complete",
        }
        assert updated["evidence_actions"] == []

        # 重复标记:400
        with pytest.raises(HTTPException) as exc_info:
            update_evidence_status(
                created["id"], StrategyEvidenceIn(action="mark_design_complete"),
                db=db, claims=CLAIMS_A,
            )
        assert exc_info.value.status_code == 400


def test_evidence_endpoint_rejects_system_strategy():
    with _session() as db:
        preset = SYSTEM_STRATEGY_SPECS["ma_cross"]
        db.add(Strategy(
            id=1, owner_id="00000000-0000-0000-0000-000000000000",
            is_system=True, name="双均线", template="ma_cross", kind="single",
            params={}, spec=preset, spec_hash=strategy_spec_hash(preset),
            enabled=True,
        ))
        db.commit()
        with pytest.raises(HTTPException) as exc_info:
            update_evidence_status(
                1, StrategyEvidenceIn(action="mark_design_complete"),
                db=db, claims=CLAIMS_A,
            )
        assert exc_info.value.status_code == 403


def test_backtest_auto_advances_and_edit_falls_back(monkeypatch):
    """完整链路:回测自动推进 -> 规格编辑回落 -> 否决命中 -> 人工复位。"""
    _patch_bars(monkeypatch, RISE_THEN_FALL)
    with _session() as db:
        created = create_strategy(
            StrategyCreateIn(name="自动推进", spec=_spec_dict()),
            db=db, claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(strategy_id=created["id"], codes=["sh.600519"],
                       start=START, end=END),
            db=db, claims=CLAIMS_A,
        )
        assert result["evidence_transition"] == {
            "from": "unverified", "to": "oos_passed",
        }
        current = get_strategy(created["id"], db=db, claims=CLAIMS_A)
        assert current["evidence_status"] == "oos_passed"
        # 推进改变了 spec_hash,但刚完成的回测按身份仍算当前规格的证据
        assert current["evidence_backtest_count"] == 1

        # 规格编辑导致身份变化:状态回落到 design_complete
        edited = deepcopy(_spec_dict())
        edited["native_exit"]["condition"]["right"]["value"] = 9.0
        updated = update_strategy(
            created["id"], StrategyPatchIn(spec=edited), db=db, claims=CLAIMS_A,
        )
        assert updated["evidence_status"] == "design_complete"
        assert updated["evidence_backtest_count"] == 0


def test_rejection_hit_marks_strategy_rejected(monkeypatch):
    _patch_bars(monkeypatch, RISE_THEN_FALL)
    with _session() as db:
        created = create_strategy(
            StrategyCreateIn(
                name="否决命中",
                spec=_spec_dict(rejection_rules=[{
                    "metric": "annual_return", "op": "lt", "threshold": 10.0,
                    "segment": "full", "description": "年化低于 1000% 则否决",
                }]),
            ),
            db=db, claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(strategy_id=created["id"], codes=["sh.600519"],
                       start=START, end=END),
            db=db, claims=CLAIMS_A,
        )
        rejection = result["validation"]["rejection"]
        assert rejection["verdict"] == "rejected"
        assert rejection["hits"][0]["metric"] == "annual_return"
        assert result["evidence_transition"] == {
            "from": "unverified", "to": "rejected",
        }

        current = get_strategy(created["id"], db=db, claims=CLAIMS_A)
        assert current["evidence_status"] == "rejected"
        assert current["evidence_actions"] == ["reset_rejected"]

        reset = update_evidence_status(
            created["id"], StrategyEvidenceIn(action="reset_rejected"),
            db=db, claims=CLAIMS_A,
        )
        assert reset["evidence_status"] == "design_complete"


def test_declared_sweep_api(monkeypatch):
    """按规格声明执行扫描:结果带 spec_hash、声明回显与稳定性评估。"""
    _patch_bars(monkeypatch, RISE_THEN_FALL)
    scans = [{"path": "$.native_exit.condition.right.value", "values": [7.0, 8.0, 9.0]}]
    with _session() as db:
        created = create_strategy(
            StrategyCreateIn(name="声明扫描", spec=_spec_dict(parameter_scans=scans)),
            db=db, claims=CLAIMS_A,
        )
        result = sweep(
            SweepIn(strategy_id=created["id"], codes=["sh.600519"],
                    start=START, end=END, declared=True),
            db=db, claims=CLAIMS_A,
        )
        assert result["declared"] is True
        assert result["declared_scans"][0]["path"] == "$.native_exit.condition.right.value"
        assert result["strategy_spec_hash"] == created["spec_hash"]
        assert len(result["results"]) == 3
        # 当前参数(退出阈值 8.0)在候选值中:稳定性可评估
        assert result["stability"]["status"] == "evaluated"
        assert result["stability"]["current_params"] == {
            "$.native_exit.condition.right.value": 8.0,
        }

        # 未声明扫描的策略:400
        plain = create_strategy(
            StrategyCreateIn(name="无声明", spec=_spec_dict()),
            db=db, claims=CLAIMS_A,
        )
        with pytest.raises(HTTPException) as exc_info:
            sweep(
                SweepIn(strategy_id=plain["id"], codes=["sh.600519"],
                        start=START, end=END, declared=True),
                db=db, claims=CLAIMS_A,
            )
        assert exc_info.value.status_code == 400
