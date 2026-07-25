"""quant_daily_bar 换 (code, date) 自然主键

代理 BigInteger 自增主键使行按插入顺序聚簇而非 (code,date),每次区间扫描都是
二级索引 + 随机回表;ix_quant_daily_bar_code 与唯一键前缀完全冗余,在千万行表上
纯粹拖慢写入(REVIEW 第五节)。

顺序依赖(已满足):`screener.py:410` 的 `func.count(DailyBar.id)` 已由 agent-pool
改成 `func.count()`,见 logs/notify-migrate.md。全仓已无 DailyBar.id 引用。

删除的索引/约束:
- 代理列 id(及其主键)
- uq_daily_bar_code_date —— 换自然主键后与 PK 完全重复
- ix_quant_daily_bar_code —— 与新 PK (code,date) 前缀冗余
保留 ix_quant_daily_bar_date:跨股票按单日查询(选股/因子)需要。

**生产库本 revision 是长事务**:748 万行(全市场后约 1300 万行)整表重建,
耗时与锁表影响见 logs/migration-plan.md,须由人类在维护窗口执行。

Revision ID: 0004_daily_bar_natural_pk
Revises: 0003_decimal_prices
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0004_daily_bar_natural_pk"
down_revision: str | None = "0003_decimal_prices"
branch_labels = None
depends_on = None


def upgrade() -> None:
    bind = op.get_bind()
    if bind.dialect.name == "mysql":
        # MySQL 需要一条语句里同时丢弃旧 PK 与自增列,否则
        # 「自增列必须是某个键的第一列」的约束会让中间状态非法。
        op.execute(
            "ALTER TABLE `quant_daily_bar` "
            "DROP PRIMARY KEY, "
            "DROP COLUMN `id`, "
            "DROP INDEX `uq_daily_bar_code_date`, "
            "DROP INDEX `ix_quant_daily_bar_code`, "
            "ADD PRIMARY KEY (`code`, `date`)"
        )
        return

    # sqlite:batch 模式重建表。recreate="always" 确保主键变更真的落到新表上。
    with op.batch_alter_table(
        "quant_daily_bar",
        recreate="always",
        # 重建后的目标形态(sqlite 无法从旧表推断新主键)
        copy_from=sa.Table(
            "quant_daily_bar",
            sa.MetaData(),
            sa.Column("id", sa.BigInteger(), nullable=False),
            sa.Column("code", sa.String(16), nullable=False),
            sa.Column("date", sa.Date(), nullable=False),
            sa.Column("open", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("high", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("low", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("close", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("raw_close", sa.Numeric(12, 4, asdecimal=False), nullable=True),
            sa.Column("volume", sa.Numeric(20, 2, asdecimal=False), nullable=False),
            sa.Column("amount", sa.Numeric(20, 2, asdecimal=False), nullable=False),
            sa.PrimaryKeyConstraint("code", "date"),
            sa.Index("ix_quant_daily_bar_date", "date"),
        ),
    ) as batch:
        batch.drop_column("id")


def downgrade() -> None:
    bind = op.get_bind()
    if bind.dialect.name == "mysql":
        op.execute(
            "ALTER TABLE `quant_daily_bar` "
            "DROP PRIMARY KEY, "
            "ADD COLUMN `id` BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY FIRST, "
            "ADD UNIQUE INDEX `uq_daily_bar_code_date` (`code`, `date`), "
            "ADD INDEX `ix_quant_daily_bar_code` (`code`)"
        )
        return

    with op.batch_alter_table(
        "quant_daily_bar",
        recreate="always",
        copy_from=sa.Table(
            "quant_daily_bar",
            sa.MetaData(),
            sa.Column("code", sa.String(16), nullable=False),
            sa.Column("date", sa.Date(), nullable=False),
            sa.Column("open", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("high", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("low", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("close", sa.Numeric(12, 4, asdecimal=False), nullable=False),
            sa.Column("raw_close", sa.Numeric(12, 4, asdecimal=False), nullable=True),
            sa.Column("volume", sa.Numeric(20, 2, asdecimal=False), nullable=False),
            sa.Column("amount", sa.Numeric(20, 2, asdecimal=False), nullable=False),
            sa.Index("ix_quant_daily_bar_date", "date"),
        ),
    ) as batch:
        batch.add_column(
            sa.Column("id", sa.BigInteger(), nullable=False, primary_key=True))
        batch.create_unique_constraint(
            "uq_daily_bar_code_date", ["code", "date"])
        batch.create_index("ix_quant_daily_bar_code", ["code"])
