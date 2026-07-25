"""Alembic 运行环境。

数据库 URL 解析顺序(**不硬编码任何连接串**):
1. `alembic -x db_url=sqlite+pysqlite:///./t.db upgrade head` —— 测试/校验用
2. `app.config.settings.database_url` —— 正常部署(来自 config.toml,gitignore)

生产库执行迁移由人类在维护窗口手动进行,见 logs/migration-plan.md。
"""
from __future__ import annotations

import sys
from logging.config import fileConfig
from pathlib import Path

from alembic import context
from sqlalchemy import engine_from_config, pool

# 让 `alembic` 命令能 import app 包
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

config = context.config
if config.config_file_name is not None:
    fileConfig(config.config_file_name)


def _resolve_url() -> str:
    from_cli = context.get_x_argument(as_dictionary=True).get("db_url")
    if from_cli:
        return from_cli
    # 延迟 import:仅在没给 -x db_url 时才需要 config.toml 存在
    from app.config import settings

    return settings.database_url


def _target_metadata():
    """按需加载 models 的 metadata,供 autogenerate 比对。"""
    from app.db import Base
    from app import models  # noqa: F401 - 注册所有模型到 Base.metadata

    return Base.metadata


def run_migrations_offline() -> None:
    context.configure(
        url=_resolve_url(),
        target_metadata=_target_metadata(),
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
        compare_type=True,
    )
    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    section = config.get_section(config.config_ini_section, {})
    section["sqlalchemy.url"] = _resolve_url()
    connectable = engine_from_config(
        section, prefix="sqlalchemy.", poolclass=pool.NullPool,
    )
    with connectable.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=_target_metadata(),
            compare_type=True,
            # sqlite 上 ALTER 能力有限,batch 模式自动走「建新表→拷数据→改名」
            render_as_batch=connection.dialect.name == "sqlite",
        )
        with context.begin_transaction():
            context.run_migrations()
    connectable.dispose()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
