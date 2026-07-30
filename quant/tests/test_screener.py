from datetime import date
import sys
from pathlib import Path

import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import app.selection.screener as screener
from app.db import Base
from app.models import (
    SYSTEM_OWNER_ID,
    Pool,
    DailyBar,
    FactorDaily,
    FundamentalSnapshot,
    IndexMember,
    Stock,
    ValuationSnapshot,
)
from app.selection.screener import (
    InvalidFilterError,
    evaluate_conditions,
    structured_screen,
)
from tests.factories import seed_factor_defs, seed_selection_config


def _condition(field: str, operator: str, value=None, value_to=None) -> dict:
    return {
        "id": f"{field}_{operator}",
        "field": field,
        "operator": operator,
        "value": value,
        "value_to": value_to,
        "enabled": True,
    }


def _seed_pools(db) -> None:
    """建预置池:pool_id=None 走 default_pool(kind='all'),pool_id=1 为指数池。

    id 固定:1=index(沪深300+中证500)、2=all(全A)。default_pool 优先取
    kind='all',故 pool_id 缺省时命中 id=2。min_list_days=0 让测试数据
    不被新股规则剔除。
    """
    db.add_all([
        Pool(id=1, kind="index", ref="hs300_zz500", owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="沪深300+中证500", min_list_days=0),
        Pool(id=2, kind="all", ref=None, owner_id=SYSTEM_OWNER_ID, is_system=True,
             name="全部A股", min_list_days=0),
    ])
    db.flush()


@pytest.mark.parametrize(
    ("condition", "expected"),
    [
        (_condition("pe_ttm", "eq", 12), 1),
        (_condition("pe_ttm", "ne", 12), 1),
        (_condition("pe_ttm", "gt", 12), 1),
        (_condition("pe_ttm", "gte", 12), 2),
        (_condition("pe_ttm", "lt", 20), 1),
        (_condition("pe_ttm", "lte", 12), 1),
        (_condition("pe_ttm", "between", 10, 20), 1),
        (_condition("industry", "in", ["白酒", "银行"]), 1),
        (_condition("industry", "not_in", ["白酒"]), 1),
        (_condition("roe", "is_null"), 1),
        (_condition("roe", "not_null"), 1),
    ],
)
def test_condition_operators(condition: dict, expected: int) -> None:
    rows = [
        {"code": "a", "values": {"pe_ttm": 12.0, "industry": "白酒", "roe": 0.2}},
        {"code": "b", "values": {"pe_ttm": 24.0, "industry": "煤炭", "roe": None}},
    ]
    result = evaluate_conditions(rows, {"logic": "and", "conditions": [condition]})
    assert result["independent_counts"][condition["id"]] == expected
    assert len(result["items"]) == expected


def test_group_logic_and_independent_counts() -> None:
    rows = [
        {"code": "a", "values": {"pe_ttm": 12.0, "roe": 0.2, "industry": "白酒"}},
        {"code": "b", "values": {"pe_ttm": 24.0, "roe": 0.1, "industry": "银行"}},
        {"code": "c", "values": {"pe_ttm": 30.0, "roe": None, "industry": "煤炭"}},
    ]
    payload = {
        "logic": "or",
        "groups": [
            {
                "id": "value",
                "logic": "and",
                "conditions": [
                    {**_condition("pe_ttm", "lte", 15), "id": "low_pe"},
                    {**_condition("roe", "gte", 0.15), "id": "high_roe"},
                ],
            },
            {
                "id": "industry",
                "logic": "and",
                "conditions": [
                    {**_condition("industry", "eq", "银行"), "id": "bank"},
                ],
            },
        ],
    }
    result = evaluate_conditions(rows, payload)
    assert [row["code"] for row in result["items"]] == ["a", "b"]
    assert result["independent_counts"] == {"low_pe": 1, "high_roe": 1, "bank": 1}
    assert result["field_coverage"] == {"pe_ttm": 3, "roe": 2, "industry": 3}
    assert result["items"][1]["matched_conditions"] == ["bank"]
    assert result["items"][1]["failed_conditions"] == ["low_pe", "high_roe"]


