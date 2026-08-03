"""因子评估口径列:中性化维度与前瞻期列表

两列可空,存量行读出为 NULL(等价于「未中性化 + 未记录 horizons」),
不需要回填。

Revision ID: 0026_factor_eval_neutralize
Revises: 0025_a2a_tables
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0026_factor_eval_neutralize"
down_revision: Union[str, None] = "0025_a2a_tables"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "quant_factor_evaluation",
        sa.Column("neutralize", sa.JSON(), nullable=True),
    )
    op.add_column(
        "quant_factor_evaluation",
        sa.Column("horizons", sa.JSON(), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("quant_factor_evaluation", "horizons")
    op.drop_column("quant_factor_evaluation", "neutralize")
