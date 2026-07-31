"""A2A JSON-RPC 集成测试。"""
from __future__ import annotations

import sys
from pathlib import Path

import jwt
import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# 在导入 app.main 之前禁用 Alembic 版本校验,避免测试库没有 alembic 版本记录
from app import migrations as migrations_mod  # noqa: E402

migrations_mod.check_schema_version = lambda engine, *, strict=None: True

from app import db as app_db  # noqa: E402
from app.a2a import server as a2a_server  # noqa: E402
from app.a2a import tasks as a2a_tasks  # noqa: E402
from app.backtest import jobs as backtest_jobs  # noqa: E402
from app.config import settings  # noqa: E402
from app.db import Base  # noqa: E402
from app.main import app  # noqa: E402
import app.main as app_main  # noqa: E402
from app.models import DailyBar, Stock  # noqa: E402
from app.strategy.presets import get_preset_spec  # noqa: E402
from app.strategy.spec import strategy_spec_hash  # noqa: E402
from app.tasks import SessionLocal as _  # noqa: E402,F401
import app.tasks as tasks_mod  # noqa: E402
from datetime import date, timedelta  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

CLIENT_CLAIMS = {
    "sub": "user-a",
    "username": "a",
    "can_admin": False,
    "can_client": True,
}
ADMIN_CLAIMS = {
    "sub": "user-admin",
    "username": "admin",
    "can_admin": True,
    "can_client": True,
}
NOBODY_CLAIMS = {
    "sub": "user-x",
    "username": "x",
    "can_admin": False,
    "can_client": False,
}


def _token(claims: dict) -> str:
    return jwt.encode(claims, settings.jwt_secret, algorithm="HS256")


@pytest.fixture
def client(monkeypatch, tmp_path):
    """每个测试独立 SQLite,并重置 A2A 全局状态。"""
    db_path = tmp_path / "a2a.db"
    engine = create_engine(f"sqlite+pysqlite:///{db_path}")
    SessionLocal = sessionmaker(bind=engine)
    Base.metadata.create_all(engine)

    monkeypatch.setattr(app_db, "SessionLocal", SessionLocal)
    monkeypatch.setattr(a2a_server, "SessionLocal", SessionLocal)
    monkeypatch.setattr(a2a_tasks, "SessionLocal", SessionLocal)
    monkeypatch.setattr(backtest_jobs, "SessionLocal", SessionLocal)
    monkeypatch.setattr(tasks_mod, "SessionLocal", SessionLocal)

    # 清空内存中的短任务与限速窗口,避免测试间污染
    a2a_server.short_task_store._short.clear()
    a2a_server.short_task_store._long_meta.clear()
    a2a_tasks.rate_limiter._windows.clear()

    async def _noop():
        pass

    app_main._a2a_handler.aclose = _noop
    with TestClient(app) as c:
        yield c


def _send(
    client: TestClient,
    token: str | None,
    skill: str,
    payload: dict,
    message_id: str = "m1",
    context_id: str = "c1",
) -> dict:
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    resp = client.post(
        "/a2a",
        json={
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": message_id,
                    "role": "user",
                    "parts": [
                        {"type": "data", "data": {"skill": skill, "payload": payload}}
                    ],
                },
                "contextId": context_id,
            },
        },
        headers=headers,
    )
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def _send_text(
    client: TestClient,
    token: str,
    text: str,
    message_id: str = "m1",
) -> dict:
    resp = client.post(
        "/a2a",
        json={
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": message_id,
                    "role": "user",
                    "parts": [{"type": "text", "text": text}],
                },
                "contextId": "c1",
            },
        },
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 200
    return resp.json()["result"]


def _state(result: dict) -> str:
    return result["status"]["state"]


def _fail_text(result: dict) -> str:
    message = result["status"].get("message", {})
    if "text" in message:
        return message["text"]
    for part in message.get("parts", []):
        if part.get("kind") == "text" or part.get("type") == "text":
            return part.get("text", "")
    return ""


def _artifact_data(result: dict, name: str = "result") -> dict:
    for art in result.get("artifacts", []):
        if art.get("name") == name:
            for part in art.get("parts", []):
                data = part.get("data") or part.get("text")
                if isinstance(data, dict):
                    return data
    return {}


def _first_text_part(result: dict) -> str:
    for part in result.get("parts", []):
        if part.get("kind") == "text" or part.get("type") == "text":
            return part.get("text", "")
    return ""


