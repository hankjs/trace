"""JWT subject compatibility tests."""
from __future__ import annotations

import sys
from pathlib import Path

import jwt
import pytest
from fastapi import HTTPException
from starlette.requests import Request

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.auth import create_token, require_admin, require_user
from app.config import settings


def _request(token: str) -> Request:
    return Request({
        "type": "http",
        "headers": [(b"authorization", f"Bearer {token}".encode())],
    })


def test_new_token_uses_standard_string_subject():
    token = create_token({
        "id": 42,
        "username": "tester",
        "can_admin": True,
        "can_client": True,
    })
    claims = jwt.decode(token, settings.jwt_secret, algorithms=["HS256"])
    assert claims["sub"] == "42"
    assert require_user(_request(token))["username"] == "tester"


def test_require_user_accepts_legacy_numeric_subject():
    token = jwt.encode(
        {
            "sub": 42,
            "username": "legacy",
            "can_admin": True,
            "can_client": True,
        },
        settings.jwt_secret,
        algorithm="HS256",
    )
    assert require_user(_request(token))["sub"] == 42


def test_require_admin_rejects_regular_user():
    token = create_token({
        "id": 7,
        "username": "regular",
        "can_admin": False,
        "can_client": True,
    })
    with pytest.raises(HTTPException) as exc_info:
        require_admin(_request(token))
    assert exc_info.value.status_code == 403


def test_require_admin_accepts_admin_user():
    token = create_token({
        "id": 8,
        "username": "admin",
        "can_admin": True,
        "can_client": True,
    })
    assert require_admin(_request(token))["username"] == "admin"
