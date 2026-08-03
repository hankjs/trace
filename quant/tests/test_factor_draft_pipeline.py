"""因子草稿沉淀链路:save_draft 放开 + backfill 归属守卫。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from sqlalchemy import select

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import db as app_db  # noqa: E402
from app.factors.evaluation import _load_saved_factor_values  # noqa: E402
from app.models import DailyBar, FactorDaily, FactorDef  # noqa: E402
from tests.factories import seed_factor_defs, seed_stock  # noqa: E402
from tests.test_a2a import (  # noqa: E402
    ADMIN_CLAIMS,
    CLIENT_CLAIMS,
    NOBODY_CLAIMS,
    _artifact_data,
    _fail_text,
    _send,
    _state,
    _token,
    client,  # re-export fixture
)


def _seed_bars(db, code: str, start: date, end: date) -> None:
    rng = __import__("numpy").random.default_rng(42)
    current = start
    close = 10.0
    while current <= end:
        if current.weekday() < 5:
            ret = rng.normal(0.0005, 0.02)
            close *= (1 + ret)
            db.add(DailyBar(
                code=code, date=current,
                open=close, high=close * 1.02, low=close * 0.98,
                close=close, raw_close=close,
                volume=1000, amount=10000, is_st=False,
            ))
        current += timedelta(days=1)
    db.flush()


def test_save_draft_allows_client_and_records_owner(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "draft_mom_client",
            "name": "客户端草稿",
            "expression": {"op": "field", "name": "close"},
        },
    )
    assert _state(result) == "completed"
    draft = _artifact_data(result, "factor_draft")["factor_draft"]
    assert draft["enabled"] is False
    assert draft["is_system"] is False
    assert draft["owner_id"] == CLIENT_CLAIMS["sub"]
    assert draft.get("parent_factor_key") is None


def test_save_draft_still_rejects_enabled_true(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "draft_enabled_true",
            "name": "不得启用",
            "expression": {"op": "field", "name": "close"},
            "enabled": True,
        },
    )
    assert _state(result) == "failed"
    assert "enabled:true" in _fail_text(result)


def test_save_draft_rejects_unreadable_parent(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "draft_with_bad_parent",
            "name": "坏父因子",
            "expression": {"op": "field", "name": "close"},
            "parent_factor_key": "no_such_parent_key",
        },
    )
    assert _state(result) == "failed"
    assert "不存在或不可读" in _fail_text(result)


def test_backfill_rejects_other_users_draft(client):
    save = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "draft_owned_by_a",
            "name": "A 的草稿",
            "expression": {"op": "field", "name": "close"},
        },
    )
    assert _state(save) == "completed"

    other = {
        "sub": "user-b",
        "username": "b",
        "can_admin": False,
        "can_client": True,
    }
    result = _send(
        client,
        _token(other),
        "factor.backfill",
        {
            "factor_key": "draft_owned_by_a",
            "start": "2024-01-01",
            "end": "2024-01-31",
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    assert "无权回填" in _fail_text(result)


def test_backfill_rejects_system_factor_for_client(client):
    with app_db.SessionLocal() as db:
        seed_factor_defs(db)
        db.commit()

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.backfill",
        {
            "factor_key": "mom20",
            "start": "2024-01-01",
            "end": "2024-01-31",
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    assert "无权回填" in _fail_text(result)


def test_backfill_requires_factor_key(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.backfill",
        {
            "start": "2024-01-01",
            "end": "2024-01-31",
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    assert "仅管理员可用" in _fail_text(result)


def test_backfill_is_high_cost(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.backfill",
        {
            "factor_key": "any_key",
            "start": "2024-01-01",
            "end": "2024-01-31",
        },
    )
    assert _state(result) == "failed"
    assert "高成本" in _fail_text(result)


def test_draft_pipeline_end_to_end(client):
    """save_draft → backfill → 日值表非空,按 factor_key 可读。"""
    start = date(2024, 1, 1)
    end = date(2024, 2, 29)
    code = "sh.600001"

    with app_db.SessionLocal() as db:
        seed_stock(db, code)
        _seed_bars(db, code, start, end)
        db.commit()

    save = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "draft_e2e_close",
            "name": "端到端草稿",
            "expression": {"op": "field", "name": "close"},
        },
    )
    assert _state(save) == "completed"
    draft = _artifact_data(save, "factor_draft")["factor_draft"]
    assert draft["owner_id"] == CLIENT_CLAIMS["sub"]

    backfill = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.backfill",
        {
            "factor_key": "draft_e2e_close",
            "start": start.isoformat(),
            "end": end.isoformat(),
            "codes": [code],
            "confirmed": True,
        },
    )
    assert _state(backfill) == "completed", _fail_text(backfill)
    art = _artifact_data(backfill, "factor_backfill")["factor_backfill"]
    assert art["factor_key"] == "draft_e2e_close"
    assert art["rows_written"] >= 1

    with app_db.SessionLocal() as db:
        rows = db.execute(
            select(FactorDaily).where(FactorDaily.code == code)
        ).scalars().all()
        assert any(
            "draft_e2e_close" in (r.values or {}) for r in rows
        ), "回填后 quant_factor_daily 应出现草稿 key"

        dates = {r.date for r in rows if "draft_e2e_close" in (r.values or {})}
        loaded = _load_saved_factor_values(
            db, "draft_e2e_close", [code], dates,
        )
        assert loaded, "按 factor_key 读日值不得为空"
