"""baseline: 改造前的既有 schema

对应 master(ec1c508)上 `app/models.py` + `app/schema.py` 的最终形态:
`user_id` 是 BIGINT、`quant_daily_bar` 带代理自增 id、价格金额列是 Float、
估值/财报表用含 available_date 的唯一键(原 schema.py 的手写 ALTER 结果)。

**既有生产库应 `alembic stamp 0001_baseline` 而不执行本 revision**,
随后 `alembic upgrade head` 只跑 0002 起的增量改动。
空库直接 `alembic upgrade head` 会先建出本基线再改到目标态。

Revision ID: 0001_baseline
Revises:
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0001_baseline"
down_revision: str | None = None
branch_labels = None
depends_on = None

# sqlite 只对 "INTEGER PRIMARY KEY" 自增,写 BIGINT 会让插入拿不到自增值。
# MySQL 上仍渲染 BIGINT AUTO_INCREMENT。与 app/models.py 的 _BIG_PK 一致。
_BIG_PK = sa.BigInteger().with_variant(sa.Integer, "sqlite")


def upgrade() -> None:
    op.create_table(
        "quant_stock",
        sa.Column("code", sa.String(16), primary_key=True, nullable=False),
        sa.Column("name", sa.String(64), nullable=False),
        sa.Column("industry", sa.String(64), nullable=False),
        sa.Column("is_watch", sa.Boolean(), nullable=False),
    )

    op.create_table(
        "quant_watchlist",
        sa.Column("user_id", sa.BigInteger(), primary_key=True, nullable=False),
        sa.Column("code", sa.String(16), primary_key=True, nullable=False),
        sa.Column("created_at", sa.DateTime(), nullable=False),
    )

    op.create_table(
        "quant_daily_bar",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("date", sa.Date(), nullable=False),
        sa.Column("open", sa.Float(), nullable=False),
        sa.Column("high", sa.Float(), nullable=False),
        sa.Column("low", sa.Float(), nullable=False),
        sa.Column("close", sa.Float(), nullable=False),
        sa.Column("raw_close", sa.Float(), nullable=True),
        sa.Column("volume", sa.Float(), nullable=False),
        sa.Column("amount", sa.Float(), nullable=False),
        sa.UniqueConstraint("code", "date", name="uq_daily_bar_code_date"),
    )
    op.create_index("ix_quant_daily_bar_code", "quant_daily_bar", ["code"])
    op.create_index("ix_quant_daily_bar_date", "quant_daily_bar", ["date"])

    op.create_table(
        "quant_snapshot",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("ts", sa.DateTime(), nullable=False),
        sa.Column("price", sa.Float(), nullable=False),
        sa.Column("pct_chg", sa.Float(), nullable=True),
        sa.Column("volume", sa.Float(), nullable=True),
        sa.Column("amount", sa.Float(), nullable=True),
    )
    op.create_index("ix_quant_snapshot_code", "quant_snapshot", ["code"])
    op.create_index("ix_quant_snapshot_ts", "quant_snapshot", ["ts"])

    op.create_table(
        "quant_signal",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("date", sa.Date(), nullable=False),
        sa.Column("strategy", sa.String(64), nullable=False),
        sa.Column("side", sa.String(8), nullable=False),
        sa.Column("price", sa.Float(), nullable=True),
        sa.Column("reason", sa.JSON(), nullable=True),
        sa.UniqueConstraint("code", "date", "strategy", "side", name="uq_signal"),
    )
    op.create_index("ix_quant_signal_code", "quant_signal", ["code"])
    op.create_index("ix_quant_signal_date", "quant_signal", ["date"])
    op.create_index("ix_quant_signal_strategy", "quant_signal", ["strategy"])

    op.create_table(
        "quant_trade",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        # user_id 与其索引原由 schema.py `_ensure_owner_columns` 手写 ALTER 补出
        sa.Column("user_id", sa.BigInteger(), nullable=True),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("trade_date", sa.Date(), nullable=False),
        sa.Column("side", sa.String(8), nullable=False),
        sa.Column("price", sa.Float(), nullable=False),
        sa.Column("qty", sa.Float(), nullable=False),
        sa.Column("fee", sa.Float(), nullable=False),
        sa.Column("note", sa.Text(), nullable=False),
    )
    op.create_index("ix_quant_trade_code", "quant_trade", ["code"])
    op.create_index("ix_quant_trade_user_id", "quant_trade", ["user_id"])

    op.create_table(
        "quant_backtest_run",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=True),
        sa.Column("strategy", sa.String(64), nullable=False),
        sa.Column("params", sa.JSON(), nullable=True),
        sa.Column("codes", sa.JSON(), nullable=True),
        sa.Column("start", sa.Date(), nullable=False),
        sa.Column("end", sa.Date(), nullable=False),
        sa.Column("metrics", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
    )
    op.create_index(
        "ix_quant_backtest_run_user_id", "quant_backtest_run", ["user_id"])

    op.create_table(
        "quant_index_member",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("index_name", sa.String(16), nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("in_date", sa.Date(), nullable=False),
        sa.Column("out_date", sa.Date(), nullable=True),
        sa.UniqueConstraint(
            "index_name", "code", "in_date", name="uq_index_member"),
    )
    op.create_index(
        "ix_quant_index_member_index_name", "quant_index_member", ["index_name"])
    op.create_index("ix_quant_index_member_code", "quant_index_member", ["code"])

    op.create_table(
        "quant_factor_daily",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("date", sa.Date(), nullable=False),
        sa.Column("mom20", sa.Float(), nullable=True),
        sa.Column("mom60", sa.Float(), nullable=True),
        sa.Column("rsi14", sa.Float(), nullable=True),
        sa.Column("atr_pct", sa.Float(), nullable=True),
        sa.Column("vol_ratio5", sa.Float(), nullable=True),
        sa.Column("ma20_slope", sa.Float(), nullable=True),
        sa.Column("amount_avg20", sa.Float(), nullable=True),
        sa.UniqueConstraint("code", "date", name="uq_factor_code_date"),
    )
    op.create_index("ix_quant_factor_daily_code", "quant_factor_daily", ["code"])
    op.create_index("ix_quant_factor_daily_date", "quant_factor_daily", ["date"])

    op.create_table(
        "quant_valuation_snapshot",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("data_date", sa.Date(), nullable=False),
        sa.Column("report_period", sa.Date(), nullable=True),
        sa.Column("available_date", sa.Date(), nullable=False),
        sa.Column("source", sa.String(96), nullable=False),
        sa.Column("pe_ttm", sa.Float(), nullable=True),
        sa.Column("pb", sa.Float(), nullable=True),
        sa.Column("ps_ttm", sa.Float(), nullable=True),
        sa.Column("dividend_yield", sa.Float(), nullable=True),
        sa.Column("total_market_cap", sa.Float(), nullable=True),
        # 原 schema.py `_VERSIONED_UNIQUES` 手写 ALTER 的结果
        sa.UniqueConstraint(
            "code", "data_date", "available_date",
            name="uq_valuation_code_date_available"),
    )
    op.create_index(
        "ix_quant_valuation_snapshot_code", "quant_valuation_snapshot", ["code"])
    op.create_index(
        "ix_quant_valuation_snapshot_data_date",
        "quant_valuation_snapshot", ["data_date"])
    op.create_index(
        "ix_quant_valuation_snapshot_available_date",
        "quant_valuation_snapshot", ["available_date"])

    op.create_table(
        "quant_fundamental_snapshot",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("data_date", sa.Date(), nullable=False),
        sa.Column("report_period", sa.Date(), nullable=False),
        sa.Column("available_date", sa.Date(), nullable=False),
        sa.Column("source", sa.String(96), nullable=False),
        sa.Column("roe", sa.Float(), nullable=True),
        sa.Column("revenue_yoy", sa.Float(), nullable=True),
        sa.Column("profit_yoy", sa.Float(), nullable=True),
        sa.Column("gross_margin", sa.Float(), nullable=True),
        sa.Column("net_margin", sa.Float(), nullable=True),
        sa.Column("debt_ratio", sa.Float(), nullable=True),
        sa.Column("cashflow_ratio", sa.Float(), nullable=True),
        sa.UniqueConstraint(
            "code", "report_period", "available_date",
            name="uq_fundamental_code_period_available"),
    )
    op.create_index(
        "ix_quant_fundamental_snapshot_code",
        "quant_fundamental_snapshot", ["code"])
    op.create_index(
        "ix_quant_fundamental_snapshot_data_date",
        "quant_fundamental_snapshot", ["data_date"])
    op.create_index(
        "ix_quant_fundamental_snapshot_report_period",
        "quant_fundamental_snapshot", ["report_period"])
    op.create_index(
        "ix_quant_fundamental_snapshot_available_date",
        "quant_fundamental_snapshot", ["available_date"])

    op.create_table(
        "quant_pick",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("date", sa.Date(), nullable=False),
        sa.Column("code", sa.String(16), nullable=False),
        sa.Column("score", sa.Float(), nullable=False),
        sa.Column("rank", sa.Integer(), nullable=False),
        sa.Column("factors", sa.JSON(), nullable=True),
        sa.UniqueConstraint("date", "code", name="uq_pick_date_code"),
    )
    op.create_index("ix_quant_pick_date", "quant_pick", ["date"])
    op.create_index("ix_quant_pick_code", "quant_pick", ["code"])

    op.create_table(
        "quant_strategy_eval",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("strategy", sa.String(64), nullable=False),
        sa.Column("params", sa.JSON(), nullable=True),
        sa.Column("scope", sa.String(64), nullable=False),
        sa.Column("start", sa.Date(), nullable=False),
        sa.Column("end", sa.Date(), nullable=False),
        sa.Column("metrics", sa.JSON(), nullable=True),
        sa.Column("run_at", sa.DateTime(), nullable=False),
    )
    op.create_index(
        "ix_quant_strategy_eval_strategy", "quant_strategy_eval", ["strategy"])
    op.create_index(
        "ix_quant_strategy_eval_scope", "quant_strategy_eval", ["scope"])
    op.create_index(
        "ix_quant_strategy_eval_run_at", "quant_strategy_eval", ["run_at"])

    op.create_table(
        "quant_backtest_equity",
        sa.Column("id", _BIG_PK, primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("run_id", sa.Integer(), nullable=False),
        sa.Column("date", sa.Date(), nullable=False),
        sa.Column("equity", sa.Float(), nullable=False),
        sa.ForeignKeyConstraint(["run_id"], ["quant_backtest_run.id"]),
        sa.UniqueConstraint("run_id", "date", name="uq_bt_equity_run_date"),
    )
    op.create_index(
        "ix_quant_backtest_equity_run_id", "quant_backtest_equity", ["run_id"])


def downgrade() -> None:
    for table in (
        "quant_backtest_equity",
        "quant_strategy_eval",
        "quant_pick",
        "quant_fundamental_snapshot",
        "quant_valuation_snapshot",
        "quant_factor_daily",
        "quant_index_member",
        "quant_backtest_run",
        "quant_trade",
        "quant_signal",
        "quant_snapshot",
        "quant_daily_bar",
        "quant_watchlist",
        "quant_stock",
    ):
        op.drop_table(table)
