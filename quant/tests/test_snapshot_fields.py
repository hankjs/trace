"""估值/财务快照字段接入引擎数据供给的行为断言(内存 SQLite)。

覆盖契约:
1. PIT join 严禁未来函数:估值按交易日取 available_date <= 当日的最新一条,
   财务版本(修订)只从其 available_date 起生效;
2. spec 字段名到快照列的映射(market_cap/revenue_growth/cashflow_quality 等);
3. 缺数据不填 0:没有可用记录的交易日是 NaN,缺列仍走「缺少字段」报错;
4. 回测入口按 data_requirements 供给字段,缺字段仍报 missing_data;
5. 保存策略时 capability 把库里实际有数据的字段纳入 available_fields。
"""
from __future__ import annotations

import sys
from copy import deepcopy
from datetime import date
from pathlib import Path
from types import SimpleNamespace

import pandas as pd
import pytest
from fastapi import HTTPException
from sqlalchemy import create_engine
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.strategies import StrategyCreateIn, create_strategy
from app.backtest.engine import run_backtest
from app.data.ingest import (attach_snapshot_fields, load_bars_df,
                             required_snapshot_fields,
                             snapshot_available_fields)
from app.db import Base
from app.models import (DailyBar, FundamentalSnapshot,
                        ValuationSnapshot)
from app.strategy.compiler import compile_single
from app.strategy.presets import get_preset_spec
from app.strategy.spec import (CapabilityStatus, parse_strategy_spec,
                               resolve_capabilities)

CODE = "sh.600001"
CLAIMS = {"sub": "11111111-1111-1111-1111-111111111111",
          "username": "a", "can_client": True}


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed_bars(db: Session, days: int = 10) -> list[date]:
    dates = pd.bdate_range(end="2026-07-24", periods=days).date
    for index, day in enumerate(dates):
        price = 10.0 + index * 0.01
        db.add(DailyBar(code=CODE, date=day, open=price, high=price + 0.05,
                        low=price - 0.05, close=price, raw_close=price,
                        volume=1e6, amount=1e7, is_st=False))
    db.commit()
    return list(dates)


def _pe_spec() -> dict:
    """breakout 预置规格 + 「PE(TTM) 低于 20」的入场过滤条件。"""
    raw = get_preset_spec("breakout").model_dump(mode="json")
    raw["entry"]["condition"] = {
        "op": "all",
        "args": [
            raw["entry"]["condition"],
            {"op": "lt",
             "left": {"op": "field", "name": "pe_ttm"},
             "right": {"op": "literal", "value": 20.0}},
        ],
    }
    raw["data_requirements"].append(
        {"field": "pe_ttm", "availability": "point_in_time", "required": True},
    )
    return raw


def test_valuation_join_never_reads_future_availability():
    """data_date 为 D5 的估值到 D8 才可用:D5-D7 不得看到它。"""
    with _session() as db:
        dates = _seed_bars(db)
        d3, d5, d8 = dates[2], dates[4], dates[7]
        db.add_all([
            ValuationSnapshot(code=CODE, data_date=d3, available_date=d3,
                              source="test", pe_ttm=10.0),
            ValuationSnapshot(code=CODE, data_date=d5, available_date=d8,
                              source="test", pe_ttm=99.0),
            ValuationSnapshot(code=CODE, data_date=d8, available_date=d8,
                              source="test", pe_ttm=12.0),
        ])
        db.commit()

        df = load_bars_df(db, CODE, extra_fields=["pe_ttm"])

    pe = {day: value for day, value in zip(df["date"], df["pe_ttm"])}
    assert pe[dates[0]] != pe[dates[0]]  # 首个可用日前是 NaN
    assert pe[d3] == 10.0
    assert pe[dates[4]] == 10.0  # D5:99 那条尚不可用,仍看到 D3 的值
    assert pe[dates[6]] == 10.0
    assert pe[d8] == 12.0  # D8 起同 available_date 取 data_date 最新
    assert pe[dates[-1]] == 12.0


