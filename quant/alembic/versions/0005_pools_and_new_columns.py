"""新表(池/日历)、新列、预置池数据、删冗余索引

brief §3.5 + logs/notify-migrate.md 的 Coordinator 补充清单 + scope-gap 3.3/3.4/3.6。
pool_id 两列取 brief 与 scope-gap 的并集(见 logs/decisions-migrate.md D5);
预置池 seed 四条(D6)。

Revision ID: 0005_pools_and_new_columns
Revises: 0004_daily_bar_natural_pk
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0005_pools_and_new_columns"
down_revision: str | None = "0004_daily_bar_natural_pk"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "quant_pool",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("kind", sa.String(16), nullable=False),   # index / all / static
        sa.Column("ref", sa.String(32), nullable=True),     # 如 hs300_zz500
        sa.Column("user_id", sa.String(36), nullable=True),  # NULL = 系统级共享池
        sa.Column("name", sa.String(64), nullable=False),
        sa.Column("min_list_days", sa.Integer(), nullable=False),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.UniqueConstraint("user_id", "name", name="uq_pool_user_name"),
    )
    op.create_index("ix_quant_pool_kind", "quant_pool", ["kind"])
    op.create_index("ix_quant_pool_user_id", "quant_pool", ["user_id"])

    op.create_table(
        "quant_pool_member",
        sa.Column("pool_id", sa.Integer(), primary_key=True, nullable=False),
        sa.Column("code", sa.String(16), primary_key=True, nullable=False),
        sa.ForeignKeyConstraint(["pool_id"], ["quant_pool.id"]),
    )

    # 交易日历:采集逻辑在 app/data/trade_calendar.py
    op.create_table(
        "quant_trade_calendar",
        sa.Column("date", sa.Date(), primary_key=True, nullable=False),
        sa.Column("is_open", sa.Boolean(), nullable=False),
        sa.Column("source", sa.String(16), nullable=False,
                  server_default="baostock"),
    )

    # quant_stock:全A point-in-time 过滤依据(退市股标记而非删除)
    with op.batch_alter_table("quant_stock") as batch:
        batch.add_column(sa.Column("list_date", sa.Date(), nullable=True))
        batch.add_column(sa.Column("delist_date", sa.Date(), nullable=True))
        batch.add_column(
            sa.Column("is_st", sa.Boolean(), nullable=False,
                      server_default=sa.false()))

    # 回测可复现:固化当时费率快照 + 所用股票池
    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.add_column(sa.Column("costs", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("pool_id", sa.Integer(), nullable=True))

    # 排行榜混批:evaluate.py 原靠 run_at 精确相等分批,改用显式 batch_id
    with op.batch_alter_table("quant_strategy_eval") as batch:
        batch.add_column(sa.Column("batch_id", sa.String(36), nullable=True))
        batch.add_column(sa.Column("pool_id", sa.Integer(), nullable=True))
    op.create_index(
        "ix_quant_strategy_eval_batch_id", "quant_strategy_eval", ["batch_id"])

    # 冗余索引:与各自唯一键前缀完全重复(scope-gap 3.6)
    op.drop_index("ix_quant_factor_daily_code", table_name="quant_factor_daily")
    op.drop_index("ix_quant_pick_date", table_name="quant_pick")

    # 全市场日频最终会超 21 亿行,Integer 主键会溢出(REVIEW 五)。
    # 目标类型用 variant:MySQL 上 BIGINT,sqlite 上仍是 INTEGER
    # (sqlite 只对 "INTEGER PRIMARY KEY" 自增,写 BIGINT 会破坏插入)。
    for table in ("quant_valuation_snapshot", "quant_fundamental_snapshot"):
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "id",
                existing_type=sa.Integer(),
                type_=sa.BigInteger().with_variant(sa.Integer, "sqlite"),
                existing_nullable=False,
                existing_autoincrement=True,
                autoincrement=True,
            )

    _seed_system_pools()


def _seed_system_pools() -> None:
    """系统级预置池(user_id IS NULL)。id 固定 1~4,供代码硬编码引用。

    kind='index' 的 min_list_days=0:指数成分本身已含上市时长的隐含约束,
    再叠加 60 天会与历史回测口径打架(见 decisions D6)。
    hs300 / zz500 单指数池用来承接 scope-gap 2.1 里要收口的
    screener.py:326-345 与 fundamentals.py:491-497 两处单指数分支。
    """
    pool = sa.table(
        "quant_pool",
        sa.column("id", sa.Integer),
        sa.column("kind", sa.String),
        sa.column("ref", sa.String),
        sa.column("user_id", sa.String),
        sa.column("name", sa.String),
        sa.column("min_list_days", sa.Integer),
        sa.column("created_at", sa.DateTime),
    )
    now = sa.func.now()
    op.execute(pool.insert().values([
        {"id": 1, "kind": "index", "ref": "hs300_zz500", "user_id": None,
         "name": "沪深300+中证500", "min_list_days": 0, "created_at": now},
        {"id": 2, "kind": "all", "ref": None, "user_id": None,
         "name": "全部A股", "min_list_days": 60, "created_at": now},
        {"id": 3, "kind": "index", "ref": "hs300", "user_id": None,
         "name": "沪深300", "min_list_days": 0, "created_at": now},
        {"id": 4, "kind": "index", "ref": "zz500", "user_id": None,
         "name": "中证500", "min_list_days": 0, "created_at": now},
    ]))


def downgrade() -> None:
    for table in ("quant_fundamental_snapshot", "quant_valuation_snapshot"):
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "id",
                existing_type=sa.BigInteger().with_variant(sa.Integer, "sqlite"),
                type_=sa.Integer(),
                existing_nullable=False,
                existing_autoincrement=True,
                autoincrement=True,
            )

    op.create_index("ix_quant_pick_date", "quant_pick", ["date"])
    op.create_index("ix_quant_factor_daily_code", "quant_factor_daily", ["code"])

    op.drop_index(
        "ix_quant_strategy_eval_batch_id", table_name="quant_strategy_eval")
    with op.batch_alter_table("quant_strategy_eval") as batch:
        batch.drop_column("pool_id")
        batch.drop_column("batch_id")

    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.drop_column("pool_id")
        batch.drop_column("costs")

    with op.batch_alter_table("quant_stock") as batch:
        batch.drop_column("is_st")
        batch.drop_column("delist_date")
        batch.drop_column("list_date")

    op.drop_table("quant_trade_calendar")
    op.drop_table("quant_pool_member")
    op.drop_index("ix_quant_pool_user_id", table_name="quant_pool")
    op.drop_index("ix_quant_pool_kind", table_name="quant_pool")
    op.drop_table("quant_pool")
