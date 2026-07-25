from datetime import date
import sys
from pathlib import Path

import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.db import Base
from app.models import (
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


def _condition(field: str, operator: str, value=None, value_to=None) -> dict:
    return {
        "id": f"{field}_{operator}",
        "field": field,
        "operator": operator,
        "value": value,
        "value_to": value_to,
        "enabled": True,
    }


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
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造"))
        db.add(FactorDaily(
            id=1, code="sh.600001", date=day, mom20=0.05, mom60=0.08,
            rsi14=55, atr_pct=0.02, vol_ratio5=1.2,
            ma20_slope=0.01, amount_avg20=1e8,
        ))
        db.add_all([
            DailyBar(
                code="sh.600001", date=date(2025, 1, 9),
                open=10, high=10.5, low=9.8, close=10, raw_close=10,
                volume=100, amount=1000,
            ),
            DailyBar(
                code="sh.600001", date=day,
                open=10, high=11, low=9.9, close=11, raw_close=11,
                volume=120, amount=1200,
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
            "universe": "all",
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
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造"))
        db.add_all([
            FactorDaily(id=1, code="sh.600001", date=before_revision),
            FactorDaily(id=2, code="sh.600001", date=after_revision),
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
            "universe": "all",
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
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造"))
        db.add(IndexMember(
            id=1, index_name="hs300", code="sh.600001",
            in_date=date(2025, 1, 1),
        ))
        db.commit()

        with pytest.raises(InvalidFilterError, match="缺少.*历史成分"):
            structured_screen(db, {
                "date": date(2024, 1, 10),
                "logic": "and",
                "conditions": [_condition("roe", "not_null")],
                "groups": [],
                "universe": "pool",
            })


def test_stale_valuation_is_treated_as_missing() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    day = date(2025, 1, 10)
    with Session(engine) as db:
        db.add(Stock(code="sh.600001", name="测试股份", industry="制造"))
        db.add(FactorDaily(id=1, code="sh.600001", date=day))
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
            "universe": "all",
        })

    assert result["combined_count"] == 0
    assert result["field_coverage"] == {"pe_ttm": 0}
    assert result["data_policy"]["valuation_max_age_days"] == 7
