"""定时任务执行日志表 quant_job_run。

Revision ID: 0022_job_run
Revises: 0021_drop_redundant_indexes
Create Date: 2026-07-29

系统调度与 admin 手动触发共用的操作日志(见 app/job_log.py)。
旁路日志表,不参与任何研究/回测链路,可随时 truncate。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0022_job_run"
down_revision: Union[str, None] = "0021_drop_redundant_indexes"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_job_run",
        sa.Column(
            "id",
            sa.BigInteger().with_variant(sa.Integer(), "sqlite"),
            autoincrement=True, nullable=False,
        ),
        sa.Column("job_id", sa.String(64), nullable=False),
        sa.Column("trigger", sa.String(16), nullable=False),
        sa.Column("status", sa.String(16), nullable=False),
        sa.Column("operator", sa.String(64), nullable=True),
        sa.Column("started_at", sa.DateTime(), nullable=False),
        sa.Column("finished_at", sa.DateTime(), nullable=True),
        sa.Column("result", sa.Text(), nullable=True),
        sa.Column("error", sa.Text(), nullable=True),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_quant_job_run_job_id", "quant_job_run", ["job_id"])


def downgrade() -> None:
    op.drop_index("ix_quant_job_run_job_id", table_name="quant_job_run")
    op.drop_table("quant_job_run")
