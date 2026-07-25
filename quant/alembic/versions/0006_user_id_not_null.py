"""user_id 收紧为 NOT NULL(默认 no-op,需显式开启)

brief §3.1 要求这一步单独一个 revision,便于人类决定何时执行。

**默认什么都不做。** 仅当环境变量 `QUANT_ENFORCE_USER_ID_NOT_NULL=1` 时才执行 DDL。
理由与备选方案对比见 logs/decisions-migrate.md D7:做成第二个 head 会让日常
`alembic upgrade head` 报 multiple heads;放主链无条件执行则人类无从决定时机。

执行前提:先跑 `uv run python scripts/claim_legacy_user_data.py --user-id <UUID>`
认领遗留数据(生产库有 3 条成交 + 3 条回测 user_id IS NULL)。
本 revision 自带前置校验:仍有 NULL 行就报错中止,不会把遗留数据锁成不可见。

    QUANT_ENFORCE_USER_ID_NOT_NULL=1 uv run alembic upgrade head

注意:开启后 models.py 的 Trade.user_id / BacktestRun.user_id 仍声明 nullable,
verify_migration_parity.py 会报不一致 —— 这是预期的,人类收紧生产库时应同步
把 models.py 改成 non-nullable。默认(不设环境变量)路径下两者一致。

Revision ID: 0006_user_id_not_null
Revises: 0005_pools_and_new_columns
"""
from __future__ import annotations

import os

import sqlalchemy as sa
from alembic import op

revision: str = "0006_user_id_not_null"
down_revision: str | None = "0005_pools_and_new_columns"
branch_labels = None
depends_on = None

_ENV_FLAG = "QUANT_ENFORCE_USER_ID_NOT_NULL"
_TABLES = ("quant_trade", "quant_backtest_run")


def _enabled() -> bool:
    return os.environ.get(_ENV_FLAG, "").strip().lower() in {"1", "true", "yes"}


def upgrade() -> None:
    if not _enabled():
        return

    bind = op.get_bind()
    for table in _TABLES:
        orphans = bind.execute(
            sa.text(f"SELECT COUNT(*) FROM {table} WHERE user_id IS NULL")  # noqa: S608
        ).scalar_one()
        if orphans:
            raise RuntimeError(
                f"{table} 仍有 {orphans} 行 user_id IS NULL，无法收紧为 NOT NULL。"
                "请先执行 scripts/claim_legacy_user_data.py --user-id <UUID> 认领，"
                "否则这些行对任何用户都不可见(SQL 里 NULL = ? 恒为 unknown)。"
            )

    for table in _TABLES:
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "user_id", existing_type=sa.String(36), nullable=False)


def downgrade() -> None:
    if not _enabled():
        return
    for table in _TABLES:
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                "user_id", existing_type=sa.String(36), nullable=True)
