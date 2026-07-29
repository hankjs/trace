"""账户偏好:无行默认值 + 部分更新落库。"""
from __future__ import annotations

import sys
from pathlib import Path

from sqlalchemy import create_engine
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.api.settings import SettingsPatch, get_my_settings, patch_my_settings
from app.db import Base
from app.models import UserSettings
from app import user_settings as settings_svc

CLAIMS = {"sub": "user-uuid-1", "username": "tester", "can_client": True}


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False}, poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def test_default_without_row():
    with _session() as db:
        out = settings_svc.get_settings(db, "user-uuid-1")
        assert out["can_trade_bse"] is False
        assert out["updated_at"] is None
        assert db.get(UserSettings, "user-uuid-1") is None


def test_patch_creates_row():
    with _session() as db:
        out = settings_svc.update_settings(
            db, "user-uuid-1", can_trade_bse=True,
        )
        assert out["can_trade_bse"] is True
        assert out["updated_at"] is not None
        row = db.get(UserSettings, "user-uuid-1")
        assert row is not None
        assert row.can_trade_bse is True


def test_api_get_and_patch():
    with _session() as db:
        got = get_my_settings(db=db, claims=CLAIMS)
        assert got["can_trade_bse"] is False

        patched = patch_my_settings(
            body=SettingsPatch(can_trade_bse=True),
            db=db,
            claims=CLAIMS,
        )
        assert patched["can_trade_bse"] is True

        got2 = get_my_settings(db=db, claims=CLAIMS)
        assert got2["can_trade_bse"] is True


def test_patch_none_keeps_value():
    with _session() as db:
        settings_svc.update_settings(db, "user-uuid-1", can_trade_bse=True)
        out = settings_svc.update_settings(db, "user-uuid-1", can_trade_bse=None)
        assert out["can_trade_bse"] is True
