"""信号引擎按策略行运行的行为断言。

覆盖本次改造的三个契约点:
1. 跑**所有启用的**单标的策略,每行按自己的参数出信号(停用的跳过);
2. 每只股票的日线只加载一次(循环顺序决定夜间流水线是否会被策略数拖爆);
3. 信号列表按策略可见性过滤 —— 别人的策略名不能出现在我的页面上。
"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from sqlalchemy import create_engine, select
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import app.strategy.engine as signal_engine
from app.api.signals import list_signals
from app.db import Base
from app.models import (SYSTEM_OWNER_ID, DailyBar, ResearchPlan, Signal, Stock,
                        Strategy, TradeCalendar)

USER_A = "11111111-1111-1111-1111-111111111111"
USER_B = "22222222-2222-2222-2222-222222222222"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}
CLAIMS_B = {"sub": USER_B, "username": "b", "can_client": True}

CODE = "sh.600000"


@pytest.fixture(autouse=True)
def sqlite_upsert(monkeypatch):
    """_save_signal 用 MySQL 的 ON DUPLICATE KEY UPDATE,sqlite 无法编译。

    生产是 MySQL,这里只把落库语句换成等价的 sqlite upsert,被测的循环与
    信号判定逻辑保持原样。
    """
    def _save(db, code, day, strategy_id, side, price, reason, spec_hash):
        stmt = sqlite_insert(Signal).values(
            code=code, date=day, strategy_id=strategy_id, side=side,
            price=price, reason=reason, spec_hash=spec_hash)
        db.execute(stmt.on_conflict_do_update(
            index_elements=["code", "date", "strategy_id", "side"],
            set_={"price": price, "reason": reason, "spec_hash": spec_hash}))

    monkeypatch.setattr(signal_engine, "_save_signal", _save)


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_bars(db: Session) -> date:
    """119 天平盘 + 最后一天大涨:快线在最后一根 bar 上穿慢线(产生 buy)。"""
    db.add(Stock(code=CODE, name="浦发银行", industry="银行"))
    prices = [10.0] * 119 + [15.0]
    day = date(2026, 7, 24) - timedelta(days=200)
    last = day
    for price in prices:
        while day.weekday() >= 5:  # 跳过周末,贴近真实交易日序列
            day += timedelta(days=1)
        db.add(DailyBar(code=CODE, date=day, open=price, high=price,
                        low=price, close=price, volume=1e6, amount=1e7))
        last = day
        day += timedelta(days=1)
    db.commit()
    return last


def test_runs_every_enabled_strategy_with_its_own_params():
    with _session() as db:
        last = _seed_bars(db)
        db.add_all([
            Strategy(id=1, owner_id=SYSTEM_OWNER_ID, is_system=True,
                     name="双均线趋势策略", template="ma_cross", kind="single",
                     params={}, enabled=True),
            Strategy(id=2, owner_id=USER_A, is_system=False, name="我的快线",
                     template="ma_cross", kind="single",
                     params={"fast": 3, "slow": 8}, enabled=True),
            # 停用的不该出信号
            Strategy(id=3, owner_id=USER_A, is_system=False, name="停用的",
                     template="breakout", kind="single", params={},
                     enabled=False),
            # 组合策略不按个股出信号
            Strategy(id=4, owner_id=SYSTEM_OWNER_ID, is_system=True,
                     name="强势股票轮动策略", template="momentum_rotation",
                     kind="portfolio", params={}, enabled=True),
        ])
        db.commit()

        result = signal_engine.run_signals(db, day=last, codes=[CODE])
        rows = db.execute(select(Signal)).scalars().all()

    assert result["total"] == 2
    assert {row.strategy_id for row in rows} == {1, 2}
    assert {row.side for row in rows} == {"buy"}
    # 各自按自己的参数记录 reason,便于事后解释
    by_id = {row.strategy_id: row.reason["params"] for row in rows}
    assert by_id[1] == {}
    assert by_id[2] == {"fast": 3, "slow": 8}


def test_loads_each_stock_bars_only_once():
    """循环顺序:每只股票加载一次后跑全部策略,而不是每个策略各加载一遍。

    原实现是 `for 策略: for 股票: load_bars_df(...)`,策略数一涨就把夜间
    流水线的查库次数乘上去。这里用调用计数把顺序钉住。
    """
    calls: list[str] = []
    with _session() as db:
        last = _seed_bars(db)
        db.add_all([
            Strategy(id=i, owner_id=SYSTEM_OWNER_ID, is_system=True,
                     name=f"策略{i}", template="ma_cross", kind="single",
                     params={}, enabled=True)
            for i in (1, 2, 3)
        ])
        db.commit()

        original = signal_engine.load_bars_df

        def counting(db_, code, start=None, end=None):
            calls.append(code)
            return original(db_, code, start=start, end=end)

        signal_engine.load_bars_df = counting
        try:
            signal_engine.run_signals(db, day=last, codes=[CODE])
        finally:
            signal_engine.load_bars_df = original

    assert calls == [CODE], f"3 个策略只应加载 1 次日线,实际 {len(calls)} 次"


def test_signals_are_filtered_by_strategy_visibility():
    """信号引擎跨用户跑,但列表只出公共策略和我自己的。"""
    with _session() as db:
        last = _seed_bars(db)
        db.add_all([
            Strategy(id=1, owner_id=SYSTEM_OWNER_ID, is_system=True,
                     name="双均线趋势策略", template="ma_cross", kind="single",
                     params={}, enabled=True),
            Strategy(id=2, owner_id=USER_A, is_system=False, name="A 的策略",
                     template="ma_cross", kind="single",
                     params={"fast": 3, "slow": 8}, enabled=True),
            Strategy(id=3, owner_id=USER_B, is_system=False, name="B 的策略",
                     template="ma_cross", kind="single",
                     params={"fast": 4, "slow": 9}, enabled=True),
        ])
        db.commit()

        signal_engine.run_signals(db, day=last, codes=[CODE])

        seen_a = list_signals(date_=None, code=None, strategy_id=None,
                              side=None, limit=200, db=db, claims=CLAIMS_A)
        seen_b = list_signals(date_=None, code=None, strategy_id=None,
                              side=None, limit=200, db=db, claims=CLAIMS_B)

    assert {item["strategy_name"] for item in seen_a["items"]} == {
        "双均线趋势策略", "A 的策略"}
    assert {item["strategy_name"] for item in seen_b["items"]} == {
        "双均线趋势策略", "B 的策略"}


def test_reason_text_uses_each_strategy_own_params():
    """措辞按模板选,数字用该策略实际参数(不是模板默认值)。"""
    with _session() as db:
        last = _seed_bars(db)
        db.add(Strategy(id=2, owner_id=USER_A, is_system=False,
                        name="我的快线", template="ma_cross", kind="single",
                        params={"fast": 3, "slow": 8}, enabled=True))
        db.commit()

        signal_engine.run_signals(db, day=last, codes=[CODE])
        listed = list_signals(date_=None, code=None, strategy_id=None,
                              side=None, limit=200, db=db, claims=CLAIMS_A)

    item = listed["items"][0]
    assert item["strategy_name"] == "我的快线"
    assert item["reason_text"] == "3日均线上穿8日均线，策略模拟状态变为持有。"


def test_execution_uses_spec_even_when_legacy_template_name_is_unknown():
    """迁移占位 template 不得参与运行分支；完整 spec 才是唯一执行定义。"""
    from app.strategy.presets import get_preset_spec

    with _session() as db:
        last = _seed_bars(db)
        spec = get_preset_spec("ma_cross").model_dump(mode="json")
        db.add(Strategy(id=1, owner_id=USER_A, is_system=False, name="幽灵策略",
                        template="removed_template", kind="single", params={},
                        spec=spec, enabled=True))
        db.commit()

        result = signal_engine.run_signals(db, day=last, codes=[CODE])

    assert result["total"] == 1
    assert result["signals"]["幽灵策略#1"] == {CODE: "buy"}


def test_ongoing_holding_generates_plan_with_entry_based_overlay_lines():
    with _session() as db:
        last = _seed_bars(db)
        next_day = last + timedelta(days=1)
        while next_day.weekday() >= 5:
            next_day += timedelta(days=1)
        following = next_day + timedelta(days=1)
        while following.weekday() >= 5:
            following += timedelta(days=1)
        db.add_all([
            Strategy(
                id=1, owner_id=USER_A, is_system=False,
                name="带风险线的趋势", template="ma_cross", kind="single",
                params={
                    "risk_overlay": {
                        "enabled": True, "type": "fixed_pct", "value": 0.1,
                    },
                    "take_profit": {
                        "enabled": True, "type": "fixed_pct", "value": 0.2,
                    },
                }, enabled=True,
            ),
            TradeCalendar(date=last, is_open=True),
            TradeCalendar(date=next_day, is_open=True),
            TradeCalendar(date=following, is_open=True),
        ])
        db.commit()
        signal_engine.run_signals(db, day=last, codes=[CODE])
        db.add(DailyBar(
            code=CODE, date=next_day, open=15, high=15, low=15,
            close=15, raw_close=15, volume=1e6, amount=15e6,
        ))
        db.commit()

        result = signal_engine.run_signals(db, day=next_day, codes=[CODE])
        plans = db.execute(select(ResearchPlan).where(
            ResearchPlan.strategy_id == 1,
            ResearchPlan.code == CODE,
        ).order_by(ResearchPlan.id)).scalars().all()

        assert result["total"] == 0
        assert [plan.signal_type for plan in plans] == ["buy", "hold"]
        assert plans[-1].signal_price is None
        overlay = next(
            rule for rule in plans[-1].risk_rules if rule["source"] == "overlay"
        )
        assert overlay["reference_line"] == pytest.approx(13.50135)
        assert plans[-1].take_profit["reference_line"] == pytest.approx(18.0018)


def test_blocked_t_plus_one_entry_does_not_create_fake_sell_signal():
    with _session() as db:
        last = _seed_bars(db)
        next_day = last + timedelta(days=1)
        while next_day.weekday() >= 5:
            next_day += timedelta(days=1)
        following = next_day + timedelta(days=1)
        while following.weekday() >= 5:
            following += timedelta(days=1)
        db.add_all([
            Strategy(
                id=1, owner_id=USER_A, is_system=False,
                name="默认趋势", template="ma_cross", kind="single",
                params={}, enabled=True,
            ),
            TradeCalendar(date=last, is_open=True),
            TradeCalendar(date=next_day, is_open=True),
            TradeCalendar(date=following, is_open=True),
        ])
        db.commit()
        first = signal_engine.run_signals(db, day=last, codes=[CODE])
        db.add(DailyBar(
            code=CODE, date=next_day, open=16.5, high=16.5, low=16.5,
            close=16.5, raw_close=16.5, volume=1e6, amount=16.5e6,
        ))
        db.commit()

        second = signal_engine.run_signals(db, day=next_day, codes=[CODE])
        signals = db.execute(select(Signal).where(
            Signal.strategy_id == 1, Signal.code == CODE,
        ).order_by(Signal.id)).scalars().all()
        plans = db.execute(select(ResearchPlan).where(
            ResearchPlan.strategy_id == 1, ResearchPlan.code == CODE,
        ).order_by(ResearchPlan.id)).scalars().all()

        assert first["total"] == 1
        assert second["total"] == 0
        assert [signal.side for signal in signals] == ["buy"]
        assert [plan.signal_type for plan in plans] == ["buy"]
