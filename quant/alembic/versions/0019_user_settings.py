"""用户账户偏好表 quant_user_settings(北交所交易权限等)。

Revision ID: 0019_user_settings
Revises: 0018_evidence_promotion
Create Date: 2026-07-29

与共享 users 表解耦:权限/登录仍在 server.users,研究侧偏好独立存放。
无行 = 全默认(can_trade_bse=false),不预插存量用户。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0019_user_settings"
down_revision: Union[str, None] = "0018_evidence_promotion"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_user_settings",
        sa.Column("user_id", sa.String(36), nullable=False),
        sa.Column(
            "can_trade_bse", sa.Boolean(), nullable=False, server_default=sa.false(),
        ),
        sa.Column("updated_at", sa.DateTime(), nullable=False),
        sa.PrimaryKeyConstraint("user_id"),
    )


def downgrade() -> None:
    op.drop_table("quant_user_settings")
