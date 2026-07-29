"""股票池组 CRUD 的行为断言。

重点覆盖四个契约点与隔离约束:
1. 预置池 members 返回**当日解析出的成分**(空列表会让前端「另存为」静默建空池);
2. 预置池不可改名/不可删/不可增删成员;
3. 自定义池按 user_id 隔离,跨用户访问按 404 处理;
4. 批量导入部分成功(未入库代码进 skipped),create 带 codes 一步建池;
5. 仍被回测/评估/研究计划引用的池不可删(409,与策略删除的保护同款)。
"""
from __future__ import annotations

import sys
from datetime import date, datetime, timedelta
from pathlib import Path

import pytest
from fastapi import BackgroundTasks, HTTPException

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine, delete, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.backtest import BacktestIn, create_backtest, get_backtest
from app.api.pools import (add_pool_members, create_pool, default_pool,
                           delete_pool, get_pool, list_pool_members,
                           list_pools, remove_pool_member, update_pool,
                           PoolCreateIn, PoolMembersIn, PoolPatchIn)
from app.db import Base
from app.models import (SYSTEM_OWNER_ID, BacktestRun, DailyBar, IndexMember,
                        Pool, PoolMember, ResearchPlan, Stock, Strategy,
                        StrategyEval)
from app.strategy.presets import get_preset_spec
from app.strategy.spec import strategy_spec_hash

# 回测用的公共策略(对齐 Alembic 0012 的 seed):单标的 1 号、组合 5 号
MA_CROSS_ID = 1
ROTATION_ID = 5

USER_A = "11111111-1111-1111-1111-111111111111"
USER_B = "22222222-2222-2222-2222-222222222222"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}
CLAIMS_B = {"sub": USER_B, "username": "b", "can_client": True}

TODAY = date.today()
LONG_AGO = TODAY - timedelta(days=800)


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False}, poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed(db: Session) -> None:
    """预置池 4 条(对齐 Alembic 0005)+ 公共策略(对齐 0012)+ 股票与成分。"""
    db.add_all([
        Strategy(id=MA_CROSS_ID, owner_id=SYSTEM_OWNER_ID, is_system=True,
                 name="双均线趋势策略", template="ma_cross", kind="single",
                 params={}, enabled=True),
        Strategy(id=ROTATION_ID, owner_id=SYSTEM_OWNER_ID, is_system=True,
                 name="强势股票轮动策略", template="momentum_rotation",
                 kind="portfolio", params={}, enabled=True),
    ])
    db.add_all([
        Pool(id=1, kind="index", ref="hs300_zz500", owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="沪深300+中证500", min_list_days=0),
        Pool(id=2, kind="all", ref=None, owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="全部A股", min_list_days=60),
        Pool(id=3, kind="index", ref="hs300", owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="沪深300", min_list_days=0),
        Pool(id=4, kind="index", ref="zz500", owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="中证500", min_list_days=0),
    ])
    db.add_all([
        Stock(code="sh.600519", name="贵州茅台", industry="白酒",
              list_date=LONG_AGO, is_st=False),
        Stock(code="sz.000001", name="平安银行", industry="银行",
              list_date=LONG_AGO, is_st=False),
        Stock(code="sz.300750", name="宁德时代", industry="电池",
              list_date=LONG_AGO, is_st=False),
    ])
    db.add_all([
        IndexMember(index_name="hs300", code="sh.600519", in_date=LONG_AGO),
        IndexMember(index_name="hs300", code="sz.000001", in_date=LONG_AGO),
        IndexMember(index_name="zz500", code="sz.300750", in_date=LONG_AGO),
    ])
    db.commit()


def _seed_bars(db: Session, codes: list[str], start: date, end: date) -> None:
    """合成日线:含 start 之前的预热段,价格温和上行,足够跑组合回测。"""
    from app.backtest.engine import PORTFOLIO_WARMUP_DAYS

    day = start - timedelta(days=PORTFOLIO_WARMUP_DAYS)
    rows = []
    for offset, code in enumerate(codes):
        price = 10.0 + offset
        d = day
        i = 0
        while d <= end:
            px = price * (1 + 0.002 * i + 0.01 * offset)
            rows.append({
                "code": code, "date": d, "open": px, "high": px * 1.01,
                "low": px * 0.99, "close": px, "raw_close": px,
                "volume": 1e6, "amount": 1e7, "is_st": False,
            })
            d += timedelta(days=1)
            i += 1
    db.execute(DailyBar.__table__.insert(), rows)
    db.commit()


