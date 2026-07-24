"""登录接口:与 server 共用 users 表。"""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy.orm import Session

from ..auth import authenticate, create_token, require_user
from ..db import get_db

router = APIRouter(prefix="/api/auth", tags=["auth"])


class LoginIn(BaseModel):
    username: str
    password: str


@router.post("/login")
def login(body: LoginIn, db: Session = Depends(get_db)):
    user = authenticate(db, body.username.strip(), body.password)
    if user is None:
        raise HTTPException(401, "用户名或密码错误")
    return {"token": create_token(user), "username": user["username"]}


@router.get("/me")
def me(claims: dict = Depends(require_user)):
    return {"username": claims["username"], "can_admin": claims["can_admin"],
            "can_client": claims["can_client"]}
