"""JWT subject compatibility tests."""
from __future__ import annotations

import sys
from pathlib import Path

import jwt
import pytest
from fastapi import HTTPException
from starlette.requests import Request

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.auth import (
    authenticate,
    create_token,
    require_admin,
    require_client,
    require_user,
    user_id_from_claims,
)
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


def test_require_client_rejects_user_without_product_permission():
    token = create_token({
        "id": 9,
        "username": "disabled",
        "can_admin": False,
        "can_client": False,
    })
    with pytest.raises(HTTPException) as exc_info:
        require_client(_request(token))
    assert exc_info.value.status_code == 403


def test_user_id_accepts_uuid_subject():
    """P0 回归:共享 users.id 是 36 位 UUID,原 int(sub) 对其抛 ValueError → 全线 401。"""
    uuid = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"
    assert user_id_from_claims({"sub": uuid}) == uuid


def test_require_user_extracts_uuid_from_real_token():
    """端到端:UUID 用户签发的 token 能通过校验并取回原样 UUID。"""
    uuid = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"
    token = create_token({
        "id": uuid,
        "username": "uuid-user",
        "can_admin": False,
        "can_client": True,
    })
    claims = require_client(_request(token))
    assert claims["sub"] == uuid
    assert user_id_from_claims(claims) == uuid


def test_user_id_accepts_string_and_legacy_numeric_subjects():
    """返回值统一是 str:数字 sub 转成等值字符串,与库里 VARCHAR(36) 口径一致。"""
    assert user_id_from_claims({"sub": "42"}) == "42"
    assert user_id_from_claims({"sub": 42}) == "42"


@pytest.mark.parametrize("claims", [{}, {"sub": None}, {"sub": ""}, {"sub": "   "}])
def test_user_id_rejects_missing_or_blank_subject(claims):
    with pytest.raises(HTTPException) as exc_info:
        user_id_from_claims(claims)
    assert exc_info.value.status_code == 401


# --- bcrypt 超长密码(brief §5 / REVIEW 六.1) ----------------------------

UUID = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"


def _fake_db(password_hash: str):
    """伪造 users 表的单行查询结果(users 表由 Rust server 拥有,这里只读)。"""
    class _Result:
        def mappings(self):
            return self

        def first(self):
            return {
                "id": UUID,
                "username": "victim",
                "password_hash": password_hash,
                "can_login_admin": 0,
                "can_login_client": 1,
            }

    class _Db:
        def execute(self, *_args, **_kwargs):
            return _Result()

    return _Db()


def test_authenticate_returns_none_for_overlong_password():
    """bcrypt 5.0 对超 72 字节密码抛 ValueError(不再静默截断)。

    原实现让未认证请求就能刷出 500 + 堆栈;Rust 侧
    `bcrypt::verify(...).unwrap_or(false)`(server/src/routes.rs:39)当作校验失败,
    两端必须一致地返回 401 —— 即 authenticate 返回 None。
    """
    import bcrypt

    hashed = bcrypt.hashpw(b"correct-horse", bcrypt.gensalt())
    overlong = "x" * 100  # > 72 字节

    # 先确认底层库确实抛错(否则本测试失去意义)
    with pytest.raises(ValueError):
        bcrypt.checkpw(overlong.encode(), hashed)

    assert authenticate(_fake_db(hashed.decode()), "victim", overlong) is None


def test_authenticate_succeeds_for_correct_password():
    """确认吞 ValueError 没把正常登录一起吞掉。"""
    import bcrypt

    hashed = bcrypt.hashpw(b"correct-horse", bcrypt.gensalt())
    user = authenticate(_fake_db(hashed.decode()), "victim", "correct-horse")
    assert user == {
        "id": UUID,
        "username": "victim",
        "can_admin": False,
        "can_client": True,
    }
    # UUID 用户能拿到可用 token,取回的 user_id 就是那个 UUID
    assert user_id_from_claims(require_user(_request(create_token(user)))) == UUID


def test_authenticate_returns_none_for_wrong_password():
    import bcrypt

    hashed = bcrypt.hashpw(b"correct-horse", bcrypt.gensalt())
    assert authenticate(_fake_db(hashed.decode()), "victim", "wrong-horse") is None
