"""回测作业状态机:pending/running/done/failed。

Revision ID: 0015_backtest_job_status
Revises: 0014_dynamic_strategy_spec
Create Date: 2026-07-28

开发阶段默认异步回测:先落 pending 行(冻结 request_snapshot 与 spec),
后台执行后 update 为 done/failed,避免长请求占满 worker。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0015_backtest_job_status"
down_revision: Union[str, None] = "0014_dynamic_strategy_spec"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.add_column(sa.Column(
            "status", sa.String(16), nullable=False, server_default="done",
        ))
        batch.add_column(sa.Column("error", sa.Text(), nullable=True))
        batch.add_column(sa.Column("request_snapshot", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("started_at", sa.DateTime(), nullable=True))
        batch.add_column(sa.Column("finished_at", sa.DateTime(), nullable=True))
    op.create_index(
        "ix_quant_backtest_run_status", "quant_backtest_run", ["status"],
    )


def downgrade() -> None:
    op.drop_index("ix_quant_backtest_run_status", table_name="quant_backtest_run")
    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.drop_column("finished_at")
        batch.drop_column("started_at")
        batch.drop_column("request_snapshot")
        batch.drop_column("error")
        batch.drop_column("status")
