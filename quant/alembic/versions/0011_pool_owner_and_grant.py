"""池可见性:user_id NULL 改为 owner_id NOT NULL + is_system + 授权关联表

## 为什么改

`quant_pool.user_id IS NULL` 表示「系统级预置池」,这一个决定引出三个问题:

**1. 唯一约束失效。** `UniqueConstraint("user_id", "name")` 对预置池完全不起
作用 —— MySQL(与 SQLite)里 NULL 互不相等,实测可以插入 3 条
`(user_id=NULL, name='全部A股')` 而不报错,同时用户池的同名会被正确拦住。
唯一性保护恰好在最需要它的地方失灵:预置池是所有用户共用的,重复了影响面最大。

**2. 每个查询都要重复 NULL 判断。** 可见性写成
`(user_id IS NULL) OR (user_id = :uid)`,散落在 5 处(`api/pools.py` 的
98/112/118/250/251 行),漏一次就是越权读取或漏掉预置池。

**3. 无法表达中间态。** 只有「我的」和「所有人的」,没有「分享给特定用户」。

## 新结构

```
quant_pool(..., owner_id NOT NULL, is_system BOOL)
quant_pool_grant(pool_id, user_id, can_edit)
```

可见性 = `is_system` OR `owner_id` 是我 OR `grant` 里有我的行。

系统池归哨兵 UUID `00000000-0000-0000-0000-000000000000`,不指向 users 表的
真实行 —— 预置池不该因 admin 被删或换人而失去归属,也不该让「属于某人」与
「系统级」混淆。故 `owner_id` 不加 users 外键。

系统池**不在 grant 表插行**,靠 `is_system` 短路:否则每个新用户注册都要批量插
授权行,新增系统池还要回填所有存量用户,漏一步就有人看不到预置池。grant 表
只存真实的分享关系。

Revision ID: 0011_pool_owner_and_grant
Revises: 0010_daily_bar_is_st
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0011_pool_owner_and_grant"
down_revision: str | None = "0010_daily_bar_is_st"
branch_labels = None
depends_on = None

SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"


def upgrade() -> None:
    with op.batch_alter_table("quant_pool") as batch:
        batch.add_column(sa.Column("owner_id", sa.String(36), nullable=True))
        batch.add_column(sa.Column(
            "is_system", sa.Boolean(), nullable=False, server_default="0"))

    # 迁移既有数据:user_id IS NULL 的是系统池,其余保留原属主
    op.execute(sa.text(
        "UPDATE quant_pool SET owner_id = :sys, is_system = 1 "
        "WHERE user_id IS NULL"
    ).bindparams(sys=SYSTEM_OWNER_ID))
    op.execute("UPDATE quant_pool SET owner_id = user_id WHERE user_id IS NOT NULL")

    with op.batch_alter_table("quant_pool") as batch:
        batch.alter_column("owner_id", existing_type=sa.String(36),
                           nullable=False)
        batch.create_index("ix_quant_pool_owner_id", ["owner_id"])
        batch.create_index("ix_quant_pool_is_system", ["is_system"])
        batch.create_unique_constraint("uq_pool_owner_name", ["owner_id", "name"])
        # 旧约束与旧列一并去掉
        batch.drop_constraint("uq_pool_user_name", type_="unique")
        batch.drop_index("ix_quant_pool_user_id")
        batch.drop_column("user_id")

    op.create_table(
        "quant_pool_grant",
        sa.Column("pool_id", sa.Integer(), primary_key=True, nullable=False),
        sa.Column("user_id", sa.String(36), primary_key=True, nullable=False),
        sa.Column("can_edit", sa.Boolean(), nullable=False, server_default="0"),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.ForeignKeyConstraint(["pool_id"], ["quant_pool.id"],
                                ondelete="CASCADE"),
    )
    op.create_index("ix_quant_pool_grant_user_id", "quant_pool_grant", ["user_id"])


def downgrade() -> None:
    op.drop_index("ix_quant_pool_grant_user_id", table_name="quant_pool_grant")
    op.drop_table("quant_pool_grant")

    with op.batch_alter_table("quant_pool") as batch:
        batch.add_column(sa.Column("user_id", sa.String(36), nullable=True))
    op.execute(sa.text(
        "UPDATE quant_pool SET user_id = owner_id WHERE is_system = 0"))

    with op.batch_alter_table("quant_pool") as batch:
        batch.create_index("ix_quant_pool_user_id", ["user_id"])
        batch.create_unique_constraint("uq_pool_user_name", ["user_id", "name"])
        batch.drop_constraint("uq_pool_owner_name", type_="unique")
        batch.drop_index("ix_quant_pool_is_system")
        batch.drop_index("ix_quant_pool_owner_id")
        batch.drop_column("is_system")
        batch.drop_column("owner_id")
