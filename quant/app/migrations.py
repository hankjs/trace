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


class SchemaVersionError(RuntimeError):
    """Alembic 版本与代码不一致且 schema_strict=True 时抛出,阻止启动。"""


def check_schema_version(engine: Engine, *, strict: bool | None = None) -> bool:
    """库版本是否为最新。

    strict 默认读 settings.schema_strict:True 时版本不一致抛 SchemaVersionError,
    避免带着错误 schema 跑出不可复现的研究结论;False 时只打日志(临时排障)。
    """
    if strict is None:
        from .config import settings
        strict = settings.schema_strict

    expected = expected_heads()
    try:
        current = current_heads(engine)
    except Exception as exc:  # noqa: BLE001 - 连不上库的报错留给后续请求暴露
        logger.exception("无法读取数据库 alembic 版本")
        if strict:
            raise SchemaVersionError("无法读取数据库 alembic 版本") from exc
        return False

    if not current:
        msg = (
            "数据库无 alembic 版本记录。空库请执行 `alembic upgrade head`;"
            "改造前就存在的库请先 `alembic stamp 0001_baseline` 再 upgrade head。"
        )
        logger.error(msg)
        if strict:
            raise SchemaVersionError(msg)
        return False
    if current != expected:
        msg = (
            f"数据库 schema 版本 {sorted(current)} 与代码期望 "
            f"{sorted(expected)} 不一致,请执行 `alembic upgrade head`"
        )
        logger.error(msg)
        if strict:
            raise SchemaVersionError(msg)
        return False

    logger.info("数据库 schema 版本已是最新: %s", sorted(current))
    return True