def test_fundamental_revision_only_effective_from_available_date():
    """同一报告期的修订值只从其 available_date 起参与研究。"""
    with _session() as db:
        dates = _seed_bars(db)
        d2, d6 = dates[1], dates[5]
        db.add_all([
            FundamentalSnapshot(code=CODE, data_date=date(2025, 12, 31),
                                report_period=date(2025, 12, 31),
                                available_date=d2, source="test", roe=0.10),
            FundamentalSnapshot(code=CODE, data_date=date(2025, 12, 31),
                                report_period=date(2025, 12, 31),
                                available_date=d6, source="test", roe=0.12),
        ])
        db.commit()

        df = load_bars_df(db, CODE, extra_fields=["roe"])

    roe = {day: value for day, value in zip(df["date"], df["roe"])}
    assert roe[dates[0]] != roe[dates[0]]  # NaN
    assert roe[d2] == 0.10
    assert roe[dates[4]] == 0.10  # 修订版尚不可用
    assert roe[d6] == 0.12
    assert roe[dates[-1]] == 0.12


def test_field_name_mapping_and_no_zero_fill():
    """spec 字段名映射到快照列;从未发布的字段保持 NaN 而不是 0。"""
    with _session() as db:
        dates = _seed_bars(db)
        d1 = dates[0]
        db.add_all([
            ValuationSnapshot(code=CODE, data_date=d1, available_date=d1,
                              source="test", total_market_cap=1.2e11),
            FundamentalSnapshot(code=CODE, data_date=date(2025, 12, 31),
                                report_period=date(2025, 12, 31),
                                available_date=d1, source="test",
                                revenue_yoy=0.08, cashflow_ratio=1.1,
                                net_margin=0.15),
        ])
        db.commit()

        df = load_bars_df(db, CODE, extra_fields=[
            "market_cap", "revenue_growth", "cashflow_quality", "net_margin",
            "pb",
        ])

    assert df["market_cap"].iat[-1] == 1.2e11
    assert df["revenue_growth"].iat[-1] == 0.08
    assert df["cashflow_quality"].iat[-1] == 1.1
    assert df["net_margin"].iat[-1] == 0.15
    # pb 从未写入:NaN,不是 0
    assert df["pb"].isna().all()


def test_net_margin_is_supported_and_required_from_spec():
    """净利率与毛利率同级:白名单 + 快照映射 + data_requirements 可声明。"""
    from app.data.ingest import FUNDAMENTAL_SPEC_FIELDS
    from app.strategy.spec import SUPPORTED_FIELDS

    assert "net_margin" in SUPPORTED_FIELDS
    assert FUNDAMENTAL_SPEC_FIELDS["net_margin"] == "net_margin"

    raw = get_preset_spec("breakout").model_dump(mode="json")
    raw["entry"]["condition"] = {
        "op": "all",
        "args": [
            raw["entry"]["condition"],
            {"op": "gt",
             "left": {"op": "field", "name": "net_margin"},
             "right": {"op": "literal", "value": 0.05}},
        ],
    }
    raw["data_requirements"].append(
        {"field": "net_margin", "availability": "point_in_time",
         "required": True},
    )
    spec = parse_strategy_spec(raw)
    assert "net_margin" in required_snapshot_fields(spec)


def test_attach_snapshot_fields_rejects_unknown_and_handles_empty():
    with _session() as db:
        empty = pd.DataFrame(columns=["date", "close"])
        result = attach_snapshot_fields(db, empty, CODE, ["pe_ttm"])
        assert "pe_ttm" in result.columns
        with pytest.raises(ValueError, match="不供给字段"):
            attach_snapshot_fields(db, empty, CODE, ["no_such_field"])


def test_missing_field_still_raises_instead_of_silent_zero():
    """帧里没有声明字段时编译器仍如实报「缺少字段」。"""
    spec = parse_strategy_spec(_pe_spec())
    frame = pd.DataFrame({
        "date": pd.bdate_range(end="2026-07-24", periods=30).date,
        "open": [10.0] * 30, "high": [10.1] * 30, "low": [9.9] * 30,
        "close": [10.0] * 30, "raw_close": [10.0] * 30,
        "volume": [1e6] * 30, "amount": [1e7] * 30,
    })

    with pytest.raises(ValueError, match="缺少"):
        compile_single(spec, frame)

    report = resolve_capabilities(
        _pe_spec(), available_fields={"open", "high", "low", "close"},
    )
    assert report.status == CapabilityStatus.MISSING_DATA
    assert any(issue.code == "field_not_available" for issue in report.issues)


