"""W3 A2A domain 新增表:审计、findings、因子评估

Revision ID: 0025_a2a_tables
Revises: 0024_dynamic_factors
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0025_a2a_tables"
down_revision: Union[str, None] = "0024_dynamic_factors"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_BIG_PK = sa.BigInteger().with_variant(sa.Integer, "sqlite")


def upgrade() -> None:
    op.create_table(
        "quant_a2a_audit",
        sa.Column("id", _BIG_PK, primary_key=True, autoincrement=True, nullable=False),
        sa.Column("user_id", sa.String(36), nullable=False, index=True),
        sa.Column("a2a_task_id", sa.String(64), nullable=False, index=True),
        sa.Column("skill", sa.String(64), nullable=False, index=True),
        sa.Column("source", sa.String(32), nullable=False),
        sa.Column("run_id", sa.Integer(), nullable=True),
        sa.Column("experiment_id", sa.Integer(), nullable=True),
        sa.Column("trial_id", sa.Integer(), nullable=True),
        sa.Column("failure_kind", sa.String(32), nullable=True, index=True),
        sa.Column("missing_capability", sa.String(128), nullable=True, index=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
        sa.Index("ix_a2a_audit_user_created", "user_id", "created_at"),
    )

    op.create_table(
        "quant_research_finding",
        sa.Column("id", _BIG_PK, primary_key=True, autoincrement=True, nullable=False),
        sa.Column("user_id", sa.String(36), nullable=False, index=True),
        sa.Column("kind", sa.String(32), nullable=False, index=True),
        sa.Column("detail", sa.String(512), nullable=False),
        sa.Column("evidence", sa.Text(), nullable=True),
        sa.Column("suggested_system_work", sa.Text(), nullable=True),
        sa.Column("experiment_id", sa.Integer(), nullable=True),
        sa.Column("run_id", sa.Integer(), nullable=True),
        sa.Column("session_ref", sa.String(64), nullable=True),
        sa.Column("source", sa.String(32), nullable=False),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
    )

    op.create_table(
        "quant_factor_evaluation",
        sa.Column("id", _BIG_PK, primary_key=True, autoincrement=True, nullable=False),
        sa.Column("user_id", sa.String(36), nullable=False, index=True),
        sa.Column("factor_key", sa.String(64), nullable=True, index=True),
        sa.Column("expression", sa.JSON(), nullable=True),
        sa.Column("expression_hash", sa.String(64), nullable=True),
        sa.Column("start", sa.Date(), nullable=False),
        sa.Column("end", sa.Date(), nullable=False),
        sa.Column("pool_id", sa.Integer(), nullable=True),
        sa.Column("codes", sa.JSON(), nullable=True),
        sa.Column("layers", sa.Integer(), nullable=False),
        sa.Column("rebalance", sa.String(16), nullable=False),
        sa.Column("universe", sa.JSON(), nullable=False),
        sa.Column("result", sa.JSON(), nullable=True),
        sa.Column("status", sa.String(16), nullable=False, index=True, server_default="done"),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
        sa.Column("finished_at", sa.DateTime(), nullable=True),
    )


def downgrade() -> None:
    op.drop_table("quant_factor_evaluation")
    op.drop_table("quant_research_finding")
    op.drop_table("quant_a2a_audit")