# ---- 契约点 2:预置池的 members 返回解析成分,不是空列表 ----

def test_preset_index_pool_members_resolve_current_constituents():
    with _session() as db:
        _seed(db)
        result = list_pool_members(pool_id=3, db=db, claims=CLAIMS_A)

    assert [item["code"] for item in result["items"]] == [
        "sh.600519", "sz.000001",
    ]
    assert result["count"] == 2
    assert result["resolved"] is True
    # 名称随成员一并返回,供前端直接展示
    assert result["items"][0]["name"] == "贵州茅台"


def test_pool_members_include_latest_price():
    """成员项附最新参考价:有日线给收盘价,无行情给空字段。"""
    with _session() as db:
        _seed(db)
        db.add(DailyBar(
            code="sh.600519", date=TODAY,
            open=1700, high=1710, low=1690, close=1705.5, raw_close=1705.5,
            volume=1, amount=1, is_st=False,
        ))
        db.commit()
        result = list_pool_members(pool_id=3, db=db, claims=CLAIMS_A)

    by_code = {item["code"]: item for item in result["items"]}
    assert by_code["sh.600519"]["price"] == 1705.5
    assert by_code["sh.600519"]["price_source"] == "close"
    assert by_code["sh.600519"]["pct_chg"] is None
    assert by_code["sh.600519"]["price_ts"] == str(TODAY)
    assert by_code["sz.000001"]["price"] is None
    assert by_code["sz.000001"]["price_source"] is None


def test_preset_all_market_pool_members_resolve_from_stock_table():
    from app.data.clock import today_cst

    with _session() as db:
        _seed(db)
        day = today_cst()
        for code in ("sh.600519", "sz.000001", "sz.300750"):
            db.add(DailyBar(
                code=code, date=day,
                open=10, high=10, low=10, close=10, raw_close=10,
                volume=1, amount=1, is_st=False,
            ))
        db.commit()
        result = list_pool_members(pool_id=2, db=db, claims=CLAIMS_A)

    assert [item["code"] for item in result["items"]] == [
        "sh.600519", "sz.000001", "sz.300750",
    ]


def test_combined_index_pool_takes_union_of_all_indices():
    with _session() as db:
        _seed(db)
        result = list_pool_members(pool_id=1, db=db, claims=CLAIMS_A)

    assert [item["code"] for item in result["items"]] == [
        "sh.600519", "sz.000001", "sz.300750",
    ]


def test_save_as_custom_pool_from_preset_snapshot_is_not_empty():
    """「另存为」全链路:取预置成分 -> 带 codes 建池 -> 新池成员非空。"""
    with _session() as db:
        _seed(db)
        snapshot = list_pool_members(pool_id=3, db=db, claims=CLAIMS_A)
        created = create_pool(
            PoolCreateIn(name="沪深300 副本", min_list_days=0,
                         codes=[item["code"] for item in snapshot["items"]]),
            db=db, claims=CLAIMS_A,
        )
        members = list_pool_members(pool_id=created["id"], db=db, claims=CLAIMS_A)

    assert created["kind"] == "static"
    assert created["owner_id"] == USER_A
    assert created["is_system"] is False
    assert created["member_count"] == 2
    assert [item["code"] for item in members["items"]] == [
        "sh.600519", "sz.000001",
    ]
    assert members["resolved"] is False


# ---- 契约点:预置池只读 ----