def test_agent_card_is_public(client):
    resp = client.get("/.well-known/agent-card.json")
    assert resp.status_code == 200
    card = resp.json()
    assert card["name"] == "quant-research"
    assert card["protocolVersion"] == "0.3"
    assert card["capabilities"]["streaming"] is True
    assert card["capabilities"]["pushNotifications"] is False
    assert len(card["skills"]) == 19


def test_missing_jwt_returns_failed_task(client):
    result = _send(client, None, "catalog.get", {})
    assert _state(result) == "failed"
    assert "未登录" in _fail_text(result)


def test_bad_jwt_returns_failed_task(client):
    result = _send(client, "bad-token", "catalog.get", {})
    assert _state(result) == "failed"


def test_text_only_request_is_rejected_with_skill_list(client):
    result = _send_text(client, _token(CLIENT_CLAIMS), "hello")
    assert _state(result) == "failed"
    text = _fail_text(result)
    assert "仅 text 的请求被拒绝" in text
    assert "catalog.get" in text


def test_unknown_skill_lists_all_skill_ids(client):
    result = _send(client, _token(CLIENT_CLAIMS), "not.a.skill", {})
    assert _state(result) == "failed"
    text = _fail_text(result)
    assert "未知 skill" in text
    assert "catalog.get" in text


def test_catalog_get_success(client):
    result = _send(client, _token(CLIENT_CLAIMS), "catalog.get", {})
    assert _state(result) == "completed"
    data = _artifact_data(result, name="catalog")
    assert "filter_fields" in data["catalog"]
    assert "version" in data["catalog"]


def test_unauthorized_client_is_rejected(client):
    result = _send(client, _token(NOBODY_CLAIMS), "catalog.get", {})
    assert _state(result) == "failed"
    assert "没有量化研究系统访问权限" in _fail_text(result)


def test_strategy_validate_returns_result(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "strategy.validate",
        {
            "spec": {
                "kind": "single",
                "entry": {
                    "condition": {
                        "op": "gt",
                        "left": {"op": "field", "name": "close"},
                        "right": {"op": "field", "name": "open"},
                    }
                },
                "exit": {"condition": {"op": "literal", "value": False}},
                "holding": {"max_positions": 1},
            }
        },
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, name="validation_result")
    assert "valid" in data["validation_result"]
    assert "capability" in data["validation_result"]


from app.strategy.presets import get_preset_spec  # noqa: E402
from app.strategy.spec import strategy_spec_hash  # noqa: E402


VALID_SPEC = get_preset_spec("breakout").model_dump(mode="json")


def test_strategy_save_draft_creates_disabled_unverified(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "strategy.save_draft",
        {"name": "测试草稿", "spec": VALID_SPEC},
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, name="strategy_draft")
    draft = data["strategy_draft"]
    assert draft["name"] == "测试草稿"
    assert draft["enabled"] is False
    assert draft["research_status"] == "unverified"
    assert "strategy_id" in draft


def test_experiment_create_requires_strategy_id(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "experiment.create",
        {"title": "t", "hypothesis": "h", "permanent_candidate_id": "C1"},
    )
    assert _state(result) == "failed"
    assert "必须提供 strategy_id" in _fail_text(result)


