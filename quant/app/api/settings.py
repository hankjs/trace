"""账户偏好:当前用户可读可改,与共享 users 表无关。"""
from __future__ import annotations

from fastapi import APIRouter, Depends
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..db import get_db
from .. import user_settings as settings_svc

router = APIRouter(prefix="/api/settings", tags=["settings"])


class SettingsOut(BaseModel):
    user_id: str
    can_trade_bse: bool = Field(
        description="当前账户是否已开通北交所交易权限(仅账户标记,不改行情入库)",
    )
    updated_at: str | None = None


class SettingsPatch(BaseModel):
    can_trade_bse: bool | None = Field(
        default=None,
        description="是否开通北交所交易权限;省略则不改",
    )


@router.get("", response_model=SettingsOut)
def get_my_settings(
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    return settings_svc.get_settings(db, user_id_from_claims(claims))


@router.patch("", response_model=SettingsOut)
def patch_my_settings(
    body: SettingsPatch,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    return settings_svc.update_settings(
        db,
        user_id_from_claims(claims),
        can_trade_bse=body.can_trade_bse,
    )
