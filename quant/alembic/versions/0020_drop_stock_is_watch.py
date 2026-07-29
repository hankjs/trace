"""删除 quant_stock.is_watch 遗留列。

Revision ID: 0020_drop_stock_is_watch
Revises: 0019_user_settings
Create Date: 2026-07-29

早期单用户时代把「是否自选」挂在股票主表上;多用户后自选改为
quant_watchlist(user_id, code)。API 响应里的 is_watch 由 watchlist join
计算,不再读写本列。删列避免继续被误用为真源。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0020_drop_stock_is_watch"
down_revision: Union[str, None] = "0019_user_settings"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.drop_column("quant_stock", "is_watch")


def downgrade() -> None:
    op.add_column(
        "quant_stock",
        sa.Column(
            "is_watch", sa.Boolean(), nullable=False, server_default=sa.false(),
        ),
    )
    # server_default 只服务迁移;业务默认由应用层负责,去掉 DB 默认以免残留语义
    op.alter_column("quant_stock", "is_watch", server_default=None)
