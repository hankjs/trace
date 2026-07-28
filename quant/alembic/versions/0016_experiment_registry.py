"""实验注册表:冻结规格的试验族与完整 trial 账本。

Revision ID: 0016_experiment_registry
Revises: 0015_backtest_job_status
Create Date: 2026-07-28

与 quant_research_plan(当日信号解释)分表:
- experiment 保存永久候选 ID、冻结规格、假设、验证方案;
- trial 记录每一次具体回测(含失败/无交易),不可物理删除。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0016_experiment_registry"
down_revision: Union[str, None] = "0015_backtest_job_status"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_experiment",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("owner_id", sa.String(36), nullable=False),
        sa.Column("permanent_candidate_id", sa.String(64), nullable=False),
        sa.Column("family_id", sa.String(64), nullable=True),
        sa.Column("title", sa.String(128), nullable=False),
        sa.Column("hypothesis", sa.Text(), nullable=False),
        sa.Column("strategy_id", sa.Integer(), nullable=True),
        sa.Column("frozen_spec_snapshot", sa.JSON(), nullable=False),
        sa.Column("frozen_spec_hash", sa.String(64), nullable=False),
        sa.Column("identity_hash", sa.String(64), nullable=False),
        sa.Column("validation_snapshot", sa.JSON(), nullable=True),
        sa.Column("universe_snapshot", sa.JSON(), nullable=True),
        sa.Column("cost_snapshot", sa.JSON(), nullable=True),
        sa.Column("status", sa.String(16), nullable=False, server_default="design"),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("updated_at", sa.DateTime(), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "owner_id", "permanent_candidate_id",
            name="uq_experiment_owner_candidate",
        ),
        sa.ForeignKeyConstraint(
            ["strategy_id"], ["quant_strategy.id"],
            name="fk_experiment_strategy_id",
            ondelete="SET NULL",
        ),
    )
    op.create_index(
        "ix_quant_experiment_owner_id", "quant_experiment", ["owner_id"],
    )
    op.create_index(
        "ix_quant_experiment_identity_hash", "quant_experiment", ["identity_hash"],
    )
    op.create_index(
        "ix_quant_experiment_status", "quant_experiment", ["status"],
    )
    op.create_index(
        "ix_quant_experiment_strategy_id", "quant_experiment", ["strategy_id"],
    )

    op.create_table(
        "quant_experiment_trial",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("experiment_id", sa.Integer(), nullable=False),
        sa.Column("trial_index", sa.Integer(), nullable=False),
        sa.Column("param_patch", sa.JSON(), nullable=True),
        sa.Column("backtest_run_id", sa.Integer(), nullable=True),
        sa.Column("outcome", sa.String(16), nullable=False, server_default="error"),
        sa.Column("metrics_summary", sa.JSON(), nullable=True),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("data_fingerprint", sa.String(64), nullable=True),
        sa.Column("universe_fingerprint", sa.String(64), nullable=True),
        sa.Column("cost_fingerprint", sa.String(64), nullable=True),
        sa.Column("execution_fingerprint", sa.String(64), nullable=True),
        sa.Column("oos_revealed_at", sa.DateTime(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "experiment_id", "trial_index", name="uq_experiment_trial_index",
        ),
        sa.ForeignKeyConstraint(
            ["experiment_id"], ["quant_experiment.id"],
            name="fk_experiment_trial_experiment",
            ondelete="RESTRICT",
        ),
        sa.ForeignKeyConstraint(
            ["backtest_run_id"], ["quant_backtest_run.id"],
            name="fk_experiment_trial_backtest",
            ondelete="RESTRICT",
        ),
    )
    op.create_index(
        "ix_quant_experiment_trial_experiment_id",
        "quant_experiment_trial", ["experiment_id"],
    )
    op.create_index(
        "ix_quant_experiment_trial_backtest_run_id",
        "quant_experiment_trial", ["backtest_run_id"],
    )
    op.create_index(
        "ix_quant_experiment_trial_outcome",
        "quant_experiment_trial", ["outcome"],
    )


def downgrade() -> None:
    op.drop_table("quant_experiment_trial")
    op.drop_table("quant_experiment")
