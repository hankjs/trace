"""策略 CRUD 的行为断言。

重点覆盖归属与配额契约(与 test_api_pools.py 同构):
1. 公共策略全用户可见、不可改不可删;
2. 自建策略按 owner_id 隔离,跨用户访问按 404 处理(不能靠状态码枚举);
3. 参数按模板元数据校验,kind 由模板决定而非客户端传入;
4. 被回测引用的策略不可删(外键 RESTRICT),引导改用停用;
5. 数量与启用数配额。
"""
from __future__ import annotations

import sys
from copy import deepcopy
from datetime import date
from pathlib import Path

import pandas as pd
import pytest
from fastapi import BackgroundTasks, HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.strategies import (StrategyCreateIn, StrategyDuplicateIn,
                                StrategyPatchIn, create_strategy,
                                delete_strategy, duplicate_strategy,
                                get_strategy, list_strategies, list_templates,
                                update_strategy)
from app.api.backtest import BacktestIn, create_backtest
from app.db import Base
from app.models import SYSTEM_OWNER_ID, BacktestRun, Strategy
from app.strategy.presets import SYSTEM_STRATEGY_SPECS, get_preset_spec
from app.strategy.spec import strategy_spec_hash
from app.strategy.store import MAX_ENABLED_PER_USER, MAX_STRATEGIES_PER_USER
from app.strategy.strategies import REGISTRY

USER_A = "11111111-1111-1111-1111-111111111111"
USER_B = "22222222-2222-2222-2222-222222222222"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}
CLAIMS_B = {"sub": USER_B, "username": "b", "can_client": True}


def _create_in(name: str, template: str = "ma_cross", *,
               params: dict | None = None, enabled: bool = True,
               **kwargs) -> StrategyCreateIn:
    """测试辅助:从预置模板构造完整 StrategyCreateIn(创建路径已要求 spec)。"""
    spec = get_preset_spec(template, params).model_dump(mode="json")
    return StrategyCreateIn(name=name, spec=spec, enabled=enabled, **kwargs)

# 对齐 Alembic 0014 的完整 StrategySpec seed
PRESETS = [
    (1, "ma_cross", "single", "双均线趋势策略"),
    (2, "breakout", "single", "价格突破策略"),
    (3, "mean_reversion", "single", "上升趋势中的超跌反弹策略"),
    (4, "volume_breakout", "single", "缩量整理后的放量突破策略"),
    (5, "momentum_rotation", "portfolio", "强势股票轮动策略"),
    (6, "multifactor_hold", "portfolio", "多指标综合评分持有策略"),
]


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed(db: Session) -> None:
    db.add_all([
        Strategy(id=sid, owner_id=SYSTEM_OWNER_ID, is_system=True, name=name,
                 template=template, kind=kind, params={},
                 spec=SYSTEM_STRATEGY_SPECS[template],
                 spec_hash=strategy_spec_hash(SYSTEM_STRATEGY_SPECS[template]),
                 enabled=True)
        for sid, template, kind, name in PRESETS
    ])
    db.commit()


def _volume_confirmed_breakout_spec() -> dict:
    raw = get_preset_spec(
        "breakout", {"entry": 20, "exit": 10},
    ).model_dump(mode="json")
    raw["metadata"].update({
        "canonical_id": "USER-VOLUME-BREAKOUT-20-10",
        "sources": [{"book": "用户研究", "candidate_id": "volume-breakout"}],
        "hypothesis": "20 日价格突破且成交量放大时进入，跌破 10 日低点退出。",
    })
    raw["data_requirements"].append({
        "field": "volume", "availability": "daily_close", "required": True,
    })
    price_breakout = deepcopy(raw["entry"]["condition"])
    raw["entry"] = {
        "reason_code": "volume_confirmed_breakout",
        "condition": {
            "op": "all",
            "args": [
                price_breakout,
                {
                    "op": "gt",
                    "left": {"op": "field", "name": "volume"},
                    "right": {
                        "op": "multiply",
                        "left": {"op": "literal", "value": 1.5},
                        "right": {
                            "op": "rolling_mean",
                            "input": {"op": "field", "name": "volume"},
                            "window": 20,
                            "shift": 1,
                        },
                    },
                },
            ],
        },
    }
    return raw


