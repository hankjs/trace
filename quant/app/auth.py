"""鉴权:与 server 共用 MySQL users 表(bcrypt 哈希)和 JWT 密钥(HS256)。

users 表由 Rust server 拥有和维护,这里只用裸 SQL 只读查询,
不进 Base.metadata,避免 create_all 接管该表。
token claims 与 server/src/auth.rs 完全一致,两端 token 互通。
"""
from __future__ import annotations

import time

import bcrypt
import jwt
from fastapi import HTTPException, Request
from sqlalchemy import text
from sqlalchemy.orm import Session

from .config import settings

TOKEN_TTL_SECONDS = 30 * 24 * 3600  # 与 server 一致:30 天


def authenticate(db: Session, username: str, password: str) -> dict | None:
    """校验用户名密码,成功返回用户 dict,失败返回 None。"""
    row = db.execute(
        text(
            "SELECT id, username, password_hash, can_login_admin, can_login_client"
            " FROM users WHERE username = :username"
        ),
        {"username": username},
    ).mappings().first()
    if row is None:
        return None
    if not bcrypt.checkpw(password.encode(), row["password_hash"].encode()):
        return None
    return {
        "id": row["id"],
        "username": row["username"],
        "can_admin": bool(row["can_login_admin"]),
        "can_client": bool(row["can_login_client"]),
    }


def create_token(user: dict) -> str:
    """签发与 server 兼容的 HS256 JWT。"""
    claims = {
        "sub": user["id"],
        "username": user["username"],
        "can_admin": user["can_admin"],
        "can_client": user["can_client"],
        "exp": int(time.time()) + TOKEN_TTL_SECONDS,
    }
    return jwt.encode(claims, settings.jwt_secret, algorithm="HS256")


def require_user(request: Request) -> dict:
    """FastAPI 依赖:校验 Authorization: Bearer <token>,失败 401。"""
    auth = request.headers.get("Authorization", "")
    if not auth.startswith("Bearer "):
        raise HTTPException(401, "未登录")
    token = auth[len("Bearer "):]
    try:
        claims = jwt.decode(token, settings.jwt_secret, algorithms=["HS256"])
    except jwt.PyJWTError:
        raise HTTPException(401, "登录已过期,请重新登录")
    return claims
