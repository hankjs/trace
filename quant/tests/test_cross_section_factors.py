"""横截面因子算子 cs_* 与截面求值路径。"""
from __future__ import annotations

from datetime import date, timedelta

import numpy as np
import pandas as pd
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.db import Base
from app.factors.backfill import run_factor_backfill_task
from app.factors.engine import evaluate_factor_cross_section
from app.factors.evaluation import evaluate_factor_efficacy
from app.models import DailyBar, FactorDef, Stock, SYSTEM_OWNER_ID, Task
from app.strategy.components import evaluate_expression
from app.strategy.operators import INDUSTRY_FIELD_KEY, MIN_GROUP_SIZE, SUPPORTED_GROUP_BY
from app.strategy.spec import (
    SUPPORTED_FIELDS,
    expression_mode,
    parse_expression,
    validate_expression,
    validate_strategy_spec,
)
# client 夹具与 _send/_state 等辅助直接复用 test_a2a,避免两处 A2A 夹具漂移
from tests.test_a2a import (
    CLIENT_CLAIMS,
    _fail_text,
    _send,
    _state,
    _token,
    client,
)


def _frame(values: dict[str, list[float]]) -> pd.DataFrame:
    return pd.DataFrame(values, index=pd.date_range("2024-01-02", periods=1, freq="B"))


def test_cs_rank_outputs_pct_within_zero_one():
    frame = _frame({"a": [1.0], "b": [2.0], "c": [3.0]})
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"}, "group_by": None,
    })
    out = evaluate_expression(expr, {"x": frame})
    row = out.iloc[0]
    assert list(row) == pytest.approx([1 / 3, 2 / 3, 1.0])


def test_cs_rank_keeps_nan_out_of_ranking():
    frame = _frame({"a": [1.0], "b": [np.nan], "c": [3.0]})
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"}, "group_by": None,
    })
    out = evaluate_expression(expr, {"x": frame})
    row = out.iloc[0]
    assert np.isnan(row["b"])
    assert row["a"] == pytest.approx(0.5)
    assert row["c"] == pytest.approx(1.0)


def test_cs_zscore_zero_variance_row_is_nan():
    frame = _frame({"a": [5.0], "b": [5.0], "c": [5.0]})
    expr = parse_expression({
        "op": "cs_zscore", "input": {"op": "field", "name": "x"}, "group_by": None,
    })
    out = evaluate_expression(expr, {"x": frame})
    assert out.iloc[0].isna().all()


def test_cs_demean_sums_to_zero():
    frame = _frame({"a": [1.0], "b": [2.0], "c": [3.0]})
    expr = parse_expression({
        "op": "cs_demean", "input": {"op": "field", "name": "x"}, "group_by": None,
    })
    out = evaluate_expression(expr, {"x": frame})
    assert float(out.iloc[0].sum()) == pytest.approx(0.0, abs=1e-12)


def test_group_by_industry_computes_within_group():
    frame = _frame({
        "a": [1.0], "b": [2.0], "c": [3.0],
        "d": [10.0], "e": [20.0], "f": [30.0],
    })
    industries = pd.Series({
        "a": "X", "b": "X", "c": "X",
        "d": "Y", "e": "Y", "f": "Y",
    })
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"},
        "group_by": "industry",
    })
    out = evaluate_expression(expr, {"x": frame, INDUSTRY_FIELD_KEY: industries})
    row = out.iloc[0]
    assert list(row[["a", "b", "c"]]) == pytest.approx([1 / 3, 2 / 3, 1.0])
    assert list(row[["d", "e", "f"]]) == pytest.approx([1 / 3, 2 / 3, 1.0])


def test_small_group_outputs_nan():
    frame = _frame({"a": [1.0], "b": [2.0], "c": [3.0], "d": [4.0], "e": [5.0]})
    industries = pd.Series({
        "a": "small", "b": "small",  # size 2 < MIN_GROUP_SIZE
        "c": "big", "d": "big", "e": "big",
    })
    assert MIN_GROUP_SIZE == 3
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"},
        "group_by": "industry",
    })
    out = evaluate_expression(expr, {"x": frame, INDUSTRY_FIELD_KEY: industries})
    row = out.iloc[0]
    assert np.isnan(row["a"]) and np.isnan(row["b"])
    assert not np.isnan(row["c"])


def test_cs_op_on_series_raises_with_hint():
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"}, "group_by": None,
    })
    with pytest.raises(ValueError, match="只能用于横截面") as ei:
        evaluate_expression(expr, {"x": pd.Series([1.0, 2.0, 3.0])})
    assert "rolling_rank" in str(ei.value)


def test_group_by_without_industry_context_raises():
    frame = _frame({"a": [1.0], "b": [2.0], "c": [3.0]})
    expr = parse_expression({
        "op": "cs_rank", "input": {"op": "field", "name": "x"},
        "group_by": "industry",
    })
    with pytest.raises(ValueError, match="未提供"):
        evaluate_expression(expr, {"x": frame})


