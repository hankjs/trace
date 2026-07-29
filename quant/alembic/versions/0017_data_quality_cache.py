"""数据质量旁路缓存表:只存聚合 JSON,不触碰源数据。

Revision ID: 0017_data_quality_cache
Revises: 0016_experiment_registry
Create Date: 2026-07-28

看板 /admin 的 data-quality 接口原先每次现算 ST/估值/财务/复权覆盖,
在千万级日线上冷路径约 2~3s。本表固定一行缓存完整报告;源表仍只读。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0017_data_quality_cache"
down_revision: Union[str, None] = "0016_experiment_registry"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_data_quality_cache",
        sa.Column("scope", sa.String(32), nullable=False),
        sa.Column("as_of", sa.Date(), nullable=False),
        sa.Column("payload", sa.JSON(), nullable=False),
        sa.Column("computed_at", sa.DateTime(), nullable=False),
        sa.PrimaryKeyConstraint("scope"),
    )


def downgrade() -> None:
    op.drop_table("quant_data_quality_cache")
