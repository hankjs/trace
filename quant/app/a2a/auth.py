"""A2A 入口鉴权：Bearer JWT 校验 + user context。

除 Agent Card 外全部 JSON-RPC 入口共用本 ServerCallContextBuilder。
"""
from __future__ import annotations

import jwt
from fastapi import HTTPException
from starlette.requests import Request

from a2a.auth.user import UnauthenticatedUser, User
from a2a.server.routes.common import DefaultServerCallContextBuilder

from ..config import settings


class AuthenticatedUser(User):
    """把 quant 的 JWT claims 适配到 A2A User 接口。"""

    def __init__(self, claims: dict):
        self._claims = claims

    @property
    def is_authenticated(self) -> bool:
        return True

    @property
    def user_name(self) -> str:
        return str(self._claims.get("username", ""))

    @property
    def claims(self) -> dict:
        return self._claims


class QuantA2AContextBuilder(DefaultServerCallContextBuilder):
    """从 Authorization: Bearer <jwt> 解析 Trace 用户 claims。

    失败时不抛异常，而是返回未认证 user；由 executor 在 Task 层面拒绝，
    避免 SDK dispatcher 把异常转成内部错误。
    """

    def build_user(self, request: Request) -> User:
        auth = request.headers.get("Authorization", "")
        if not auth.startswith("Bearer "):
            return UnauthenticatedUser()
        token = auth[len("Bearer "):]
        try:
            claims = jwt.decode(token, settings.jwt_secret, algorithms=["HS256"])
        except jwt.PyJWTError:
            return UnauthenticatedUser()
        return AuthenticatedUser(claims)

    def build(self, request: Request):
        ctx = super().build(request)
        user = ctx.user
        if isinstance(user, AuthenticatedUser):
            ctx.state["claims"] = user.claims
        return ctx


def require_claims(context) -> dict:
    """从 ServerCallContext 取 claims；若缺失说明鉴权未通过。"""
    claims = context.state.get("claims")
    if not claims:
        raise HTTPException(401, "未登录")
    return claims


def can_client(claims: dict) -> bool:
    return bool(claims.get("can_client") or claims.get("can_admin"))


def can_admin(claims: dict) -> bool:
    return bool(claims.get("can_admin"))


def user_id_from_claims(claims: dict) -> str:
    sub = claims.get("sub")
    if not sub:
        raise HTTPException(401, "登录凭证缺少有效用户标识")
    return str(sub)


__all__ = [
    "AuthenticatedUser",
    "QuantA2AContextBuilder",
    "can_admin",
    "can_client",
    "require_claims",
    "user_id_from_claims",
]