def test_backtest_run_requires_strategy_id(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "backtest.run",
        {
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    assert "必须提供 strategy_id" in _fail_text(result)


def test_backtest_run_rejects_historical_fields(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "backtest.run",
        {
            "strategy_id": 1,
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
            "initial_cash": 100000,
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    text = _fail_text(result)
    assert "initial_cash" in text
    assert "不支持" in text


def test_backtest_run_requires_confirmation(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "backtest.run",
        {
            "strategy_id": 1,
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
        },
    )
    assert _state(result) == "failed"
    assert "confirmed=true" in _fail_text(result)


def test_experiment_trial_batch_rejects_more_than_eight(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "experiment.trial_batch",
        {
            "experiment_id": 1,
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
            "param_patches": [{} for _ in range(9)],
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    assert "最多 8 个" in _fail_text(result)


def test_factor_preview_clamps_codes_and_reports_truncated(client):
    with app_db.SessionLocal() as db:
        db.add(
            Stock(
                code="sh.600519",
                name="茅台",
                list_date=date(2015, 1, 1),
                is_st=False,
            )
        )
        d = date(2024, 1, 1)
        for i in range(100):
            db.add(
                DailyBar(
                    code="sh.600519",
                    date=d + timedelta(days=i),
                    open=10,
                    high=11,
                    low=9,
                    close=10 + i * 0.01,
                    raw_close=10 + i * 0.01,
                    volume=1000,
                    amount=10000,
                    is_st=False,
                )
            )
        db.commit()

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.preview",
        {
            "expression": {
                "op": "divide",
                "left": {"op": "field", "name": "close"},
                "right": {"op": "field", "name": "open"},
            },
            "codes": ["sh.600519"] + [f"sh.6005{i:02d}" for i in range(1, 10)],
            "days": 30,
        },
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, name="factor_preview")
    preview = data["factor_preview"]
    assert len(preview["items"]) == 5
    assert preview["truncated_codes"] is True
    assert preview["note"] == "spot_check_only_not_market_efficacy"


def test_factor_save_draft_requires_admin(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "factor.save_draft",
        {
            "key": "test_factor",
            "name": "测试因子",
            "expression": {"op": "field", "name": "close"},
        },
    )
    assert _state(result) == "failed"
    assert "仅管理员可用" in _fail_text(result)


def test_factor_save_draft_rejects_enabled_true(client):
    result = _send(
        client,
        _token(ADMIN_CLAIMS),
        "factor.save_draft",
        {
            "key": "test_factor_enabled",
            "name": "测试因子",
            "expression": {"op": "field", "name": "close"},
            "enabled": True,
        },
    )
    assert _state(result) == "failed"
    assert "enabled:true" in _fail_text(result)


def test_report_finding_dedups_and_limits_batch(client):
    # 超过 20 条应直接拒绝
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "system.report_finding",
        {
            "session_ref": "sess-1",
            "findings": [{"kind": "product_gap", "detail": f"d{i}"} for i in range(21)],
        },
    )
    assert _state(result) == "failed"
    assert "不能超过 20 条" in _fail_text(result)

    # 第一批写入 2 条
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "system.report_finding",
        {
            "session_ref": "sess-1",
            "findings": [
                {"kind": "product_gap", "detail": "same"},
                {"kind": "ux_friction", "detail": "other"},
            ],
        },
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, name="report_finding")
    assert data["report_finding"]["inserted"] == 2
    assert data["report_finding"]["skipped"] == 0

    # 同日同 session_ref+kind+detail 重复应跳过
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "system.report_finding",
        {
            "session_ref": "sess-1",
            "findings": [
                {"kind": "product_gap", "detail": "same"},
                {"kind": "missing_engine", "detail": "new"},
            ],
        },
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, name="report_finding")
    assert data["report_finding"]["inserted"] == 1
    assert data["report_finding"]["skipped"] == 1


def test_gap_summary_global_requires_admin(client):
    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "system.gap_summary",
        {"scope": "global"},
    )
    assert _state(result) == "failed"
    assert "scope=global 仅管理员可用" in _fail_text(result)


def test_daily_quota_blocks_high_cost_skill(client):
    from datetime import date as dt_date
    from app.models import A2aAudit

    today = dt_date.today()
    with app_db.SessionLocal() as db:
        # 预写 50 条高成本 audit,把配额用光
        for i in range(settings.a2a_daily_quota):
            db.add(
                A2aAudit(
                    user_id=CLIENT_CLAIMS["sub"],
                    a2a_task_id=f"pre-{i}",
                    skill="backtest.run",
                    source="test",
                    created_at=today,
                )
            )
        db.commit()

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "backtest.run",
        {
            "strategy_id": 1,
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
            "confirmed": True,
        },
    )
    assert _state(result) == "failed"
    text = _fail_text(result)
    assert "配额不足" in text
    assert "剩余 0" in text


def test_rate_limit_blocks_excessive_reads(client):
    # 触发 60 次读/create 类请求,第 61 次应被限流
    token = _token(CLIENT_CLAIMS)
    for i in range(settings.a2a_read_rate_limit):
        result = _send(client, token, "catalog.get", {}, message_id=f"r{i}")
        assert _state(result) == "completed", _fail_text(result)

    result = _send(client, token, "catalog.get", {}, message_id="blocked")
    assert _state(result) == "failed"
    assert "过快" in _fail_text(result)


def test_backtest_list_artifact_contract(client):
    """§8.8:artifact 名必须为 backtest_list,含 items/has_more(回归:曾错名为 items)。"""
    result = _send(client, _token(CLIENT_CLAIMS), "backtest.list", {"limit": 5})
    assert _state(result) == "completed"
    data = _artifact_data(result, name="backtest_list")
    assert "backtest_list" in data
    assert "items" in data["backtest_list"]
    assert "has_more" in data["backtest_list"]