def test_structured_screen_excludes_not_yet_available_report() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(FactorDaily(
            id=1, code="sh.600001", date=day,
            values={
                "mom20": 0.05, "mom60": 0.08, "rsi14": 55,
                "atr_pct": 0.02, "vol_ratio5": 1.2,
                "ma20_slope": 0.01, "amount_avg20": 1e8,
            },
        ))
        db.add_all([
            DailyBar(
                code="sh.600001", date=date(2025, 1, 9),
                open=10, high=10.5, low=9.8, close=10, raw_close=10,
                volume=100, amount=1000, is_st=False,
            ),
            DailyBar(
                code="sh.600001", date=day,
                open=10, high=11, low=9.9, close=11, raw_close=11,
                volume=120, amount=1200, is_st=False,
            ),
        ])
        db.add(ValuationSnapshot(
            code="sh.600001", data_date=day, available_date=day,
            report_period=None, source="test", pe_ttm=15, pb=2,
            ps_ttm=3, dividend_yield=0.02, total_market_cap=1e10,
        ))
        db.add_all([
            FundamentalSnapshot(
                code="sh.600001", data_date=date(2024, 9, 30),
                report_period=date(2024, 9, 30), available_date=date(2024, 10, 30),
                source="test", roe=0.12,
            ),
            FundamentalSnapshot(
                code="sh.600001", data_date=date(2024, 12, 31),
                report_period=date(2024, 12, 31), available_date=date(2025, 3, 30),
                source="test", roe=0.30,
            ),
        ])
        db.commit()

        result = structured_screen(db, {
            "date": day,
            "logic": "and",
            "conditions": [
                {**_condition("roe", "between", 0.10, 0.15), "id": "roe_range"},
                {**_condition("pe_ttm", "lte", 20), "id": "pe_limit"},
            ],
            "groups": [],
            "pool_id": None,
            "limit": 20,
        })

    assert result["combined_count"] == 1
    assert result["independent_counts"] == {"roe_range": 1, "pe_limit": 1}
    assert result["field_coverage"] == {"roe": 1, "pe_ttm": 1}
    assert result["items"][0]["values"]["roe"] == pytest.approx(0.12)
    assert result["items"][0]["values"]["fundamental_available_date"] == "2024-10-30"


def test_structured_screen_uses_revision_only_after_available_date() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    before_revision = date(2025, 1, 10)
    after_revision = date(2025, 2, 10)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add_all([
            FactorDaily(id=1, code="sh.600001", date=before_revision, values={}),
            FactorDaily(id=2, code="sh.600001", date=after_revision, values={}),
        ])
        db.add_all([
            DailyBar(
                code="sh.600001", date=before_revision,
                open=10, high=11, low=9, close=10, raw_close=10,
                volume=100, amount=1000, is_st=False,
            ),
            DailyBar(
                code="sh.600001", date=after_revision,
                open=10, high=11, low=9, close=10, raw_close=10,
                volume=100, amount=1000, is_st=False,
            ),
        ])
        db.add_all([
            FundamentalSnapshot(
                code="sh.600001", data_date=date(2024, 9, 30),
                report_period=date(2024, 9, 30),
                available_date=date(2024, 10, 30), source="test", roe=0.12,
            ),
            FundamentalSnapshot(
                code="sh.600001", data_date=date(2024, 9, 30),
                report_period=date(2024, 9, 30),
                available_date=date(2025, 2, 1), source="test", roe=0.18,
            ),
        ])
        db.commit()

        payload = {
            "logic": "and",
            "conditions": [_condition("roe", "not_null")],
            "groups": [],
            "pool_id": None,
        }
        before = structured_screen(db, {**payload, "date": before_revision})
        after = structured_screen(db, {**payload, "date": after_revision})

    assert before["items"][0]["values"]["roe"] == pytest.approx(0.12)
    assert before["items"][0]["values"]["fundamental_available_date"] == "2024-10-30"
    assert after["items"][0]["values"]["roe"] == pytest.approx(0.18)
    assert after["items"][0]["values"]["fundamental_available_date"] == "2025-02-01"


def test_historical_pool_without_membership_history_fails_explicitly() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(IndexMember(
            id=1, index_name="hs300", code="sh.600001",
            in_date=date(2025, 1, 1),
        ))
        db.commit()

        # pool_id 指定非默认池时需要 user_id(预置池对所有登录用户可见)
        with pytest.raises(InvalidFilterError, match="缺少.*历史成分"):
            structured_screen(db, {
                "date": date(2024, 1, 10),
                "logic": "and",
                "conditions": [_condition("roe", "not_null")],
                "groups": [],
                "pool_id": 1,
            }, user_id="test-user")


