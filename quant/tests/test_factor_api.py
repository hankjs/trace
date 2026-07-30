"""因子 CRUD、校验、预览与目录 API 行为断言。"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

import pytest
from fastapi import HTTPException
from sqlalchemy import create_engine, select
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from starlette.requests import Request

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.factors import (
    FactorBackfillIn,
    FactorCreateIn,
    FactorPatchIn,
    FactorPreviewIn,
    FactorValidateIn,
    SelectionConfigPutIn,
    backfill_factors,
    create_factor,
    delete_factor,
    get_factor,
    get_selection_config,
    list_factors,
    preview_factor,
    update_factor,
    update_selection_config,
    validate_factor_expression,
)
from app.auth import create_token, require_admin
from app.db import Base
from app.models import DailyBar, FactorDef, Pool
from app.models import SYSTEM_OWNER_ID
from tests.factories import seed_factor_defs, seed_selection_config, seed_stock


def _request(claims: dict) -> Request:
    user = {
        "id": claims["sub"],
        "username": claims["username"],
        "can_admin": claims.get("can_admin", False),
        "can_client": claims.get("can_client", True),
    }
    return Request({
        "type": "http",
        "headers": [(b"authorization", f"Bearer {create_token(user)}".encode())],
    })

USER = "11111111-1111-1111-1111-111111111111"
CLAIMS_USER = {"sub": USER, "username": "user", "can_client": True}
CLAIMS_ADMIN = {"sub": USER, "username": "admin", "can_client": True, "can_admin": True}
CLAIMS_NON_ADMIN = {"sub": USER, "username": "nonadmin", "can_client": True, "can_admin": False}


def _db():
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _valid_expression():
    return {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 20}


def test_list_factors_returns_seed_items():
    with _db() as db:
        seed_factor_defs(db)
        result = list_factors(db=db, _claims=CLAIMS_USER)
    assert len(result["items"]) == 7
    assert all(isinstance(item["key"], str) for item in result["items"])


def test_get_factor_404_for_unknown():
    with _db() as db:
        seed_factor_defs(db)
        with pytest.raises(HTTPException) as exc:
            get_factor("notexist", db=db, _claims=CLAIMS_USER)
    assert exc.value.status_code == 404


def test_create_factor_happy_path():
    with _db() as db:
        body = FactorCreateIn(
            key="custom_mom",
            name="自定义动量",
            expression=_valid_expression(),
        )
        created = create_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert created["key"] == "custom_mom"
    assert created["expression_hash"] is not None
    assert created["min_bars"] == 21
    assert created["enabled"] is True


def test_create_factor_rejects_non_admin():
    with pytest.raises(HTTPException) as exc:
        require_admin(_request(CLAIMS_NON_ADMIN))
    assert exc.value.status_code == 403


def test_create_factor_accepts_admin_via_dependency():
    assert require_admin(_request(CLAIMS_ADMIN))["sub"] == USER


def test_create_factor_duplicate_key():
    with _db() as db:
        seed_factor_defs(db)
        body = FactorCreateIn(
            key="mom20",
            name="重复",
            expression=_valid_expression(),
        )
        with pytest.raises(HTTPException) as exc:
            create_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert exc.value.status_code == 409


def test_create_factor_rejects_reserved_key():
    with _db() as db:
        body = FactorCreateIn(
            key="close",
            name="保留字",
            expression=_valid_expression(),
        )
        with pytest.raises(HTTPException) as exc:
            create_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert exc.value.status_code == 409


def test_create_factor_invalid_expression():
    with _db() as db:
        body = FactorCreateIn(
            key="bad",
            name="Bad",
            expression={"op": "unknown_op"},
        )
        with pytest.raises(HTTPException) as exc:
            create_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert exc.value.status_code == 422


def test_update_factor_expression_recomputes_hash():
    with _db() as db:
        seed_factor_defs(db)
        body = FactorPatchIn(enabled=False)
        updated = update_factor("mom20", body, db=db, _claims=CLAIMS_ADMIN)
    assert updated["enabled"] is False


def test_delete_system_factor_rejected():
    with _db() as db:
        seed_factor_defs(db)
        with pytest.raises(HTTPException) as exc:
            delete_factor("mom20", db=db, _claims=CLAIMS_ADMIN)
    assert exc.value.status_code == 409
    assert "只能禁用" in exc.value.detail


def test_delete_custom_factor_ok():
    with _db() as db:
        body = FactorCreateIn(
            key="deletable",
            name="可删除",
            expression=_valid_expression(),
        )
        create_factor(body, db=db, _claims=CLAIMS_ADMIN)
        delete_factor("deletable", db=db, _claims=CLAIMS_ADMIN)
        assert db.execute(
            select(FactorDef).where(FactorDef.key == "deletable")
        ).scalar_one_or_none() is None


def test_validate_endpoint_shape():
    with _db() as db:
        result = validate_factor_expression(
            FactorValidateIn(expression=_valid_expression()),
            db=db, _claims=CLAIMS_ADMIN,
        )
    assert result["valid"] is True
    assert result["expression_hash"] is not None
    assert result["min_bars"] == 21
    assert result["result_type"] == "number"


def test_preview_by_expression_returns_aligned_data():
    with _db() as db:
        seed_stock(db, "sh.600001")
        # 创建足够长的历史日线
        for i in range(100):
            from app.models import DailyBar
            db.add(DailyBar(
                code="sh.600001",
                date=date(2024, 1, 1) + timedelta(days=i),
                open=10, high=11, low=9, close=10 + i * 0.01,
                raw_close=10 + i * 0.01,
                volume=1000, amount=10000, is_st=False,
            ))
        db.commit()
        body = FactorPreviewIn(
            expression=_valid_expression(),
            code="sh.600001",
            days=30,
        )
        result = preview_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert result["code"] == "sh.600001"
    assert len(result["dates"]) == 30
    assert len(result["values"]) == 30
    assert result["reason_tree"]["op"] == "momentum"


def test_preview_by_factor_key():
    with _db() as db:
        seed_factor_defs(db)
        seed_stock(db, "sh.600001")
        for i in range(100):
            from app.models import DailyBar
            db.add(DailyBar(
                code="sh.600001",
                date=date(2024, 1, 1) + timedelta(days=i),
                open=10, high=11, low=9, close=10 + i * 0.01,
                raw_close=10 + i * 0.01,
                volume=1000, amount=10000, is_st=False,
            ))
        db.commit()
        body = FactorPreviewIn(factor_key="mom20", code="sh.600001", days=30)
        result = preview_factor(body, db=db, _claims=CLAIMS_ADMIN)
    assert len(result["values"]) == 30


def test_selection_config_get_and_put():
    with _db() as db:
        seed_factor_defs(db)
        seed_selection_config(db)
        before = get_selection_config(db=db, _claims=CLAIMS_USER)
        assert before["top_n"] == 30
        body = SelectionConfigPutIn(
            score_weights={"mom20": 0.5, "mom60": 0.5},
            vol_confirm={"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            hard_filters=[{"type": "exclude_st"}],
            top_n=10,
        )
        after = update_selection_config(body, db=db, _claims=CLAIMS_ADMIN)
    assert after["top_n"] == 10
    assert after["score_weights"] == {"mom20": 0.5, "mom60": 0.5}


def test_selection_config_put_rejects_non_admin():
    with pytest.raises(HTTPException) as exc:
        require_admin(_request(CLAIMS_NON_ADMIN))
    assert exc.value.status_code == 403


def test_selection_config_put_invalid_weight_key():
    with _db() as db:
        seed_factor_defs(db)
        seed_selection_config(db)
        body = SelectionConfigPutIn(
            score_weights={"unknown_factor": 1.0},
            vol_confirm={"factor": "vol_ratio5", "cap": 3.0, "weight": 0.0},
            hard_filters=[{"type": "exclude_st"}],
            top_n=10,
        )
        with pytest.raises(HTTPException) as exc:
            update_selection_config(body, db=db, _claims=CLAIMS_ADMIN)
    assert exc.value.status_code == 422


def test_catalog_includes_new_factor_after_create():
    from app.catalog import catalog_payload

    with _db() as db:
        seed_factor_defs(db)
        body = FactorCreateIn(
            key="custom_mom",
            name="自定义动量",
            expression=_valid_expression(),
        )
        create_factor(body, db=db, _claims=CLAIMS_ADMIN)
        payload = catalog_payload(db)
    keys = {item["key"] for item in payload["factors"]}
    assert "custom_mom" in keys


def test_screener_accepts_condition_on_new_key():
    from app.models import DailyBar, FactorDaily, Pool
    from app.models import SYSTEM_OWNER_ID
    from app.selection.screener import structured_screen

    with _db() as db:
        seed_factor_defs(db)
        seed_stock(db, "sh.600001", list_date=date(2020, 1, 1))
        db.add(Pool(
            id=2, kind="all", ref=None, owner_id=SYSTEM_OWNER_ID, is_system=True,
            name="全部A股", min_list_days=0,
        ))
        body = FactorCreateIn(
            key="custom_mom",
            name="自定义动量",
            expression=_valid_expression(),
        )
        create_factor(body, db=db, _claims=CLAIMS_ADMIN)
        db.add(DailyBar(
            code="sh.600001", date=date(2025, 1, 10),
            open=10, high=11, low=9, close=11, raw_close=11,
            volume=1000, amount=10000, is_st=False,
        ))
        db.add(FactorDaily(
            code="sh.600001", date=date(2025, 1, 10),
            values={"custom_mom": 0.05},
        ))
        db.commit()
        result = structured_screen(db, {
            "date": date(2025, 1, 10),
            "logic": "and",
            "conditions": [{
                "id": "custom",
                "field": "custom_mom",
                "operator": "gte",
                "value": 0.04,
                "enabled": True,
            }],
            "groups": [],
            "pool_id": None,
        })
    assert result["combined_count"] == 1


def test_backfill_submits_task():
    from app.tasks import HANDLERS

    with _db() as db:
        seed_factor_defs(db)
        seed_selection_config(db)
        body = FactorBackfillIn(
            factor_key=None,
            start=date(2024, 1, 1),
            end=date(2024, 1, 5),
        )
        result = backfill_factors(body, db=db, claims=CLAIMS_ADMIN)
    assert result["task"]["type"] == "factor_backfill"
    assert "factor_backfill" in HANDLERS