@pytest.mark.parametrize("preset_id", [1, 2, 3, 4])
def test_preset_pool_cannot_be_renamed(preset_id):
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            update_pool(pool_id=preset_id, body=PoolPatchIn(name="改名"),
                        db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 403
        assert "预置池" in exc.value.detail
        # 名称未被改动
        assert db.get(Pool, preset_id).name != "改名"


def test_preset_pool_cannot_be_deleted():
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            delete_pool(pool_id=2, db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 403
        assert db.get(Pool, 2) is not None


def test_preset_pool_members_cannot_be_added_or_removed():
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            add_pool_members(pool_id=3, body=PoolMembersIn(codes=["sh.600519"]),
                             db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 403
        with pytest.raises(HTTPException) as exc:
            remove_pool_member(pool_id=3, code="sh.600519", db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 403
        # 预置池不应因此产生任何成员行
        assert db.execute(select(PoolMember)).all() == []


# ---- 用户隔离(IDOR) ----

def test_custom_pool_is_invisible_to_other_users():
    with _session() as db:
        _seed(db)
        created = create_pool(PoolCreateIn(name="我的池"), db=db, claims=CLAIMS_A)
        pool_id = created["id"]

        listed_b = list_pools(db=db, claims=CLAIMS_B)
        assert pool_id not in [item["id"] for item in listed_b["items"]]
        # 但预置池对 B 仍然可见
        assert {1, 2, 3, 4} <= {item["id"] for item in listed_b["items"]}

        listed_a = list_pools(db=db, claims=CLAIMS_A)
        assert pool_id in [item["id"] for item in listed_a["items"]]

        for call in (
            lambda: get_pool(pool_id=pool_id, db=db, claims=CLAIMS_B),
            lambda: list_pool_members(pool_id=pool_id, db=db, claims=CLAIMS_B),
            lambda: update_pool(pool_id=pool_id, body=PoolPatchIn(name="劫持"),
                                db=db, claims=CLAIMS_B),
            lambda: delete_pool(pool_id=pool_id, db=db, claims=CLAIMS_B),
            lambda: add_pool_members(pool_id=pool_id,
                                     body=PoolMembersIn(codes=["sh.600519"]),
                                     db=db, claims=CLAIMS_B),
            lambda: remove_pool_member(pool_id=pool_id, code="sh.600519",
                                       db=db, claims=CLAIMS_B),
        ):
            with pytest.raises(HTTPException) as exc:
                call()
            # 404 而非 403:否则可靠状态码枚举他人的池
            assert exc.value.status_code == 404

        # 越权调用全部未落地
        assert db.get(Pool, pool_id).name == "我的池"
        assert db.get(Pool, pool_id).owner_id == USER_A


def test_same_pool_name_allowed_across_users_and_rejected_within_user():
    with _session() as db:
        _seed(db)
        create_pool(PoolCreateIn(name="核心池"), db=db, claims=CLAIMS_A)
        # 不同用户同名可以共存(唯一键是 (user_id, name))
        created_b = create_pool(PoolCreateIn(name="核心池"), db=db, claims=CLAIMS_B)
        assert created_b["owner_id"] == USER_B

        with pytest.raises(HTTPException) as exc:
            create_pool(PoolCreateIn(name="核心池"), db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409


# ---- 契约点 3:批量导入部分成功 ----

def test_bulk_import_skips_unknown_codes_instead_of_failing():
    with _session() as db:
        _seed(db)
        created = create_pool(PoolCreateIn(name="导入池"), db=db, claims=CLAIMS_A)
        result = add_pool_members(
            pool_id=created["id"],
            body=PoolMembersIn(codes=[
                "sh.600519",      # 已入库
                "SZ.000001",      # 大写,规范化后已入库
                "sh.999999",      # 格式合法但未入库
                "600519",         # 格式非法(缺市场前缀)
                "sh.600519",      # 重复
            ]),
            db=db, claims=CLAIMS_A,
        )

        assert result["added"] == 2
        assert result["skipped"] == ["600519", "sh.999999"]
        assert [item["code"] for item in result["items"]] == [
            "sh.600519", "sz.000001",
        ]
        members = list_pool_members(pool_id=created["id"], db=db, claims=CLAIMS_A)
        assert [item["code"] for item in members["items"]] == [
            "sh.600519", "sz.000001",
        ]


def test_bulk_import_is_idempotent_for_existing_members():
    with _session() as db:
        _seed(db)
        created = create_pool(
            PoolCreateIn(name="幂等池", codes=["sh.600519"]),
            db=db, claims=CLAIMS_A,
        )
        again = add_pool_members(
            pool_id=created["id"],
            body=PoolMembersIn(codes=["sh.600519", "sz.000001"]),
            db=db, claims=CLAIMS_A,
        )

        assert again["added"] == 1  # 已在池中的不重复计数
        members = list_pool_members(pool_id=created["id"], db=db, claims=CLAIMS_A)
        assert len(members["items"]) == 2


def test_create_pool_reports_skipped_codes():
    with _session() as db:
        _seed(db)
        created = create_pool(
            PoolCreateIn(name="含无效代码", codes=["sh.600519", "sh.999999"]),
            db=db, claims=CLAIMS_A,
        )

    assert created["member_count"] == 1
    assert created["skipped"] == ["sh.999999"]


# ---- 自定义池的增删改 ----

def test_update_and_delete_own_pool():
    with _session() as db:
        _seed(db)
        created = create_pool(
            PoolCreateIn(name="待改池", min_list_days=60, codes=["sh.600519"]),
            db=db, claims=CLAIMS_A,
        )
        pool_id = created["id"]

        patched = update_pool(pool_id=pool_id,
                              body=PoolPatchIn(name="已改名", min_list_days=5),
                              db=db, claims=CLAIMS_A)
        assert patched["name"] == "已改名"
        assert patched["min_list_days"] == 5
        assert patched["member_count"] == 1

        removed = remove_pool_member(pool_id=pool_id, code="SH.600519",
                                     db=db, claims=CLAIMS_A)
        assert removed["deleted"] == 1

        with pytest.raises(HTTPException) as exc:
            remove_pool_member(pool_id=pool_id, code="sh.600519",
                               db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 404

        assert delete_pool(pool_id=pool_id, db=db, claims=CLAIMS_A) == {
            "deleted": pool_id,
        }
        assert db.get(Pool, pool_id) is None
        # 成员行随池一并清理,不留孤儿
        assert db.execute(
            select(PoolMember).where(PoolMember.pool_id == pool_id)
        ).all() == []


def test_pool_list_puts_presets_first_and_counts_only_static():
    with _session() as db:
        _seed(db)
        create_pool(PoolCreateIn(name="自定义", codes=["sh.600519"]),
                    db=db, claims=CLAIMS_A)
        listed = list_pools(db=db, claims=CLAIMS_A)

    ids = [item["id"] for item in listed["items"]]
    assert ids[:4] == [1, 2, 3, 4]
    assert listed["count"] == 5
    presets = [item for item in listed["items"] if item["is_system"]]
    # 预置池的成员数按交易日动态解析,列表页不逐池解析(全A 要扫全表)
    assert all(item["member_count"] is None for item in presets)
    assert listed["items"][-1]["member_count"] == 1


def test_default_pool_prefers_all_market_preset():
    with _session() as db:
        _seed(db)
        assert default_pool(db).id == 2
        assert default_pool(db).kind == "all"


def test_min_list_days_bounds_are_enforced():
    for bad in (-1, 3651):
        with pytest.raises(ValueError):
            PoolCreateIn(name="越界", min_list_days=bad)
        with pytest.raises(ValueError):
            PoolPatchIn(min_list_days=bad)


def test_bulk_import_rejects_empty_and_oversized_code_lists():
    with pytest.raises(ValueError):
        PoolMembersIn(codes=[])
    with pytest.raises(ValueError):
        PoolMembersIn(codes=[f"sh.{i:06d}" for i in range(10001)])


def test_large_pool_import_exceeds_single_in_clause_limit():
    """「全A 另存为」会一次带数千代码:上限要容得下,且分批查不能报错。"""
    codes = [f"sh.{600000 + i:06d}" for i in range(1200)]
    with _session() as db:
        _seed(db)
        db.execute(Stock.__table__.insert(), [
            {"code": code, "name": f"股票{code}", "industry": "测试",
             "list_date": LONG_AGO, "is_st": False}
            for code in codes if code != "sh.600519"
        ])
        db.commit()
        created = create_pool(PoolCreateIn(name="大池", codes=codes),
                             db=db, claims=CLAIMS_A)
        members = list_pool_members(pool_id=created["id"], db=db, claims=CLAIMS_A)

    assert created["member_count"] == len(codes)
    assert created.get("skipped") is None
    assert len(members["items"]) == len(codes)
    assert all(item["name"] for item in members["items"])


def test_blank_pool_name_is_rejected():
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            create_pool(PoolCreateIn(name="   "), db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 400


# ---- 契约点 4:GET /api/backtest/{run_id} 回显 pool ----

# ---- 契约点:被历史记录引用的池不可删(与策略删除的 409 保护同款) ----

def test_delete_pool_referenced_by_backtest_returns_409():
    with _session() as db:
        _seed(db)
        created = create_pool(PoolCreateIn(name="被引用池"), db=db, claims=CLAIMS_A)
        pool_id = created["id"]
        db.add(BacktestRun(user_id=USER_A, strategy_id=ROTATION_ID,
                           params={}, pool_id=pool_id,
                           start=date(2024, 1, 1), end=date(2024, 6, 30),
                           metrics={"total_return": 0.1}))
        db.commit()

        with pytest.raises(HTTPException) as exc:
            delete_pool(pool_id=pool_id, db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409
        assert "回测" in exc.value.detail
        # 池与审计引用都保持原样
        assert db.get(Pool, pool_id) is not None
        # 引用清掉后即可正常删除
        db.execute(delete(BacktestRun))
        db.commit()
        assert delete_pool(pool_id=pool_id, db=db, claims=CLAIMS_A) == {
            "deleted": pool_id,
        }
        assert db.get(Pool, pool_id) is None


def test_delete_pool_referenced_by_eval_or_plan_returns_409():
    with _session() as db:
        _seed(db)
        eval_pool = create_pool(PoolCreateIn(name="评估引用池"), db=db,
                                claims=CLAIMS_A)
        db.add(StrategyEval(strategy_id=ROTATION_ID, params={}, scope="daily",
                            batch_id="batch-1", pool_id=eval_pool["id"],
                            start=date(2024, 1, 1), end=date(2024, 6, 30),
                            metrics={}))
        plan_pool = create_pool(PoolCreateIn(name="计划引用池"), db=db,
                                claims=CLAIMS_A)
        db.add(ResearchPlan(
            owner_id=USER_A, strategy_is_system=True, strategy_id=ROTATION_ID,
            strategy_name="强势股票轮动策略", template="momentum_rotation",
            strategy_kind="portfolio", strategy_version="v1",
            params_snapshot={}, plan_type="pool_scan",
            pool_id=plan_pool["id"], data_date=date(2024, 6, 28),
            generated_at=datetime(2024, 6, 28, 15, 30), signal_type="entry",
            status="pending", status_reason={}, entry_observation={},
            risk_rules=[], take_profit={}, native_exit=[], exit_hits=[],
            backtest_evidence={}, product_boundary="信息研究,不自动交易",
        ))
        db.commit()

        with pytest.raises(HTTPException) as exc:
            delete_pool(pool_id=eval_pool["id"], db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409
        assert "评估" in exc.value.detail
        with pytest.raises(HTTPException) as exc:
            delete_pool(pool_id=plan_pool["id"], db=db, claims=CLAIMS_A)
        assert exc.value.status_code == 409
        assert "研究计划" in exc.value.detail
        assert db.get(Pool, eval_pool["id"]) is not None
        assert db.get(Pool, plan_pool["id"]) is not None


def test_delete_unreferenced_pool_still_works():
    with _session() as db:
        _seed(db)
        created = create_pool(
            PoolCreateIn(name="无引用池", codes=["sh.600519"]),
            db=db, claims=CLAIMS_A,
        )
        pool_id = created["id"]

        assert delete_pool(pool_id=pool_id, db=db, claims=CLAIMS_A) == {
            "deleted": pool_id,
        }
        assert db.get(Pool, pool_id) is None
        assert db.execute(
            select(PoolMember).where(PoolMember.pool_id == pool_id)
        ).all() == []


def test_historical_backtest_echoes_pool_for_survivorship_annotation():
    """按编号查历史回测时前端没有本地选择状态,偏差标注只能靠回显的 kind。"""
    with _session() as db:
        _seed(db)
        static = create_pool(
            PoolCreateIn(name="静态池", codes=["sh.600519"]),
            db=db, claims=CLAIMS_A,
        )
        db.add_all([
            BacktestRun(id=1, user_id=USER_A, strategy_id=ROTATION_ID,
                        params={}, codes=["sh.600519"], pool_id=static["id"],
                        start=date(2024, 1, 1), end=date(2024, 6, 30),
                        metrics={"total_return": 0.1}),
            BacktestRun(id=2, user_id=USER_A, strategy_id=ROTATION_ID,
                        params={}, codes=["sh.600519"], pool_id=3,
                        start=date(2024, 1, 1), end=date(2024, 6, 30),
                        metrics={"total_return": 0.2}),
            # 池已被删除的历史回测
            BacktestRun(id=3, user_id=USER_A, strategy_id=ROTATION_ID,
                        params={}, codes=["sh.600519"], pool_id=999,
                        start=date(2024, 1, 1), end=date(2024, 6, 30),
                        metrics={"total_return": 0.3}),
        ])
        db.commit()

        on_static = get_backtest(run_id=1, db=db, claims=CLAIMS_A)
        on_index = get_backtest(run_id=2, db=db, claims=CLAIMS_A)
        on_missing = get_backtest(run_id=3, db=db, claims=CLAIMS_A)

    assert on_static["pool"] == {
        "id": static["id"], "name": "静态池", "kind": "static",
        "has_survivorship_bias": True,
    }
    assert on_index["pool"] == {
        "id": 3, "name": "沪深300", "kind": "index",
        "has_survivorship_bias": False,
    }
    assert on_missing["pool"] is None


def test_backtest_rejects_pool_owned_by_another_user():
    with _session() as db:
        _seed(db)
        created = create_pool(PoolCreateIn(name="B 的池", codes=["sh.600519"]),
                              db=db, claims=CLAIMS_B)
        with pytest.raises(HTTPException) as exc:
            create_backtest(
                BacktestIn(strategy_id=ROTATION_ID, codes=[],
                           start=date(2024, 1, 1), end=date(2024, 6, 30),
                           pool_id=created["id"]),
                background_tasks=BackgroundTasks(), db=db, claims=CLAIMS_A,
            )
        assert exc.value.status_code == 404


def test_portfolio_backtest_resolves_static_pool_and_persists_pool_id():
    """组合策略留空 codes 时按 pool_id 解析成分,并把 pool_id 落库供回查。"""
    start, end = date(2024, 1, 1), date(2024, 6, 30)
    with _session() as db:
        _seed(db)
        _seed_bars(db, ["sh.600519", "sz.000001"], start, end)
        pool = create_pool(
            PoolCreateIn(name="回测静态池",
                         codes=["sh.600519", "sz.000001", "sh.999999"]),
            db=db, claims=CLAIMS_A,
        )
        result = create_backtest(
            BacktestIn(strategy_id=ROTATION_ID, codes=[],
                       start=start, end=end, pool_id=pool["id"]),
            background_tasks=BackgroundTasks(),
            db=db, claims=CLAIMS_A,
        )
        run = db.get(BacktestRun, result["run_id"])
        # 落库后按编号回查,pool 依然带得回来
        reloaded = get_backtest(run_id=result["run_id"], db=db, claims=CLAIMS_A)

    # 未入库的 sh.999999 在建池时已被过滤,不会进回测样本
    assert sorted(result["codes"]) == ["sh.600519", "sz.000001"]
    assert result["pool"] == {
        "id": pool["id"], "name": "回测静态池", "kind": "static",
        "has_survivorship_bias": True,
    }
    assert run.pool_id == pool["id"]
    assert reloaded["pool"] == result["pool"]
    assert reloaded["params"] == result["parameter_snapshot"]
    assert reloaded["costs"] == result["costs"]
    assert reloaded["trade_details"] == result["trade_details"]
    assert reloaded["exit_reason_distribution"] == result["exit_reason_distribution"]


def test_portfolio_backtest_defaults_to_frozen_spec_pool():
    start, end = date(2024, 1, 1), date(2024, 6, 30)
    with _session() as db:
        _seed(db)
        _seed_bars(db, ["sh.600519", "sz.000001"], start, end)
        pool = create_pool(
            PoolCreateIn(name="规格指定池", codes=["sh.600519"]),
            db=db,
            claims=CLAIMS_A,
        )
        strategy = db.get(Strategy, ROTATION_ID)
        spec = get_preset_spec("momentum_rotation").model_dump(mode="json")
        spec["universe"]["pool_id"] = pool["id"]
        strategy.spec = spec
        strategy.spec_hash = strategy_spec_hash(spec)
        db.commit()

        result = create_backtest(
            BacktestIn(
                strategy_id=ROTATION_ID,
                codes=[],
                start=start,
                end=end,
            ),
            background_tasks=BackgroundTasks(),
            db=db,
            claims=CLAIMS_A,
        )

    assert result["pool"]["id"] == pool["id"]
    assert result["codes"] == ["sh.600519"]


def test_explicit_codes_backtest_does_not_echo_pool():
    """显式给 codes 的回测与池无关,不应回显 pool(否则前端误标偏差)。"""
    start, end = date(2024, 1, 1), date(2024, 6, 30)
    with _session() as db:
        _seed(db)
        _seed_bars(db, ["sh.600519"], start, end)
        result = create_backtest(
            BacktestIn(strategy_id=MA_CROSS_ID, codes=["sh.600519"],
                       start=start, end=end),
            background_tasks=BackgroundTasks(),
            db=db, claims=CLAIMS_A,
        )
        run = db.get(BacktestRun, result["run_id"])

    assert "pool" not in result
    assert run.pool_id is None


def test_single_backtest_resolves_pool_and_persists_pool_id():
    """单标的策略也可用股票池定义研究范围:codes 留空 + pool_id 解析成分。"""
    # 起点须在名录最早生效日(LONG_AGO ≈ 2024-05)之后
    start, end = date(2024, 6, 3), date(2024, 12, 31)
    with _session() as db:
        _seed(db)
        _seed_bars(db, ["sh.600519", "sz.000001"], start, end)
        result = create_backtest(
            BacktestIn(strategy_id=MA_CROSS_ID, codes=[],
                       start=start, end=end, pool_id=3),
            background_tasks=BackgroundTasks(), db=db, claims=CLAIMS_A,
        )
        run = db.get(BacktestRun, result["run_id"])
        reloaded = get_backtest(run_id=result["run_id"], db=db, claims=CLAIMS_A)

    # 沪深300 预置池(id=3)成分为 sh.600519 / sz.000001
    assert sorted(result["codes"]) == ["sh.600519", "sz.000001"]
    assert result["pool"] == {
        "id": 3, "name": "沪深300", "kind": "index",
        "has_survivorship_bias": False,
    }
    assert run.pool_id == 3
    assert reloaded["pool"] == result["pool"]


def test_single_backtest_rejects_codes_with_pool():
    """codes 与 pool_id 互斥:同时给出按 400 拒绝,而不是静默忽略一个。"""
    start, end = date(2024, 1, 1), date(2024, 6, 30)
    with _session() as db:
        _seed(db)
        with pytest.raises(HTTPException) as exc:
            create_backtest(
                BacktestIn(strategy_id=MA_CROSS_ID, codes=["sh.600519"],
                           start=start, end=end, pool_id=3),
                background_tasks=BackgroundTasks(), db=db, claims=CLAIMS_A,
            )
    assert exc.value.status_code == 400
    assert "互斥" in str(exc.value.detail) or "只能选其一" in str(exc.value.detail)


def test_single_backtest_gates_entries_by_membership():
    """指数池成员资格逐日生效:入池日之前不允许建仓(point-in-time)。"""
    start, end = date(2024, 6, 3), date(2024, 12, 31)
    mid = date(2024, 9, 2)
    with _session() as db:
        _seed(db)
        # sz.300750 在区间中段才纳入沪深300;合成行情全程温和上行,
        # 若不按成员资格掩码,双均线策略会在 1 月就对它建仓
        db.add(IndexMember(index_name="hs300", code="sz.300750", in_date=mid))
        _seed_bars(db, ["sh.600519", "sz.300750"], start, end)
        result = create_backtest(
            BacktestIn(strategy_id=MA_CROSS_ID, codes=[],
                       start=start, end=end, pool_id=3),
            background_tasks=BackgroundTasks(), db=db, claims=CLAIMS_A,
        )

    assert "sz.300750" in result["codes"]
    late_trades = [
        t for t in result["trade_details"] if t["code"] == "sz.300750"
    ]
    assert late_trades, "入池后应当产生交易"
    assert min(t["execution_date"] for t in late_trades) >= str(mid)
