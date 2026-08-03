"""因子定义归属与谱系:owner_id + parent_factor_key

owner_id NOT NULL + 哨兵而非可空:与 quant_pool / quant_strategy 的归属模型
保持一致(见 0011),可见性统一为 is_system OR owner_id 是我,避免每处查询
重复 NULL 判断。存量行按 is_system 分流回填:系统因子归哨兵,自定义因子
(历史上只有 admin 能建)也归哨兵 —— 无法追溯真实创建者,归系统等价于
「仅 admin 可改」,与放开前的行为一致,不会误把他人草稿判给某个用户。

Revision ID: 0027_factor_def_owner
Revises: 0026_factor_eval_neutralize
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0027_factor_def_owner"
down_revision: str | None = "0026_factor_eval_neutralize"
branch_labels = None
depends_on = None

SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"


def upgrade() -> None:
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.add_column(sa.Column("owner_id", sa.String(36), nullable=True))
        batch.add_column(sa.Column("parent_factor_key", sa.String(64),
                                   nullable=True))
    op.execute(sa.text(
        "UPDATE quant_factor_def SET owner_id = :sys"
    ).bindparams(sys=SYSTEM_OWNER_ID))
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.alter_column("owner_id", existing_type=sa.String(36),
                           nullable=False)
        batch.create_index("ix_quant_factor_def_owner_id", ["owner_id"])


def downgrade() -> None:
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.drop_index("ix_quant_factor_def_owner_id")
        batch.drop_column("parent_factor_key")
        batch.drop_column("owner_id")
