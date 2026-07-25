from datetime import date
import sys
from pathlib import Path

import pandas as pd
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import fundamentals
from app.db import Base
from app.models import FundamentalSnapshot, ValuationSnapshot


def test_xq_valuation_normalizes_percent_and_market_code(monkeypatch) -> None:
    captured = {}

    def fake_fetch(*, symbol: str, timeout: int):
        captured.update(symbol=symbol, timeout=timeout)
        return pd.DataFrame([
            {"item": "时间", "value": "2025-01-10 15:00:00"},
            {"item": "市盈率(TTM)", "value": 18.5},
            {"item": "市净率", "value": 3.2},
            {"item": "市销率", "value": 4.1},
            {"item": "股息率(TTM)", "value": 2.5},
            {"item": "资产净值/总市值", "value": 1.2e11},
        ])

    monkeypatch.setattr(fundamentals.ak, "stock_individual_spot_xq", fake_fetch)
    row = fundamentals._fetch_xq_valuation(  # noqa: SLF001
        "sh.600001", date(2025, 1, 10),
    )

    assert captured == {"symbol": "SH600001", "timeout": 12}
    assert row["pe_ttm"] == 18.5
    assert row["dividend_yield"] == 0.025
    assert row["total_market_cap"] == 1.2e11


def test_stock_value_em_latest_and_history(monkeypatch) -> None:
    def fake_fetch(*, symbol: str):
        assert symbol == "600001"
        return pd.DataFrame([
            {
                "数据日期": date(2025, 1, 9), "PE(TTM)": 15.0,
                "市净率": 2.0, "市销率": 3.0, "总市值": 1e10,
            },
            {
                "数据日期": date(2025, 1, 10), "PE(TTM)": 16.0,
                "市净率": 2.1, "市销率": 3.1, "总市值": 1.1e10,
            },
            {
                "数据日期": date(2025, 1, 11), "PE(TTM)": 99.0,
                "市净率": 9.0, "市销率": 9.0, "总市值": 9e10,
            },
        ])

    monkeypatch.setattr(fundamentals.ak, "stock_value_em", fake_fetch)
    latest = fundamentals._fetch_em_valuations(  # noqa: SLF001
        "sh.600001", date(2025, 1, 10), history=False,
    )
    history = fundamentals._fetch_em_valuations(  # noqa: SLF001
        "sh.600001", date(2025, 1, 10), history=True,
    )

    assert [row["data_date"] for row in latest] == [date(2025, 1, 10)]
    assert latest[0]["pe_ttm"] == 16.0
    assert [row["data_date"] for row in history] == [
        date(2025, 1, 9), date(2025, 1, 10),
    ]


def test_financial_em_uses_update_date_and_excludes_future(monkeypatch) -> None:
    def fake_fetch(*, symbol: str, indicator: str):
        assert symbol == "600001.SH"
        assert indicator == "按报告期"
        return pd.DataFrame([
            {
                "REPORT_DATE": "2024-09-30", "NOTICE_DATE": "2024-10-20",
                "UPDATE_DATE": "2024-10-30", "ROEJQ": 12.5,
                "TOTALOPERATEREVETZ": 8.0, "PARENTNETPROFITTZ": 6.0,
                "XSMLL": 40.0, "XSJLL": 15.0, "ZCFZL": 35.0,
                "NCO_NETPROFIT": 1.1,
            },
            {
                "REPORT_DATE": "2024-12-31", "NOTICE_DATE": "2025-03-20",
                "UPDATE_DATE": "2025-03-20", "ROEJQ": 20.0,
            },
        ])

    monkeypatch.setattr(
        fundamentals.ak, "stock_financial_analysis_indicator_em", fake_fetch,
    )
    rows = fundamentals.fetch_financials("sh.600001", date(2025, 1, 10))

    assert len(rows) == 1
    assert rows[0]["available_date"] == date(2024, 10, 30)
    assert rows[0]["roe"] == 0.125
    assert rows[0]["gross_margin"] == 0.4
    assert rows[0]["cashflow_ratio"] == 1.1


def test_sync_reports_network_failures_without_raising(monkeypatch) -> None:
    def fail(*args, **kwargs):
        raise ConnectionError("offline")

    monkeypatch.setattr(fundamentals, "fetch_valuations", fail)
    monkeypatch.setattr(fundamentals, "fetch_financials", fail)
    monkeypatch.setattr(fundamentals, "_fetch_spot_valuation_map", fail)
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        result = fundamentals.sync_fundamentals(db, ["sh.600001"])

    assert result["valuation_upserted"] == 0
    assert result["financial_upserted"] == 0
    assert {(item["stage"], item["code"]) for item in result["failures"]} == {
        ("valuation", "sh.600001"),
        ("financial", "sh.600001"),
    }


def test_one_stock_failure_does_not_block_next_stock(monkeypatch) -> None:
    def fake_valuations(code: str, as_of: date, history: bool):
        if code == "sh.600001":
            raise ConnectionError("first stock offline")
        return [{
            "code": code,
            "data_date": as_of,
            "report_period": None,
            "available_date": as_of,
            "source": "test",
            "pe_ttm": 10.0,
            "pb": 1.0,
            "ps_ttm": 2.0,
            "dividend_yield": 0.03,
            "total_market_cap": 1e10,
        }]

    monkeypatch.setattr(fundamentals, "fetch_valuations", fake_valuations)
    monkeypatch.setattr(fundamentals, "_fetch_spot_valuation_map", lambda *args: {})
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        result = fundamentals.sync_fundamentals(
            db, ["sh.600001", "sh.600002"], include_financials=False,
        )
        stored = db.query(ValuationSnapshot).all()

    assert result["valuation_upserted"] == 1
    assert [row.code for row in stored] == ["sh.600002"]
    assert result["failures"][0]["code"] == "sh.600001"


def test_historical_valuation_never_falls_back_to_current_snapshot(monkeypatch) -> None:
    def fail_history(*args, **kwargs):
        raise ConnectionError("history unavailable")

    current_called = False

    def current_snapshot(*args, **kwargs):
        nonlocal current_called
        current_called = True
        return {}

    monkeypatch.setattr(fundamentals, "_fetch_em_valuations", fail_history)
    monkeypatch.setattr(fundamentals, "_fetch_xq_valuation", current_snapshot)

    with pytest.raises(RuntimeError, match="历史估值不可用"):
        fundamentals.fetch_valuations(
            "sh.600001", date(2025, 1, 10), history=True,
        )
    assert current_called is False


def test_financial_revision_is_stored_as_new_available_version() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    original = {
        "code": "sh.600001",
        "data_date": date(2024, 9, 30),
        "report_period": date(2024, 9, 30),
        "available_date": date(2024, 10, 30),
        "source": "test",
        "roe": 0.12,
    }
    revision = {
        **original,
        "available_date": date(2025, 2, 1),
        "roe": 0.18,
    }

    with Session(engine) as db:
        fundamentals._upsert_financial(db, original)  # noqa: SLF001
        fundamentals._upsert_financial(db, revision)  # noqa: SLF001
        db.commit()
        rows = db.query(FundamentalSnapshot).order_by(
            FundamentalSnapshot.available_date,
        ).all()

    assert [(row.available_date, row.roe) for row in rows] == [
        (date(2024, 10, 30), 0.12),
        (date(2025, 2, 1), 0.18),
    ]