def test_templates_cover_all_code_modules():
    """模板列表与代码注册表一致,且每个模板都有参数元数据。"""
    result = list_templates()
    assert {item["key"] for item in result["items"]} == set(REGISTRY)
    assert all(item["params"] for item in result["items"])


def test_preset_strategies_are_visible_and_readonly_to_everyone():
    with _session() as db:
        _seed(db)
        for claims in (CLAIMS_A, CLAIMS_B):
            listed = list_strategies(db=db, claims=claims)
            assert listed["count"] == len(PRESETS)
            assert all(item["is_system"] for item in listed["items"])
            # 公共策略对任何人都不可编辑
            assert not any(item["editable"] for item in listed["items"])

        with pytest.raises(HTTPException) as exc:
            update_strategy(1, StrategyPatchIn(name="改个名"), db=db,
                            claims=CLAIMS_A)
        assert exc.value.status_code == 403
        with pytest.raises(HTTPException) as exc:
            delete_strategy(1, db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 403

        # 无副作用
        assert db.get(Strategy, 1).name == "双均线趋势策略"


def test_create_requires_full_strategy_spec():
    """创建路径只接受完整 StrategySpec;params 列不再作为事实来源。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            _create_in("我的双均线", "ma_cross", params={"fast": 10}),
            db=db, claims=CLAIMS_A,
        )
    assert created["params"] == {} or created["params"] is None or created["params"] == {}
    assert created["spec"]["kind"] == "single"
    assert created["editable"] is True
    # 预置映射到 template 标签时可能仍回显系统名
    assert created["kind"] == "single"


def test_kind_comes_from_spec_not_client():
    """kind 由规格决定:否则组合策略能被标成单标的而进按个股跑的信号引擎。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            _create_in("我的轮动", "momentum_rotation"),
            db=db, claims=CLAIMS_A,
        )
    assert created["kind"] == "portfolio"


def test_invalid_spec_is_rejected():
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(
                    name="坏规格",
                    spec={"kind": "single", "schema_version": 1},
                ),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        with pytest.raises(ValueError):
            _create_in("坏模板", "not_a_template")

        assert db.execute(select(Strategy).where(
            Strategy.is_system.is_(False))).scalars().all() == []


def test_custom_strategy_is_invisible_to_other_users():
    """跨用户一律 404 而非 403:否则可靠状态码枚举他人建了哪些策略。"""
    with _session() as db:
        _seed(db)
        mine = create_strategy(
            _create_in("A 的策略", "ma_cross", params={"fast": 3}),
            db=db, claims=CLAIMS_A,
        )
        sid = mine["id"]

        # B 的列表里只有公共策略
        listed_b = list_strategies(db=db, claims=CLAIMS_B)
        assert [item["id"] for item in listed_b["items"]] == [1, 2, 3, 4, 5, 6]

        for call in (
            lambda: get_strategy(sid, db=db, claims=CLAIMS_B),
            lambda: update_strategy(sid, StrategyPatchIn(name="劫持"),
                                    db=db, claims=CLAIMS_B),
            lambda: delete_strategy(sid, db=db, claims=CLAIMS_B),
        ):
            with pytest.raises(HTTPException) as exc:
                call()
            assert exc.value.status_code == 404

        # 未被改动
        row = db.get(Strategy, sid)
        assert row.name == "A 的策略"
        assert row.owner_id == USER_A


