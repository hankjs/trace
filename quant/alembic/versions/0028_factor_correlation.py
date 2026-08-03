"""因子相关性与正交性检验结果表 quant_factor_correlation

与 quant_factor_evaluation 分表:单因子有效性 vs 相对增量,核心判据不同,
塞一张表会让 result JSON 形状分叉。

Revision ID: 0028_factor_correlation
Revises: 0027_factor_def_owner
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0028_factor_correlation"
down_revision: Union[str, None] = "0027_factor_def_owner"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_BIG_PK = sa.BigInteger().with_variant(sa.Integer, "sqlite")


def upgrade() -> None:
    op.create_table(
        "quant_factor_correlation",
        sa.Column("id", _BIG_PK, primary_key=True, autoincrement=True, nullable=False),
        sa.Column("user_id", sa.String(36), nullable=False, index=True),
        sa.Column("factor_key", sa.String(64), nullable=True, index=True),
        sa.Column("expression", sa.JSON(), nullable=True),
        sa.Column("expression_hash", sa.String(64), nullable=True),
        sa.Column("benchmark_keys", sa.JSON(), nullable=False),
        sa.Column("start", sa.Date(), nullable=False),
        sa.Column("end", sa.Date(), nullable=False),
        sa.Column("pool_id", sa.Integer(), nullable=True),
        sa.Column("codes", sa.JSON(), nullable=True),
        sa.Column("rebalance", sa.String(16), nullable=False),
        sa.Column("neutralize", sa.JSON(), nullable=True),
        sa.Column("universe", sa.JSON(), nullable=False),
        sa.Column("result", sa.JSON(), nullable=True),
        sa.Column(
            "status", sa.String(16), nullable=False, index=True, server_default="done",
        ),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column(
            "created_at", sa.DateTime(), nullable=False, server_default=sa.func.now(),
        ),
        sa.Column("finished_at", sa.DateTime(), nullable=True),
    )


def downgrade() -> None:
    op.drop_table("quant_factor_correlation")
