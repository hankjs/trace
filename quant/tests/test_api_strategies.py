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
from datetime import date
from pathlib import Path

import pytest
from fastapi import HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.strategies import (StrategyCreateIn, StrategyDuplicateIn,
                                StrategyPatchIn, create_strategy,
                                delete_strategy, duplicate_strategy,
                                get_strategy, list_strategies, list_templates,
                                update_strategy)
from app.db import Base
from app.models import SYSTEM_OWNER_ID, BacktestRun, Strategy
from app.strategy.store import MAX_ENABLED_PER_USER, MAX_STRATEGIES_PER_USER
from app.strategy.strategies import REGISTRY

USER_A = "11111111-1111-1111-1111-111111111111"
USER_B = "22222222-2222-2222-2222-222222222222"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}
CLAIMS_B = {"sub": USER_B, "username": "b", "can_client": True}

# 对齐 Alembic 0012 的 seed
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
                 template=template, kind=kind, params={}, enabled=True)
        for sid, template, kind, name in PRESETS
    ])
    db.commit()


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


def test_effective_params_merge_template_defaults():
    """params 只存用户覆盖的键,effective_params 是实际生效的全量参数。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            StrategyCreateIn(name="我的双均线", template="ma_cross",
                             params={"fast": 10}),
            db=db, claims=CLAIMS_A,
        )
    assert created["params"] == {"fast": 10}
    assert created["effective_params"]["fast"] == 10
    assert created["effective_params"]["slow"] == 20
    assert created["effective_params"]["risk_overlay"]["enabled"] is False
    assert created["effective_params"]["take_profit"]["enabled"] is False
    assert created["params_valid"] is True
    assert created["editable"] is True
    assert created["template_name"] == "双均线趋势策略"


def test_kind_comes_from_template_not_client():
    """kind 由模板决定:否则组合策略能被标成单标的而进按个股跑的信号引擎。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            StrategyCreateIn(name="我的轮动", template="momentum_rotation"),
            db=db, claims=CLAIMS_A,
        )
    assert created["kind"] == "portfolio"


def test_invalid_params_and_template_are_rejected():
    with _session() as db:
        _seed(db)
        # 跨参数约束(fast < slow)
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="坏参数", template="ma_cross",
                                 params={"fast": 30, "slow": 10}),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        # 超出上下界
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="越界", template="ma_cross",
                                 params={"fast": 99999}),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        # 不认识的参数键
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="乱参数", template="ma_cross",
                                 params={"future_window": 3}),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        # 不存在的模板
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="坏模板", template="not_a_template"),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400

        assert db.execute(select(Strategy).where(
            Strategy.is_system.is_(False))).scalars().all() == []


def test_custom_strategy_is_invisible_to_other_users():
    """跨用户一律 404 而非 403:否则可靠状态码枚举他人建了哪些策略。"""
    with _session() as db:
        _seed(db)
        mine = create_strategy(
            StrategyCreateIn(name="A 的策略", template="ma_cross",
                             params={"fast": 3}),
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
        body = StrategyCreateIn(name="同名策略", template="ma_cross")
        create_strategy(body, db=db, claims=CLAIMS_A)
        # 换个用户可以同名
        create_strategy(body, db=db, claims=CLAIMS_B)
        # 同一用户重复则 409
        with pytest.raises(HTTPException) as exc:
            create_strategy(body, db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409


def test_duplicate_makes_editable_copy_owned_by_caller():
    """公共策略只读,「另存为我的策略」后才能调参。"""
    with _session() as db:
        _seed(db)
        copy = duplicate_strategy(
            1, StrategyDuplicateIn(name="我的双均线", params={"fast": 8}),
            db=db, claims=CLAIMS_A,
        )
    assert copy["owner_id"] == USER_A
    assert copy["is_system"] is False
    assert copy["editable"] is True
    assert copy["template"] == "ma_cross"
    assert copy["effective_params"]["fast"] == 8
    assert copy["effective_params"]["slow"] == 20
    assert copy["effective_params"]["risk_overlay"]["enabled"] is False
    assert copy["effective_params"]["take_profit"]["enabled"] is False


def test_duplicate_without_name_suffixes_original():
    with _session() as db:
        _seed(db)
        copy = duplicate_strategy(1, StrategyDuplicateIn(), db=db,
                                  claims=CLAIMS_A)
    assert copy["name"] == "双均线趋势策略 副本"


def test_update_changes_name_params_and_enabled():
    with _session() as db:
        _seed(db)
        created = create_strategy(
            StrategyCreateIn(name="待改", template="ma_cross"),
            db=db, claims=CLAIMS_A,
        )
        updated = update_strategy(
            created["id"],
            StrategyPatchIn(name="改好了", params={"fast": 7}, enabled=False),
            db=db, claims=CLAIMS_A,
        )
    assert updated["name"] == "改好了"
    assert updated["effective_params"]["fast"] == 7
    assert updated["effective_params"]["slow"] == 20
    assert updated["effective_params"]["risk_overlay"]["enabled"] is False
    assert updated["effective_params"]["take_profit"]["enabled"] is False
    assert updated["enabled"] is False


def test_strategy_used_by_backtest_cannot_be_deleted():
    """外键 RESTRICT:回测是用户资产,不能因删策略而静默消失。"""
    with _session() as db:
        _seed(db)
        created = create_strategy(
            StrategyCreateIn(name="跑过回测的", template="ma_cross"),
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
            StrategyCreateIn(name="没用过的", template="breakout"),
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
                StrategyCreateIn(name=f"启用{i}", template="ma_cross",
                                 enabled=True),
                db=db, claims=CLAIMS_A,
            )
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="再来一个", template="ma_cross",
                                 enabled=True),
                db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 400
        assert str(MAX_ENABLED_PER_USER) in exc.value.detail

        # 停用的不占启用配额
        stopped = create_strategy(
            StrategyCreateIn(name="停用的", template="ma_cross", enabled=False),
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
                StrategyCreateIn(name="超额", template="ma_cross",
                                 enabled=False),
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
            StrategyCreateIn(name="A 的第一个", template="ma_cross"),
            db=db, claims=CLAIMS_A,
        )
        assert created["enabled"] is True