def test_explicit_pool_requires_login() -> None:
    """指定 pool_id 必须登录:否则无法判断该池是否属于调用者(池可见性)。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.commit()

        with pytest.raises(InvalidFilterError, match="需要登录"):
            structured_screen(db, {
                "date": date(2025, 1, 10),
                "logic": "and",
                "conditions": [],
                "groups": [],
                "pool_id": 1,
            })


def test_stale_valuation_is_treated_as_missing() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(FactorDaily(id=1, code="sh.600001", date=day, values={}))
        db.add(ValuationSnapshot(
            code="sh.600001", data_date=date(2025, 1, 2),
            available_date=date(2025, 1, 2), report_period=None,
            source="test", pe_ttm=12,
        ))
        db.commit()

        result = structured_screen(db, {
            "date": day,
            "logic": "and",
            "conditions": [_condition("pe_ttm", "not_null")],
            "groups": [],
            "pool_id": None,
        })

    assert result["combined_count"] == 0
    assert result["field_coverage"] == {"pe_ttm": 0}
    assert result["data_policy"]["valuation_max_age_days"] == 7


def test_structured_screen_skips_listing_history_when_unused(monkeypatch) -> None:
    """普通筛选不应为了未使用的上市天数扫描全部日线。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)

    def fail_if_called(*args, **kwargs):
        raise AssertionError("未使用 listing_days 时不应统计历史日线")

    monkeypatch.setattr(screener, "_listing_days_by_code", fail_if_called)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(FactorDaily(id=1, code="sh.600001", date=day,
                           values={"mom20": 0.05}))
        db.add(DailyBar(
            code="sh.600001", date=day,
            open=10, high=11, low=9.9, close=10.5, raw_close=10.5,
            volume=120, amount=1200, is_st=False,
        ))
        db.commit()

        result = structured_screen(db, {
            "date": day,
            "logic": "and",
            "conditions": [_condition("industry", "eq", "制造")],
            "groups": [],
            "pool_id": None,
        })

    assert result["combined_count"] == 1
    assert result["items"][0]["values"]["close"] == pytest.approx(10.5)
    assert result["items"][0]["values"]["listing_days"] is None


def test_structured_screen_counts_listing_days_when_requested() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(FactorDaily(id=1, code="sh.600001", date=day, values={}))
        db.add_all([
            DailyBar(
                code="sh.600001", date=date(2025, 1, 9),
                open=10, high=10.5, low=9.8, close=10, raw_close=10,
                volume=100, amount=1000, is_st=False,
            ),
            DailyBar(
                code="sh.600001", date=day,
                open=10, high=11, low=9.9, close=11, raw_close=11,
                volume=120, amount=1200, is_st=False,
            ),
        ])
        db.commit()

        result = structured_screen(db, {
            "date": day,
            "logic": "and",
            "conditions": [_condition("listing_days", "gte", 2)],
            "groups": [],
            "pool_id": None,
        })

    assert result["combined_count"] == 1
    assert result["field_coverage"] == {"listing_days": 1}
    assert result["items"][0]["values"]["listing_days"] == 2


def test_structured_screen_dynamic_factor_filter() -> None:
    """可在启用因子上做结构化条件筛选。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)
    with Session(engine) as db:
        _seed_pools(db)
        seed_factor_defs(db)
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造",
                     list_date=date(2020, 1, 1)))
        db.add(FactorDaily(
            id=1, code="sh.600001", date=day,
            values={"mom20": 0.05, "vol_ratio5": 1.2, "amount_avg20": 1e8},
        ))
        db.add(DailyBar(
            code="sh.600001", date=day,
            open=10, high=11, low=9.9, close=11, raw_close=11,
            volume=120, amount=1200, is_st=False,
        ))
        db.commit()

        result = structured_screen(db, {
            "date": day,
            "logic": "and",
            "conditions": [
                _condition("mom20", "gte", 0.04),
                _condition("vol_ratio5", "gte", 1.0),
            ],
            "groups": [],
            "pool_id": None,
        })

    assert result["combined_count"] == 1
    assert result["items"][0]["values"]["mom20"] == pytest.approx(0.05)
