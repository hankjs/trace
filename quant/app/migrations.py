"""启动时的 Alembic 版本校验。

替代原来的 `Base.metadata.create_all(engine)` + `schema.py` 手写 ALTER:
启动不再改动 schema,只检查库的 alembic 版本是否为最新,不一致就明确告警。

刻意**不在启动时自动 `upgrade head`**:多副本滚动部署会并发 DDL,
且千万行表的 ALTER 是长事务(见 logs/migration-plan.md),必须由人类在维护窗口执行。
"""
from __future__ import annotations

import logging
from pathlib import Path

from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from sqlalchemy import Engine

logger = logging.getLogger(__name__)

QUANT_DIR = Path(__file__).resolve().parent.parent
ALEMBIC_INI = QUANT_DIR / "alembic.ini"


def expected_heads() -> set[str]:
    config = Config(str(ALEMBIC_INI))
    config.set_main_option("script_location", str(QUANT_DIR / "alembic"))
    return set(ScriptDirectory.from_config(config).get_heads())


def current_heads(engine: Engine) -> set[str]:
    with engine.connect() as connection:
        return set(MigrationContext.configure(connection).get_current_heads())


def check_schema_version(engine: Engine) -> bool:
    """库版本是否为最新。不一致只告警不抛错,便于运维先看日志再决定。"""
    expected = expected_heads()
    try:
        current = current_heads(engine)
    except Exception:  # noqa: BLE001 - 连不上库的报错留给后续请求暴露
        logger.exception("无法读取数据库 alembic 版本")
        return False

    if not current:
        logger.error(
            "数据库无 alembic 版本记录。空库请执行 `alembic upgrade head`;"
            "改造前就存在的库请先 `alembic stamp 0001_baseline` 再 upgrade head。"
        )
        return False
    if current != expected:
        logger.error(
            "数据库 schema 版本 %s 与代码期望 %s 不一致,请执行 `alembic upgrade head`",
            sorted(current), sorted(expected),
        )
        return False

    logger.info("数据库 schema 版本已是最新: %s", sorted(current))
    return True