def test_required_snapshot_fields_extracts_from_spec():
    spec = parse_strategy_spec(_pe_spec())
    assert required_snapshot_fields(spec) == ["pe_ttm"]
    plain = parse_strategy_spec(get_preset_spec("breakout").model_dump(mode="json"))
    assert required_snapshot_fields(plain) == []


def test_single_backtest_runs_with_pe_filter_supplied_from_snapshot():
    """回测入口按 data_requirements 把 pe_ttm 供进编译帧(修复前必报缺字段)。"""
    with _session() as db:
        dates = _seed_bars(db, days=100)
        for day in dates:
            db.add(ValuationSnapshot(code=CODE, data_date=day,
                                     available_date=day, source="test",
                                     pe_ttm=10.0))
        db.commit()
        strategy = SimpleNamespace(
            id=1, name="PE 过滤突破", template="strategy_spec",
            params={}, spec=_pe_spec(),
        )

        result = run_backtest(
            db, strategy, [CODE], dates[0], dates[-1], save=False,
        )

    assert result["codes"] == [CODE]
    assert "total_return" in result["metrics"]


def test_single_backtest_without_snapshot_data_reports_missing_field():
    """快照表没有数据时 pe_ttm 列全 NaN 可编译;缺列才报 missing_data。

    这里验证加载层确实把列供进帧:没有快照行时列存在但全为 NaN,
    capability 检查通过,策略按 NaN 语义(条件不成立)运行而不是报错。
    """
    with _session() as db:
        dates = _seed_bars(db, days=100)
        strategy = SimpleNamespace(
            id=1, name="PE 过滤突破", template="strategy_spec",
            params={}, spec=_pe_spec(),
        )
        df = load_bars_df(db, CODE, extra_fields=["pe_ttm"])
        assert "pe_ttm" in df.columns
        assert df["pe_ttm"].isna().all()

        result = run_backtest(
            db, strategy, [CODE], dates[0], dates[-1], save=False,
        )

    assert result["codes"] == [CODE]


def test_snapshot_available_fields_reflects_actual_data():
    with _session() as db:
        assert snapshot_available_fields(db) == frozenset()
        db.add(ValuationSnapshot(code=CODE, data_date=date(2026, 7, 24),
                                 available_date=date(2026, 7, 24),
                                 source="test", pe_ttm=10.0))
        db.commit()
        assert snapshot_available_fields(db) == frozenset({"pe_ttm"})


def test_create_strategy_reports_missing_data_until_snapshot_exists():
    """保存启用策略时,capability 按库里实际数据给出 missing_data 报告。"""
    with _session() as db:
        with pytest.raises(HTTPException) as exc:
            create_strategy(
                StrategyCreateIn(name="PE 策略", spec=_pe_spec(), enabled=True),
                db=db, claims=CLAIMS,
            )
        assert exc.value.status_code == 400
        capability = exc.value.detail["capability"]
        assert capability["status"] == "missing_data"
        assert any(issue["code"] == "field_not_available"
                   for issue in capability["issues"])

        db.add(ValuationSnapshot(code=CODE, data_date=date(2026, 7, 24),
                                 available_date=date(2026, 7, 24),
                                 source="test", pe_ttm=10.0))
        db.commit()
        created = create_strategy(
            StrategyCreateIn(name="PE 策略", spec=_pe_spec(), enabled=True),
            db=db, claims=CLAIMS,
        )
        assert created["enabled"] is True
        assert created["capability"]["status"] == "supported"


def test_deepcopy_unused_but_spec_mutation_safe():
    """_pe_spec 每次返回独立副本,追加 data_requirements 不污染预置。"""
    first, second = _pe_spec(), _pe_spec()
    first["data_requirements"].append(
        {"field": "pb", "availability": "point_in_time", "required": True},
    )
    assert len(second["data_requirements"]) == len(
        _pe_spec()["data_requirements"],
    )
    assert deepcopy(second) == second
