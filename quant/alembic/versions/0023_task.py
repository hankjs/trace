"""用户异步任务表 quant_task。

Revision ID: 0023_task
Revises: 0022_job_run
Create Date: 2026-07-30

全局异步任务系统(见 app/tasks.py):回测/参数扫描/成本敏感性等耗时操作
统一"提交即返回 202、后台线程执行、任务中心查看"。每用户同时只允许一个
pending/running 任务(提交时校验,冲突 409)。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0023_task"
down_revision: Union[str, None] = "0022_job_run"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_task",
        sa.Column(
            "id",
            sa.BigInteger().with_variant(sa.Integer(), "sqlite"),
            autoincrement=True, nullable=False,
        ),
        sa.Column("user_id", sa.String(36), nullable=False),
        sa.Column("type", sa.String(32), nullable=False),
        sa.Column("status", sa.String(16), nullable=False),
        sa.Column("title", sa.String(255), nullable=False),
        sa.Column("params", sa.JSON(), nullable=True),
        sa.Column("ref_id", sa.Integer(), nullable=True),
        sa.Column("result", sa.JSON(), nullable=True),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("started_at", sa.DateTime(), nullable=True),
        sa.Column("finished_at", sa.DateTime(), nullable=True),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_quant_task_user_id", "quant_task", ["user_id"])
    op.create_index("ix_quant_task_status", "quant_task", ["status"])
    op.create_index("ix_task_user_status", "quant_task", ["user_id", "status"])


def downgrade() -> None:
    op.drop_index("ix_task_user_status", table_name="quant_task")
    op.drop_index("ix_quant_task_status", table_name="quant_task")
    op.drop_index("ix_quant_task_user_id", table_name="quant_task")
    op.drop_table("quant_task")
