"""用户账户偏好读写。无行时返回默认值,首次写入再落库。"""
from __future__ import annotations

from datetime import datetime

from sqlalchemy.orm import Session

from .models import UserSettings

# 与 UserSettings 列默认一致;改这里时同步 models / API 文档
DEFAULTS = {
    "can_trade_bse": False,
}


def get_settings(db: Session, user_id: str) -> dict:
    """读取偏好;无行不插库,直接用默认值。"""
    row = db.get(UserSettings, user_id)
    if row is None:
        return {
            "user_id": user_id,
            **DEFAULTS,
            "updated_at": None,
        }
    return {
        "user_id": user_id,
        "can_trade_bse": bool(row.can_trade_bse),
        "updated_at": row.updated_at.isoformat(timespec="seconds")
        if row.updated_at else None,
    }


def update_settings(
    db: Session,
    user_id: str,
    *,
    can_trade_bse: bool | None = None,
) -> dict:
    """部分更新;仅传入的字段会改。"""
    row = db.get(UserSettings, user_id)
    if row is None:
        row = UserSettings(
            user_id=user_id,
            can_trade_bse=DEFAULTS["can_trade_bse"],
            updated_at=datetime.now(),
        )
        db.add(row)
    if can_trade_bse is not None:
        row.can_trade_bse = bool(can_trade_bse)
    row.updated_at = datetime.now()
    db.commit()
    db.refresh(row)
    return get_settings(db, user_id)
