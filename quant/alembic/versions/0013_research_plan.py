"""新增版本化策略研究计划及组合逐股变化快照。

研究计划是策略信号的可审计解释，不是订单。每次重算都新建不可变快照，信号
仅指向当前版本；历史版本通过 supersedes_plan_id 串联。策略实例允许被删除，
因此计划保存 strategy_id 的历史值和完整策略快照，而不对 strategy_id 建外键。

Revision ID: 0013_research_plan
Revises: 0012_strategy_table
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0013_research_plan"
down_revision: str | None = "0012_strategy_table"
branch_labels = None
depends_on = None

PRICE = sa.Numeric(12, 4, asdecimal=False)
WEIGHT = sa.Numeric(12, 8, asdecimal=False)


def big_pk() -> sa.types.TypeEngine:
    return sa.BigInteger().with_variant(sa.Integer, "sqlite")


def upgrade() -> None:
    op.create_table(
        "quant_research_plan",
        sa.Column("id", big_pk(), primary_key=True, autoincrement=True,
                  nullable=False),
        sa.Column("owner_id", sa.String(36), nullable=False),
        sa.Column("strategy_is_system", sa.Boolean(), nullable=False,
                  server_default=sa.false()),
        sa.Column("strategy_id", sa.Integer(), nullable=False),
        sa.Column("strategy_name", sa.String(64), nullable=False),
        sa.Column("template", sa.String(32), nullable=False),
        sa.Column("strategy_kind", sa.String(16), nullable=False),
        sa.Column("strategy_version", sa.String(64), nullable=False),
        sa.Column("params_snapshot", sa.JSON(), nullable=False),
        sa.Column("plan_type", sa.String(32), nullable=False),
        sa.Column("code", sa.String(16), nullable=True),
        sa.Column("pool_id", sa.Integer(), nullable=True),
        sa.Column("data_date", sa.Date(), nullable=False),
        sa.Column("generated_at", sa.DateTime(), nullable=False),
        sa.Column("next_execution_date", sa.Date(), nullable=True),
        sa.Column("valid_until", sa.Date(), nullable=True),
        sa.Column("signal_type", sa.String(32), nullable=False),
        sa.Column("status", sa.String(32), nullable=False),
        sa.Column("status_reason", sa.JSON(), nullable=False),
        sa.Column("price_adjustment", sa.String(16), nullable=False,
                  server_default="forward"),
        sa.Column("signal_price", PRICE, nullable=True),
        sa.Column("entry_observation", sa.JSON(), nullable=False),
        sa.Column("risk_rules", sa.JSON(), nullable=False),
        sa.Column("take_profit", sa.JSON(), nullable=False),
        sa.Column("native_exit", sa.JSON(), nullable=False),
        sa.Column("exit_hits", sa.JSON(), nullable=False),
        sa.Column("portfolio_summary", sa.JSON(), nullable=True),
        sa.Column("backtest_run_id", sa.Integer(), nullable=True),
        sa.Column("backtest_evidence", sa.JSON(), nullable=False),
        sa.Column("product_boundary", sa.Text(), nullable=False),
        sa.Column("revision", sa.Integer(), nullable=False,
                  server_default="1"),
        sa.Column("supersedes_plan_id", big_pk(), nullable=True),
        sa.ForeignKeyConstraint(
            ["backtest_run_id"], ["quant_backtest_run.id"],
            name="fk_research_plan_backtest_run", ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(
            ["supersedes_plan_id"], ["quant_research_plan.id"],
            name="fk_research_plan_supersedes", ondelete="SET NULL"),
    )
    for column in (
        "owner_id", "strategy_is_system", "strategy_id", "template", "plan_type",
        "code", "data_date", "generated_at", "status", "backtest_run_id",
        "supersedes_plan_id",
    ):
        op.create_index(
            f"ix_quant_research_plan_{column}", "quant_research_plan", [column])

    op.create_table(
        "quant_research_plan_item",
        sa.Column("id", big_pk(), primary_key=True, autoincrement=True,
                  nullable=False),
        sa.Column("plan_id", big_pk(), nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("previous_weight", WEIGHT, nullable=False,
                  server_default="0"),
        sa.Column("target_weight", WEIGHT, nullable=False,
                  server_default="0"),
        sa.Column("change_type", sa.String(16), nullable=False),
        sa.Column("score", sa.Float(), nullable=True),
        sa.Column("score_details", sa.JSON(), nullable=True),
        sa.Column("rank", sa.Integer(), nullable=True),
        sa.Column("eligible", sa.Boolean(), nullable=False,
                  server_default=sa.true()),
        sa.Column("reasons", sa.JSON(), nullable=False),
        sa.Column("risk_snapshot", sa.JSON(), nullable=True),
        sa.ForeignKeyConstraint(
            ["plan_id"], ["quant_research_plan.id"],
            name="fk_research_plan_item_plan", ondelete="CASCADE"),
        sa.UniqueConstraint("plan_id", "code", name="uq_research_plan_item"),
    )
    for column in ("plan_id", "code", "change_type"):
        op.create_index(
            f"ix_quant_research_plan_item_{column}",
            "quant_research_plan_item", [column])

    with op.batch_alter_table("quant_signal") as batch:
        batch.add_column(sa.Column("plan_id", big_pk(), nullable=True))
        batch.create_index("ix_quant_signal_plan_id", ["plan_id"])
        batch.create_foreign_key(
            "fk_quant_signal_plan_id", "quant_research_plan",
            ["plan_id"], ["id"], ondelete="SET NULL")


def downgrade() -> None:
    with op.batch_alter_table("quant_signal") as batch:
        batch.drop_constraint("fk_quant_signal_plan_id", type_="foreignkey")
        batch.drop_index("ix_quant_signal_plan_id")
        batch.drop_column("plan_id")

    for column in ("change_type", "code", "plan_id"):
        op.drop_index(
            f"ix_quant_research_plan_item_{column}",
            table_name="quant_research_plan_item")
    op.drop_table("quant_research_plan_item")

    for column in (
        "supersedes_plan_id", "backtest_run_id", "status", "generated_at",
        "data_date", "code", "plan_type", "template", "strategy_id",
        "strategy_is_system", "owner_id",
    ):
        op.drop_index(
            f"ix_quant_research_plan_{column}", table_name="quant_research_plan")
    op.drop_table("quant_research_plan")
