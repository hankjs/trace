"""研究计划保留策略测试。"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.db import Base
from app.models import ResearchPlan, ResearchPlanItem, Signal, Strategy
from app.research_plan.retention import KEEP_LATEST_PER_CHAIN, prune_research_plans

USER = "11111111-1111-1111-1111-111111111111"


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _plan(strategy_id: int, plan_id: int, *, code: str | None = "sh.600000",
          pool_id: int | None = None, plan_type: str = "single") -> ResearchPlan:
    """构造一版最小研究计划;id 由调用方显式指定以控制版本顺序。"""
    return ResearchPlan(
        id=plan_id,
        owner_id=USER,
        strategy_is_system=False,
        strategy_id=strategy_id,
        strategy_name=f"策略{strategy_id}",
        template="breakout",
        strategy_kind="single",
        strategy_version="v1",
        params_snapshot={},
        strategy_spec_snapshot={},
        strategy_spec_hash="0" * 64,
        plan_type=plan_type,
        code=code,
        pool_id=pool_id,
        data_date=date(2026, 7, 24),
        generated_at=date(2026, 7, 24),
        next_execution_date=date(2026, 7, 27),
        valid_until=date(2026, 7, 27),
        signal_type="buy",
        status="current",
        status_reason={"code": "fresh_daily_signal"},
        price_adjustment="forward",
        signal_price=10.0,
        entry_observation={},
        risk_rules=[],
        take_profit={},
        native_exit=[],
        exit_hits=[],
        portfolio_summary=None,
        backtest_run_id=None,
        backtest_evidence={"status": "unverified"},
        product_boundary="边界",
        revision=1,
        supersedes_plan_id=None,
    )


def _portfolio_plan(strategy_id: int, plan_id: int, pool_id: int) -> ResearchPlan:
    plan = _plan(strategy_id, plan_id, code=None, pool_id=pool_id,
                 plan_type="portfolio_rebalance")
    plan.signal_type = "rebalance"
    return plan


def test_prune_keeps_latest_30_and_deletes_older_versions():
    with _session() as db:
        db.add(Strategy(id=1, owner_id=USER, is_system=False, name="策略1",
                        template="breakout", kind="single", params={},
                        enabled=True))
        plans = [_plan(1, i) for i in range(1, 36)]
        db.add_all(plans)
        db.commit()

        result = prune_research_plans(db)
        db.commit()

        remaining = db.execute(
            select(ResearchPlan.id).where(ResearchPlan.strategy_id == 1)
            .order_by(ResearchPlan.id)
        ).scalars().all()

        assert result["chains"] == 1
        assert result["candidates"] == 5
        assert result["deleted"] == 5
        assert result["protected_kept"] == 0
        assert remaining == list(range(6, 36))


def test_prune_keeps_protected_plan_beyond_limit():
    with _session() as db:
        db.add(Strategy(id=1, owner_id=USER, is_system=False, name="策略1",
                        template="breakout", kind="single", params={},
                        enabled=True))
        plans = [_plan(1, i) for i in range(1, 36)]
        db.add_all(plans)
        # 引用第 3 版(属于应删除的 5 个最旧版本之一)
        db.add(Signal(
            code="sh.600000", date=date(2026, 7, 24), strategy_id=1,
            side="buy", price=10.0, plan_id=3,
        ))
        db.commit()

        result = prune_research_plans(db)
        db.commit()

        remaining = set(db.execute(
            select(ResearchPlan.id).where(ResearchPlan.strategy_id == 1)
        ).scalars().all())

        assert result["chains"] == 1
        assert result["candidates"] == 5
        assert result["deleted"] == 4
        assert result["protected_kept"] == 1
        # 保留 id 最大的 30 个,外加被信号引用的旧版 3
        assert remaining == set(range(6, 36)) | {3}


def test_prune_deletes_research_plan_items_before_plans():
    with _session() as db:
        db.add(Strategy(id=1, owner_id=USER, is_system=False, name="策略1",
                        template="breakout", kind="single", params={},
                        enabled=True))
        plans = [_portfolio_plan(1, i, pool_id=7) for i in range(1, 36)]
        db.add_all(plans)
        items = [
            ResearchPlanItem(
                plan_id=i, code="sh.600000", previous_weight=0.0,
                target_weight=0.5, change_type="added", eligible=True,
                reasons=[],
            )
            for i in range(1, 36)
        ]
        db.add_all(items)
        db.commit()

        prune_research_plans(db)
        db.commit()

        remaining_plan_ids = set(db.execute(
            select(ResearchPlan.id).where(ResearchPlan.strategy_id == 1)
        ).scalars().all())
        remaining_item_plan_ids = set(db.execute(
            select(ResearchPlanItem.plan_id).distinct()
        ).scalars().all())

        assert remaining_plan_ids == set(range(6, 36))
        assert remaining_item_plan_ids == remaining_plan_ids


def test_prune_groups_include_null_code_and_pool_id():
    """链键中的 None 是合法分组值,不能把 NULL 行当异常丢。"""
    with _session() as db:
        db.add_all([
            Strategy(id=1, owner_id=USER, is_system=False, name="策略1",
                     template="breakout", kind="single", params={}, enabled=True),
            Strategy(id=2, owner_id=USER, is_system=False, name="策略2",
                     template="momentum_rotation", kind="portfolio", params={},
                     enabled=True),
        ])
        single_plans = [_plan(1, i, code="sh.600000") for i in range(1, 36)]
        portfolio_plans = [
            _portfolio_plan(2, i, pool_id=None) for i in range(36, 41)
        ]
        db.add_all(single_plans + portfolio_plans)
        db.commit()

        result = prune_research_plans(db)
        db.commit()

        single_remaining = db.execute(
            select(ResearchPlan.id).where(ResearchPlan.strategy_id == 1)
            .order_by(ResearchPlan.id)
        ).scalars().all()
        portfolio_remaining = db.execute(
            select(ResearchPlan.id).where(ResearchPlan.strategy_id == 2)
            .order_by(ResearchPlan.id)
        ).scalars().all()

        assert result["chains"] == 2
        assert single_remaining == list(range(6, 36))
        # 组合链只有 5 版,低于保留阈值,原样保留
        assert portfolio_remaining == list(range(36, 41))