def test_same_name_allowed_across_users_and_rejected_within_user():
    with _session() as db:
        _seed(db)
        body = _create_in("同名策略", "ma_cross")
        create_strategy(body, db=db, claims=CLAIMS_A)
        # 换个用户可以同名
        create_strategy(body, db=db, claims=CLAIMS_B)
        # 同一用户重复则 409
        with pytest.raises(HTTPException) as exc:
            create_strategy(body, db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409


def test_duplicate_makes_editable_copy_owned_by_caller():
    """公共策略只读,「另存为我的策略」后才能改规格。"""
    with _session() as db:
        _seed(db)
        copy = duplicate_strategy(
            1, StrategyDuplicateIn(name="我的双均线"),
            db=db, claims=CLAIMS_A,
        )
    assert copy["owner_id"] == USER_A
    assert copy["is_system"] is False
    assert copy["editable"] is True
    assert copy["kind"] == "single"
    assert copy["spec"]["kind"] == "single"


def test_duplicate_without_name_suffixes_original():
    with _session() as db:
        _seed(db)
        copy = duplicate_strategy(1, StrategyDuplicateIn(), db=db,
                                  claims=CLAIMS_A)
    assert copy["name"] == "双均线趋势策略 副本"


def test_update_changes_name_spec_and_enabled():
    with _session() as db:
        _seed(db)
        created = create_strategy(
            _create_in("待改", "ma_cross"),
            db=db, claims=CLAIMS_A,
        )
        new_spec = get_preset_spec("breakout").model_dump(mode="json")
        updated = update_strategy(
            created["id"],
            StrategyPatchIn(name="改好了", spec=new_spec, enabled=False),
            db=db, claims=CLAIMS_A,
        )
    assert updated["name"] == "改好了"
    assert updated["kind"] == "single"
    assert updated["spec"]["metadata"]["canonical_id"] == new_spec["metadata"]["canonical_id"]
    assert updated["enabled"] is False


def test_strategy_used_by_backtest_cannot_be_deleted():
    """外键 RESTRICT:回测是用户资产,不能因删策略而静默消失。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            _create_in("跑过回测的", "ma_cross"),
            db=db, claims=CLAIMS_A,
        )
        db.add(BacktestRun(
            user_id=USER_A, strategy_id=created["id"], params={},
            codes=["sh.600519"], start=date(2024, 1, 1), end=date(2024, 6, 30),
            metrics={"total_return": 0.1},
        ))
        db.commit()

        listed = list_strategies(db=db, claims=CLAIMS_A)
        mine = next(i for i in listed["items"] if i["id"] == created["id"])
        assert mine["backtest_count"] == 1

        with pytest.raises(HTTPException) as exc:
            delete_strategy(created["id"], db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409
        assert "停用" in exc.value.detail
        assert db.get(Strategy, created["id"]) is not None

        # 停用是可行的替代路径
        assert update_strategy(created["id"], StrategyPatchIn(enabled=False),
                               db=db, claims=CLAIMS_A)["enabled"] is False


def test_unused_strategy_can_be_deleted():
    with _session() as db:
        _seed(db)
        created = create_strategy(
            _create_in("没用过的", "breakout"),
            db=db, claims=CLAIMS_A,
        )
        assert delete_strategy(created["id"], db=db, claims=CLAIMS_A) == {
            "deleted": 1, "id": created["id"]}
        assert db.get(Strategy, created["id"]) is None


def test_enabled_quota_is_enforced():
    """启用数上限:每个启用的策略每天都要乘进全市场股票数。"""
    with _session() as db:
        _seed(db)
        for i in range(MAX_ENABLED_PER_USER):
            create_strategy(
                _create_in(f"启用{i}", "ma_cross", enabled=True),
                db=db, claims=CLAIMS_A,
            )
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                _create_in("再来一个", "ma_cross", enabled=True),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        assert str(MAX_ENABLED_PER_USER) in exc.value.detail

        # 停用的不占启用配额
        stopped = create_strategy(
            _create_in("停用的", "ma_cross", enabled=False),
            db=db, claims=CLAIMS_A,
        )
        assert stopped["enabled"] is False
        # 但启用它会被拦住
        with pytest.raises(HTTPException) as exc:
            update_strategy(stopped["id"], StrategyPatchIn(enabled=True),
                            db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 400


def test_total_quota_is_enforced():
    with _session() as db:
        _seed(db)
        db.add_all([
            Strategy(owner_id=USER_A, is_system=False, name=f"批量{i}",
                     template="ma_cross", kind="single", params={},
                     enabled=False)
            for i in range(MAX_STRATEGIES_PER_USER)
        ])
        db.commit()
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                _create_in("超额", "ma_cross", enabled=False),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        assert str(MAX_STRATEGIES_PER_USER) in exc.value.detail


def test_quota_counts_only_own_strategies():
    """公共策略与别人的策略都不占我的配额。"""
    with _session() as db:
        _seed(db)
        db.add_all([
            Strategy(owner_id=USER_B, is_system=False, name=f"B 的{i}",
                     template="ma_cross", kind="single", params={},
                     enabled=True)
            for i in range(MAX_ENABLED_PER_USER)
        ])
        db.commit()
        # A 仍然可以建并启用
        created = create_strategy(
            _create_in("A 的第一个", "ma_cross"),
            db=db, claims=CLAIMS_A,
        )
        assert created["enabled"] is True


def test_dynamic_volume_breakout_edit_keeps_prior_backtest_immutable(monkeypatch):
    start = date(2024, 1, 1)
    dates = pd.bdate_range(start, periods=100)
    prices = [10 + index * 0.05 for index in range(len(dates))]
    frame = pd.DataFrame({
        "date": dates.date,
        "open": prices,
        "high": [value * 1.01 for value in prices],
        "low": [value * 0.99 for value in prices],
        "close": prices,
        "raw_close": prices,
        "volume": [1_000_000.0] * 99 + [2_000_000.0],
        "amount": [10_000_000.0] * len(dates),
        "is_st": [False] * len(dates),
    })
    monkeypatch.setattr(
        "app.backtest.engine.load_bars_df",
        lambda db, code, start=None, end=None, **kwargs: frame,
    )

    with _session() as db:
        _seed(db)
        original_spec = _volume_confirmed_breakout_spec()
        created = create_strategy(
            StrategyCreateIn(
                name="20 日放量突破 10 日退出",
                spec=original_spec,
                enabled=True,
            ),
            db=db,
            claims=CLAIMS_A,
        )
        first = create_backtest(
            BacktestIn(
                strategy_id=created["id"],
                codes=["sh.600519"],
                start=dates[0].date(),
                end=dates[-1].date(),
            ),
            background_tasks=BackgroundTasks(), db=db,
            claims=CLAIMS_A,
        )
        first_run = db.get(BacktestRun, first["run_id"])
        first_snapshot = deepcopy(first_run.strategy_spec_snapshot)
        first_hash = first_run.strategy_spec_hash
        first_execution = first_run.execution_fingerprint
        before_edit = get_strategy(created["id"], db=db, claims=CLAIMS_A)

        edited_spec = deepcopy(original_spec)
        edited_spec["native_exit"]["condition"]["right"]["window"] = 8
        updated = update_strategy(
            created["id"],
            StrategyPatchIn(spec=edited_spec),
            db=db,
            claims=CLAIMS_A,
        )
        after_edit = get_strategy(created["id"], db=db, claims=CLAIMS_A)
        second = create_backtest(
            BacktestIn(
                strategy_id=created["id"],
                codes=["sh.600519"],
                start=dates[0].date(),
                end=dates[-1].date(),
            ),
            background_tasks=BackgroundTasks(),
            db=db,
            claims=CLAIMS_A,
        )

        assert created["template"] == "strategy_spec"
        assert created["enabled"] is True
        assert first_snapshot == original_spec
        assert first_hash == created["spec_hash"]
        assert first_execution == first["execution_fingerprint"]
        assert before_edit["evidence_backtest_count"] == 1
        assert first_run.strategy_spec_snapshot == first_snapshot
        assert first_run.execution_fingerprint == first_execution
        assert updated["spec_hash"] != first_hash
        assert after_edit["backtest_count"] == 1
        assert after_edit["evidence_backtest_count"] == 0
        assert second["strategy_spec_hash"] == updated["spec_hash"]
        assert second["execution_fingerprint"] != first_execution


def test_backtest_request_freezes_spec_before_execution_starts(monkeypatch):
    original_spec = _volume_confirmed_breakout_spec()
    edited_spec = deepcopy(original_spec)
    edited_spec["native_exit"]["condition"]["right"]["window"] = 7
    captured: dict = {}

    def fake_run_backtest(db, strategy, codes, start, end, *args, **kwargs):
        captured["spec"] = kwargs["execution_spec"].model_dump(mode="json")
        # 模拟任务排队后策略被原地修改；传入执行器的快照不能随 ORM 行变化。
        strategy.spec = deepcopy(edited_spec)
        return {"codes": codes, "strategy_spec_hash": strategy_spec_hash(captured["spec"])}

    monkeypatch.setattr("app.api.backtest.run_backtest", fake_run_backtest)
    with _session() as db:
        _seed(db)
        created = create_strategy(
            StrategyCreateIn(name="请求入口快照", spec=original_spec),
            db=db,
            claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(
                strategy_id=created["id"],
                codes=["sh.600519"],
                start=date(2024, 1, 1),
                end=date(2024, 6, 30),
            ),
            background_tasks=BackgroundTasks(),
            db=db,
            claims=CLAIMS_A,
        )

    assert captured["spec"] == original_spec
    assert result["strategy_spec_hash"] == strategy_spec_hash(original_spec)
    assert captured["spec"] != edited_spec
