"""GET /api/admin/a2a-gaps 管理端缺口排行测试。"""
from __future__ import annotations

import sys
from datetime import date, datetime, timedelta
from pathlib import Path

import jwt
import pytest
from fastapi.testclient import TestClient
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# 在导入 app.main 之前禁用 Alembic 版本校验
from app import migrations as migrations_mod  # noqa: E402

migrations_mod.check_schema_version = lambda engine, *, strict=None: True

from app import db as app_db  # noqa: E402
from app.api import admin  # noqa: E402
from app.config import settings  # noqa: E402
from app.db import Base  # noqa: E402
from app.main import app  # noqa: E402
from app.models import A2aAudit, ResearchFinding  # noqa: E402

ADMIN_CLAIMS = {
    "sub": "user-admin",
    "username": "admin",
    "can_admin": True,
    "can_client": True,
}
CLIENT_CLAIMS = {
    "sub": "user-a",
    "username": "a",
    "can_admin": False,
    "can_client": True,
}


def _token(claims: dict) -> str:
    return jwt.encode(claims, settings.jwt_secret, algorithm="HS256")


@pytest.fixture
def client(monkeypatch, tmp_path):
    """独立 SQLite 库,admin 端点使用同一 SessionLocal。"""
    db_path = tmp_path / "admin_gaps.db"
    engine = create_engine(f"sqlite+pysqlite:///{db_path}")
    SessionLocal = sessionmaker(bind=engine)
    Base.metadata.create_all(engine)

    monkeypatch.setattr(app_db, "SessionLocal", SessionLocal)
    monkeypatch.setattr(admin, "SessionLocal", SessionLocal)

    with TestClient(app) as c:
        yield c


def _get(client: TestClient, token: str | None, limit: int = 20, since_days: int = 30):
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    return client.get(
        f"/api/admin/a2a-gaps?limit={limit}&since_days={since_days}",
        headers=headers,
    )


def test_a2a_gaps_requires_auth(client):
    resp = _get(client, None)
    assert resp.status_code == 401


def test_a2a_gaps_requires_admin(client):
    resp = _get(client, _token(CLIENT_CLAIMS))
    assert resp.status_code == 403


def test_a2a_gaps_returns_empty_by_default(client):
    resp = _get(client, _token(ADMIN_CLAIMS))
    assert resp.status_code == 200
    data = resp.json()
    assert data["audit_items"] == []
    assert data["finding_items"] == []
    assert data["merged"] == []
    assert "aggregate_of_a2a_audit" in data["note"]


def test_a2a_gaps_aggregates_audit_and_findings(client):
    today = datetime.combine(date.today(), datetime.min.time())
    SessionLocal = app_db.SessionLocal
    with SessionLocal() as db:
        db.add(A2aAudit(
            user_id="user-a",
            a2a_task_id="t1",
            skill="strategy.validate",
            source="test",
            failure_kind="missing_engine",
            missing_capability="rolling_foo",
            created_at=today,
        ))
        db.add(A2aAudit(
            user_id="user-b",
            a2a_task_id="t2",
            skill="strategy.validate",
            source="test",
            failure_kind="missing_engine",
            missing_capability="rolling_foo",
            created_at=today,
        ))
        db.add(ResearchFinding(
            user_id="user-a",
            kind="missing_engine",
            detail="rolling_foo",
            source="test",
            created_at=today,
        ))
        db.commit()

    resp = _get(client, _token(ADMIN_CLAIMS))
    assert resp.status_code == 200
    data = resp.json()

    assert len(data["audit_items"]) == 1
    audit = data["audit_items"][0]
    assert audit["missing_capability"] == "rolling_foo"
    assert audit["failure_kind"] == "missing_engine"
    assert audit["count"] == 2
    assert audit["source"] == "audit"

    assert len(data["finding_items"]) == 1
    finding = data["finding_items"][0]
    assert finding["missing_capability"] == "rolling_foo"
    assert finding["failure_kind"] == "missing_engine"
    assert finding["count"] == 1
    assert finding["source"] == "finding"

    assert len(data["merged"]) == 1
    merged = data["merged"][0]
    assert merged["missing_capability"] == "rolling_foo"
    assert merged["failure_kind"] == "missing_engine"
    assert merged["count"] == 3


def test_a2a_gaps_respects_since_days(client):
    today = datetime.combine(date.today(), datetime.min.time())
    old = today - timedelta(days=40)
    SessionLocal = app_db.SessionLocal
    with SessionLocal() as db:
        db.add(A2aAudit(
            user_id="user-a",
            a2a_task_id="t1",
            skill="strategy.validate",
            source="test",
            failure_kind="missing_engine",
            missing_capability="new_foo",
            created_at=today,
        ))
        db.add(A2aAudit(
            user_id="user-a",
            a2a_task_id="t2",
            skill="strategy.validate",
            source="test",
            failure_kind="missing_engine",
            missing_capability="old_foo",
            created_at=old,
        ))
        db.commit()

    resp = _get(client, _token(ADMIN_CLAIMS), since_days=30)
    assert resp.status_code == 200
    data = resp.json()
    assert [i["missing_capability"] for i in data["audit_items"]] == ["new_foo"]
    assert [i["missing_capability"] for i in data["merged"]] == ["new_foo"]


def test_a2a_gaps_limit_and_since_days_clamped(client):
    """limit >50 与 since_days >90 应被 clamp,不报错。"""
    resp = _get(client, _token(ADMIN_CLAIMS), limit=100, since_days=180)
    assert resp.status_code == 200


def test_a2a_gaps_same_as_skill_aggregation(client):
    """REST 端点与 system.gap_summary skill 调用同一 aggregate_gaps,结果一致。"""
    from app.a2a.gaps import aggregate_gaps

    today = datetime.combine(date.today(), datetime.min.time())
    SessionLocal = app_db.SessionLocal
    with SessionLocal() as db:
        db.add(A2aAudit(
            user_id="user-a",
            a2a_task_id="t1",
            skill="factor.validate",
            source="test",
            failure_kind="missing_data",
            missing_capability="fundamental_pe",
            created_at=today,
        ))
        db.add(ResearchFinding(
            user_id="user-b",
            kind="missing_data",
            detail="fundamental_pe",
            source="test",
            created_at=today,
        ))
        db.commit()

    with SessionLocal() as db:
        direct = aggregate_gaps(db, user_id=None, scope="global", limit=20, since_days=30)

    resp = _get(client, _token(ADMIN_CLAIMS))
    assert resp.status_code == 200
    api_data = resp.json()

    assert api_data["audit_items"] == direct["audit_items"]
    assert api_data["finding_items"] == direct["finding_items"]
    assert api_data["merged"] == direct["merged"]
    assert api_data["note"] == direct["note"]
