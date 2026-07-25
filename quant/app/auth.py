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
    # bcrypt 5.0 对超 72 字节密码抛 ValueError(不再静默截断);Rust 侧
    # bcrypt::verify(...).unwrap_or(false) 等效于校验失败。这里同样吞成失败,
    # 保证两端一致地返回 401 而非本进程 500。
    try:
        ok = bcrypt.checkpw(password.encode(), row["password_hash"].encode())
    except ValueError:
        return None
    if not ok:
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
        "sub": str(user["id"]),
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
        # 共享 users.id 是 VARCHAR(36) UUID,Rust 侧 Claims.sub 也是 String
        # (crates/hank-db/src/lib.rs),故 sub 始终是字符串 —— 用 PyJWT 的
        # 默认严格校验即可,不需要 verify_sub=False 放宽。
        claims = jwt.decode(token, settings.jwt_secret, algorithms=["HS256"])
    except jwt.PyJWTError:
        raise HTTPException(401, "登录已过期,请重新登录")
    return claims


def require_admin(request: Request) -> dict:
    """FastAPI 依赖：仅允许 JWT 中明确具有管理员权限的用户。"""
    claims = require_user(request)
    if claims.get("can_admin") not in (True, 1):
        raise HTTPException(403, "需要管理员权限")
    return claims


def require_client(request: Request) -> dict:
    """业务接口要求客户端或管理员权限。"""
    claims = require_user(request)
    if claims.get("can_client") not in (True, 1) and claims.get("can_admin") not in (True, 1):
        raise HTTPException(403, "没有量化研究系统访问权限")
    return claims


def user_id_from_claims(claims: dict) -> str:
    """提取共享 users.id(VARCHAR(36) UUID 字符串),拒绝缺失或空 subject 的 token。"""
    sub = claims.get("sub")
    if sub is None:
        raise HTTPException(401, "登录凭证缺少有效用户标识")
    user_id = str(sub).strip()
    if not user_id:
        raise HTTPException(401, "登录凭证缺少有效用户标识")
    return user_id
