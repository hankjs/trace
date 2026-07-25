"""P0: user_id BIGINT -> VARCHAR(36)

共享 users.id 是 VARCHAR(36) UUID(Rust 侧 crates/hank-db/src/lib.rs:505 建表),
而量化表的 user_id 是 BIGINT,`auth.py` 的 int(sub) 对 UUID 直接抛 ValueError,
导致自选股/持仓/结构化选股/回测保存读取全线 401(REVIEW 第一节)。

数据影响:生产库 quant_trade / quant_backtest_run 各 3 条为 user_id IS NULL,
BIGINT->VARCHAR 对 NULL 无损;quant_watchlist 为空表。
非空的旧数字 user_id(若有)会被 MySQL 隐式转成等值字符串('42'),
与 auth.py 里 `str(sub)` 的取值口径一致。

Revision ID: 0002_user_id_uuid
Revises: 0001_baseline
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0002_user_id_uuid"
down_revision: str | None = "0001_baseline"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # quant_watchlist.user_id 是复合主键的一部分,改类型需重建 PK ——
    # batch 模式在 sqlite 上走「建新表→拷数据→改名」,MySQL 上是原生 MODIFY。
    with op.batch_alter_table("quant_watchlist") as batch:
        batch.alter_column(
            "user_id",
            existing_type=sa.BigInteger(),
            type_=sa.String(36),
            existing_nullable=False,
        )

    for table in ("quant_trade", "quant_backtest_run"):
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "user_id",
                existing_type=sa.BigInteger(),
                type_=sa.String(36),
                existing_nullable=True,
            )


def downgrade() -> None:
    for table in ("quant_backtest_run", "quant_trade"):
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "user_id",
                existing_type=sa.String(36),
                type_=sa.BigInteger(),
                existing_nullable=True,
            )
    with op.batch_alter_table("quant_watchlist") as batch:
        batch.alter_column(
            "user_id",
            existing_type=sa.String(36),
            type_=sa.BigInteger(),
            existing_nullable=False,
        )