def test_time_series_mode_rejects_cs_op():
    result = validate_expression(
        {"op": "cs_rank", "input": {"op": "field", "name": "close"}, "group_by": None},
        mode="time_series",
    )
    assert result.valid is False
    assert any(i.code == "expression_mode_mismatch" for i in result.capability.issues)


def test_cross_section_mode_rejects_pure_time_series():
    result = validate_expression(
        {"op": "field", "name": "close"},
        mode="cross_section",
    )
    assert result.valid is False
    assert any(i.code == "expression_mode_mismatch" for i in result.capability.issues)


def test_expression_mode_detects_rank_and_top_n():
    assert expression_mode(parse_expression({
        "op": "rank", "input": {"op": "field", "name": "close"}, "ascending": False,
    })) == "cross_section"
    assert expression_mode(parse_expression({
        "op": "top_n", "input": {"op": "field", "name": "close"}, "n": 5,
    })) == "cross_section"


def test_invalid_group_by_rejected():
    with pytest.raises(ValueError, match="group_by"):
        parse_expression({
            "op": "cs_rank",
            "input": {"op": "field", "name": "close"},
            "group_by": "market_cap",
        })
    assert "industry" in SUPPORTED_GROUP_BY


def test_industry_not_in_supported_fields():
    assert "industry" not in SUPPORTED_FIELDS
    result = validate_expression(
        {"op": "field", "name": "industry"},
        available_fields=SUPPORTED_FIELDS,
    )
    assert result.valid is False


def test_preview_rejects_cross_section_factor(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.preview",
        {
            "expression": {
                "op": "cs_rank",
                "input": {"op": "field", "name": "close"},
                "group_by": None,
            },
            "codes": ["sh.600000"],
        },
    )
    assert _state(result) == "failed"
    assert "抽查" in _fail_text(result)


def test_backfill_rejects_cross_section_factor():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        db.add(FactorDef(
            key="cs_draft", name="截面草稿",
            expression={
                "op": "cs_rank",
                "input": {"op": "field", "name": "close"},
                "group_by": None,
            },
            expression_hash="h", min_bars=1, enabled=False,
            is_system=False, owner_id="user-a",
        ))
        db.commit()
        task = Task(
            user_id="user-a", type="factor_backfill", status="pending",
            title="bf",
            params={
                "factor_key": "cs_draft",
                "start": "2024-01-01",
                "end": "2024-01-31",
                "owner_id": "user-a",
                "is_admin": False,
            },
        )
        with pytest.raises(ValueError, match="不支持回填"):
            run_factor_backfill_task(db, task)


def test_strategy_spec_rejects_cs_ops():
    from app.strategy.presets import get_preset_spec
    base = get_preset_spec("ma_cross").model_dump(mode="json")
    # inject cs_rank into entry condition
    base["entry"]["condition"] = {
        "op": "cs_rank",
        "input": {"op": "field", "name": "close"},
        "group_by": None,
    }
    result = validate_strategy_spec(base)
    assert result.valid is False
    assert any(
        i.code == "factor_only_operator" for i in result.capability.issues
    )


def test_evaluate_industry_momentum_rank_end_to_end():
    """G1:行业内动量 cs_rank 评估,ic_decay/layers 非空且声明非 PIT。"""
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    codes = [f"sh.{600000 + i:06d}" for i in range(12)]
    start = date(2024, 1, 2)
    n_days = 80
    with Session(engine) as db:
        for i, code in enumerate(codes):
            db.add(Stock(
                code=code, name=f"s{i}", list_date=date(2015, 1, 1),
                is_st=False,
                industry="制造" if i < 6 else "金融",
            ))
            base = 10.0 + i * 0.2
            for d in range(n_days):
                day = start + timedelta(days=d)
                close = base * ((1 + 0.001 * (i + 1)) ** d)
                db.add(DailyBar(
                    code=code, date=day,
                    open=close, high=close * 1.01, low=close * 0.99,
                    close=close, raw_close=close,
                    volume=1e6, amount=1e7, is_st=False,
                ))
        db.commit()
        expr = {
            "op": "cs_rank",
            "input": {
                "op": "momentum",
                "input": {"op": "field", "name": "close"},
                "window": 20,
            },
            "group_by": "industry",
        }
        row = evaluate_factor_efficacy(
            db,
            user_id="user-a",
            expression=expr,
            start=start,
            end=start + timedelta(days=70),
            codes=codes,
            layers=3,
            rebalance="weekly",
        )
    assert row.status == "done"
    result = row.result or {}
    assert result["cross_section"]["is_cross_section"] is True
    assert result["cross_section"]["group_by"] == "industry"
    assert "非 PIT" in (result["cross_section"]["note"] or "")
    assert result["ic_decay"]
    assert result["layers"]
    assert result["ic"]["n_periods"] >= 1
