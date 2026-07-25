"""收紧 user_id / batch_id 为 NOT NULL

## 为什么现在能做

`0006_user_id_not_null` 设计成需环境变量显式开启的 no-op,理由是「人类无从决定
时机」—— 当时生产库有 3 条成交 + 3 条回测 `user_id IS NULL`,贸然收紧会把它们
锁成不可见。

现在时机明确:系统尚未实际运营。库中那批「历史数据」经核实全是开发期测试垃圾
(`quant_trade` 的备注是「验证买入1/2」「验证卖出」,回测与评估都是 2026-07-24
开发当天跑的),已备份至 `/tmp/quant-backup/dev-data-2026-07-25.json` 后清空。
`users` 表只有 1 行(admin)。

没有运营数据要兼容,schema 就该表达真实约束,而不是为不存在的历史留可空口子。
同时删掉了两处为「历史数据」写的降级分支:
- `evaluate.leaderboard` 的 `batch_id IS NULL` 退回按 `run_at` 取单行
- `auth.require_user` 的 `verify_sub=False`(Rust 侧 `Claims.sub` 本就是 String)

## 为什么 pool_id 不收紧

`pool_id` 有正当的空值语义:显式传 `codes` 的回测不属于任何股票池
(`api/backtest.py` 的 `pool.id if use_pool else None`)。强制 NOT NULL 会逼调用方
填一个假的池 id,让「这次回测用的哪个池」这个字段失去意义。

Revision ID: 0009_tighten_not_null
Revises: 0008_adjust_factor_source
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0009_tighten_not_null"
down_revision: str | None = "0008_adjust_factor_source"
branch_labels = None
depends_on = None

# (表, 列, 类型)
_COLUMNS = (
    ("quant_trade", "user_id", sa.String(36)),
    ("quant_backtest_run", "user_id", sa.String(36)),
    ("quant_strategy_eval", "batch_id", sa.String(36)),
)


def upgrade() -> None:
    conn = op.get_bind()
    for table, column, _type in _COLUMNS:
        # 前置校验:仍有 NULL 就报错中止,不静默把数据锁成不可见
        remaining = conn.execute(sa.text(
            f"SELECT COUNT(*) FROM `{table}` WHERE `{column}` IS NULL")).scalar()
        if remaining:
            raise RuntimeError(
                f"{table}.{column} 仍有 {remaining} 行为 NULL,"
                f"收紧为 NOT NULL 会让它们不可见。请先认领或清理这些行。")

    for table, column, type_ in _COLUMNS:
        with op.batch_alter_table(table) as batch:
            batch.alter_column(column, existing_type=type_, nullable=False)


def downgrade() -> None:
    for table, column, type_ in _COLUMNS:
        with op.batch_alter_table(table) as batch:
            batch.alter_column(column, existing_type=type_, nullable=True)
