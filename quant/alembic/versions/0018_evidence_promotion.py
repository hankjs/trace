"""试验证据推进待办表:达标由系统提名,用户确认才改 evidence_status。

Revision ID: 0018_evidence_promotion
Revises: 0017_data_quality_cache
Create Date: 2026-07-29

试验回测永不自动 advance;质量闸门通过后写 pending 待办。
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0018_evidence_promotion"
down_revision: Union[str, None] = "0017_data_quality_cache"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "quant_evidence_promotion",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("owner_id", sa.String(36), nullable=False),
        sa.Column("strategy_id", sa.Integer(), nullable=False),
        sa.Column("experiment_id", sa.Integer(), nullable=False),
        sa.Column("trial_id", sa.Integer(), nullable=False),
        sa.Column("backtest_run_id", sa.Integer(), nullable=False),
        sa.Column("status", sa.String(16), nullable=False, server_default="pending"),
        sa.Column("suggested_target", sa.String(16), nullable=False),
        sa.Column("quality_checks", sa.JSON(), nullable=True),
        sa.Column("metrics_summary", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("resolved_at", sa.DateTime(), nullable=True),
        sa.ForeignKeyConstraint(
            ["strategy_id"], ["quant_strategy.id"], ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["experiment_id"], ["quant_experiment.id"], ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["trial_id"], ["quant_experiment_trial.id"], ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["backtest_run_id"], ["quant_backtest_run.id"], ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("trial_id", name="uq_evidence_promotion_trial"),
    )
    op.create_index(
        "ix_quant_evidence_promotion_owner_id",
        "quant_evidence_promotion", ["owner_id"],
    )
    op.create_index(
        "ix_quant_evidence_promotion_strategy_id",
        "quant_evidence_promotion", ["strategy_id"],
    )
    op.create_index(
        "ix_quant_evidence_promotion_experiment_id",
        "quant_evidence_promotion", ["experiment_id"],
    )
    op.create_index(
        "ix_quant_evidence_promotion_status",
        "quant_evidence_promotion", ["status"],
    )


def downgrade() -> None:
    op.drop_index(
        "ix_quant_evidence_promotion_status",
        table_name="quant_evidence_promotion",
    )
    op.drop_index(
        "ix_quant_evidence_promotion_experiment_id",
        table_name="quant_evidence_promotion",
    )
    op.drop_index(
        "ix_quant_evidence_promotion_strategy_id",
        table_name="quant_evidence_promotion",
    )
    op.drop_index(
        "ix_quant_evidence_promotion_owner_id",
        table_name="quant_evidence_promotion",
    )
    op.drop_table("quant_evidence_promotion")
