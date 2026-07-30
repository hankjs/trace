"""实验注册表与 trial 账本。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from fastapi import HTTPException
from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.experiments import (
    ExperimentCreateIn,
    TrialBatchCreateIn,
    TrialCreateIn,
    api_archive_experiment,
    api_create_experiment,
    api_create_trial,
    api_create_trials_batch,
    api_delete_trial_denied,
    api_get_experiment,
    api_list_experiments,
)
from app.db import Base
from app.models import DailyBar, Stock, Strategy, SYSTEM_OWNER_ID
from app.strategy.presets import get_preset_spec
from app.strategy.spec import strategy_spec_hash

CLAIMS = {
    "sub": "user-a",
    "username": "a",
    "can_admin": False,
    "can_client": True,
}

START = date(2024, 1, 2)
END = date(2024, 6, 28)


def _session():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    return sessionmaker(bind=engine)()


def _seed(db: Session) -> Strategy:
    db.add(Stock(
        code="sh.600519", name="茅台", list_date=date(2015, 1, 1), is_st=False,
    ))
    d = START - timedelta(days=250)
    price = 10.0
    while d <= END:
        price *= 1.001
        db.add(DailyBar(
            code="sh.600519", date=d,
            open=price, high=price * 1.01, low=price * 0.99, close=price,
            raw_close=price, volume=1e6, amount=1e7, is_st=False,
        ))
        d += timedelta(days=1)
    spec = get_preset_spec("breakout")
    strategy = Strategy(
        owner_id="user-a", is_system=False, name="突破试验",
        template="breakout", kind="single", params={},
        spec=spec.model_dump(mode="json"),
        spec_hash=strategy_spec_hash(spec),
        research_status="unverified", enabled=True,
    )
    db.add(strategy)
    db.commit()
    db.refresh(strategy)
    return strategy


def test_experiment_trial_ledger_keeps_failures():
    with _session() as db:
        strategy = _seed(db)
        created = api_create_experiment(
            ExperimentCreateIn(
                title="突破族",
                hypothesis="突破后延续",
                permanent_candidate_id="CAN-TEST-01",
                spec=strategy.spec,
                strategy_id=strategy.id,
            ),
            db=db, claims=CLAIMS,
        )
        assert created["status"] == "design"
        assert created["permanent_candidate_id"] == "CAN-TEST-01"

        ok = api_create_trial(
            created["id"],
            TrialCreateIn(
                codes=["sh.600519"], start=START, end=END,
            ),
            db=db, claims=CLAIMS,
        )
        assert ok["trial"]["outcome"] in {"ok", "no_trades", "rejected"}
        assert ok["trial"]["backtest_run_id"] is not None

        # 错误 patch 也必须落账本
        bad = api_create_trial(
            created["id"],
            TrialCreateIn(
                codes=["sh.600519"], start=START, end=END,
                param_patch={"$.not.exist": 1},
            ),
            db=db, claims=CLAIMS,
        )
        assert bad["trial"]["outcome"] == "error"
        assert bad["trial"]["error"]

        detail = api_get_experiment(created["id"], db=db, claims=CLAIMS)
        assert detail["trial_count"] == 2
        assert len(detail["trials"]) == 2
        assert detail["multiplicity"]["n_trials"] >= 1
        assert "disclaimer" in detail["multiplicity"]

        listed = api_list_experiments(db=db, claims=CLAIMS)
        assert listed["count"] == 1

        with pytest.raises(HTTPException) as exc:
            api_delete_trial_denied(
                created["id"], bad["trial"]["id"], claims=CLAIMS,
            )
        assert exc.value.status_code == 405

        archived = api_archive_experiment(created["id"], db=db, claims=CLAIMS)
        assert archived["status"] == "archived"


def test_list_experiments_honors_limit_and_offset():
    with _session() as db:
        strategy = _seed(db)
        for i in range(4):
            api_create_experiment(
                ExperimentCreateIn(
                    title=f"实验 {i}",
                    hypothesis="分页测试",
                    permanent_candidate_id=f"CAN-PAG-{i:03d}",
                    spec=strategy.spec,
                    strategy_id=strategy.id,
                ),
                db=db, claims=CLAIMS,
            )

        page1 = api_list_experiments(
            limit=2, offset=0, db=db, claims=CLAIMS,
        )
        page2 = api_list_experiments(
            limit=2, offset=2, db=db, claims=CLAIMS,
        )

        assert len(page1["items"]) == 2
        assert len(page2["items"]) == 2
        ids1 = {item["id"] for item in page1["items"]}
        ids2 = {item["id"] for item in page2["items"]}
        assert not ids1 & ids2


def test_batch_trials_persist_mixed_success_and_failure():
    """B5: 批量创建含失败项全部落库。"""
    with _session() as db:
        strategy = _seed(db)
        created = api_create_experiment(
            ExperimentCreateIn(
                title="批处理",
                hypothesis="批量 trial 账本",
                permanent_candidate_id="CAN-BATCH-01",
                spec=strategy.spec,
                strategy_id=strategy.id,
            ),
            db=db, claims=CLAIMS,
        )
        patches = [
            {},
            {"$.native_exit.condition.right.window": 5},
            {"$.not.exist": 1},
            {"$.holding.cooldown_days": 2},
            {"$.also.missing": 9},
            {},
            {"$.holding.cooldown_days": 3},
            {"$.holding.cooldown_days": 4},
            {"$.bad.path": 0},
            {"$.holding.cooldown_days": 5},
        ]
        assert len(patches) == 10
        result = api_create_trials_batch(
            created["id"],
            TrialBatchCreateIn(
                codes=["sh.600519"], start=START, end=END,
                param_patches=patches,
            ),
            db=db, claims=CLAIMS,
        )
        assert result["count"] == 10
        assert len(result["items"]) == 10
        outcomes = [item["trial"]["outcome"] for item in result["items"]]
        assert any(o == "error" for o in outcomes)
        detail = api_get_experiment(created["id"], db=db, claims=CLAIMS)
        assert detail["trial_count"] == 10
        assert len(detail["trials"]) == 10