def test_experiment_get_returns_registry_fields(client):
    """§8.11:experiment.get 返回 trials + multiplicity + pending_promotions(只读)。"""
    token = _token(CLIENT_CLAIMS)
    r1 = _send(client, token, "strategy.save_draft", {"name": "get链路", "spec": VALID_SPEC})
    sid = int(_artifact_data(r1, "strategy_draft")["strategy_draft"]["strategy_id"])
    r2 = _send(
        client,
        token,
        "experiment.create",
        {
            "title": "t",
            "hypothesis": "h",
            "permanent_candidate_id": "GET1",
            "strategy_id": sid,
        },
    )
    eid = int(_artifact_data(r2, "experiment")["experiment"]["id"])
    r3 = _send(client, token, "experiment.get", {"experiment_id": eid})
    assert _state(r3) == "completed"
    data = _artifact_data(r3, "experiment")
    assert "trials" in data
    assert "multiplicity" in data
    assert "pending_promotions" in data


def test_validate_failure_writes_audit_gap_columns(client):
    """§12/#17:validate 失败写 failure_kind / missing_capability,可聚合。"""
    from app.models import A2aAudit

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "strategy.validate",
        {"spec": {"bogus": "x"}},
    )
    assert _state(result) == "completed"  # validate 本身是只读短任务,valid=false 走 artifact
    with app_db.SessionLocal() as db:
        rows = (
            db.query(A2aAudit)
            .filter(
                A2aAudit.user_id == CLIENT_CLAIMS["sub"],
                A2aAudit.skill == "strategy.validate",
                A2aAudit.failure_kind.isnot(None),
            )
            .all()
        )
    assert rows, "validate 失败未写审计缺口列"
    assert any(r.missing_capability for r in rows)


def test_gap_summary_scope_me_no_cross_table_leak(client):
    """回归:scope=me 时 findings 查询不得跨表引用 A2aAudit(笛卡尔积)。"""
    from datetime import date as dt_date
    from app.models import A2aAudit, ResearchFinding

    today = dt_date.today()
    with app_db.SessionLocal() as db:
        db.add(
            A2aAudit(
                user_id=CLIENT_CLAIMS["sub"],
                a2a_task_id="gap-a1",
                skill="strategy.validate",
                source="test",
                failure_kind="missing_engine",
                missing_capability="rolling_foo",
                created_at=today,
            )
        )
        for i in range(3):
            db.add(
                ResearchFinding(
                    user_id=CLIENT_CLAIMS["sub"],
                    kind="product_gap",
                    detail=f"缺口{i}",
                    source="test",
                    created_at=today,
                )
            )
        db.commit()

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "system.gap_summary",
        {"scope": "me", "since_days": 1},
    )
    assert _state(result) == "completed"
    gs = _artifact_data(result, "gap_summary")["gap_summary"]
    audit_total = sum(i["count"] for i in gs["audit_items"])
    finding_total = sum(i["count"] for i in gs["finding_items"])
    # 若发生笛卡尔积,1 条 audit × 3 条 finding 会被放大
    assert audit_total == 1
    assert finding_total == 3
    assert gs["merged"], "双源合并排行不能为空"


def test_high_cost_idempotent_replay(client):
    """§4.4/#14:同 client_request_id 当日重发,直接回放首个任务结果,不重复计配额。"""
    from datetime import datetime

    from app.models import A2aAudit, Task as QuantTask

    with app_db.SessionLocal() as db:
        db.add(
            QuantTask(
                user_id=CLIENT_CLAIMS["sub"],
                type="experiment_trial",
                title="t",
                status="done",
                params={"client_request_id": "cr-replay-1"},
                result={
                    "trial_result": {
                        "experiment_id": 1,
                        "trial": {"id": 7, "trial_index": 1, "outcome": "ok"},
                        "promotion": {"eligible": False},
                        "detail_ref": {"experiment_id": 1, "run_id": 2},
                    }
                },
                created_at=datetime.now(),
            )
        )
        db.commit()

    result = _send(
        client,
        _token(CLIENT_CLAIMS),
        "experiment.trial",
        {
            "experiment_id": 1,
            "start": "2024-01-02",
            "end": "2024-06-28",
            "codes": ["sh.600519"],
            "confirmed": True,
            "client_request_id": "cr-replay-1",
        },
    )
    assert _state(result) == "completed"
    data = _artifact_data(result, "trial_result")
    assert data["trial_result"]["trial"]["id"] == 7

    # 幂等回放不写高成本审计 → 不重复计配额
    with app_db.SessionLocal() as db:
        n = (
            db.query(A2aAudit)
            .filter(
                A2aAudit.user_id == CLIENT_CLAIMS["sub"],
                A2aAudit.skill == "experiment.trial",
            )
            .count()
        )
    assert n == 0
