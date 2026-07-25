"""本模块新增研究表的小范围幂等迁移。"""
from __future__ import annotations

import logging

from sqlalchemy import Engine, inspect

logger = logging.getLogger(__name__)

_VERSIONED_UNIQUES = (
    (
        "quant_valuation_snapshot",
        "uq_valuation_code_date",
        "uq_valuation_code_date_available",
        ("code", "data_date", "available_date"),
    ),
    (
        "quant_fundamental_snapshot",
        "uq_fundamental_code_period",
        "uq_fundamental_code_period_available",
        ("code", "report_period", "available_date"),
    ),
)


def upgrade_research_schema(engine: Engine) -> None:
    """允许估值和财报按 available_date 保留历史修订版本。"""
    _ensure_owner_columns(engine)
    for table, legacy_name, target_name, columns in _VERSIONED_UNIQUES:
        inspector = inspect(engine)
        if not inspector.has_table(table):
            continue
        index_names = {item["name"] for item in inspector.get_indexes(table)}
        unique_names = {
            item["name"] for item in inspector.get_unique_constraints(table)
        }
        existing_names = index_names | unique_names
        if target_name in existing_names:
            continue

        quoted_columns = ", ".join(f"`{column}`" for column in columns)
        if legacy_name in existing_names:
            if engine.dialect.name != "mysql":
                raise RuntimeError(
                    f"{table} 使用旧唯一键 {legacy_name}，"
                    "自动升级当前仅支持项目约定的 MySQL 数据库"
                )
            statement = (
                f"ALTER TABLE `{table}` DROP INDEX `{legacy_name}`, "
                f"ADD UNIQUE INDEX `{target_name}` ({quoted_columns})"
            )
        else:
            statement = (
                f"CREATE UNIQUE INDEX `{target_name}` ON `{table}` "
                f"({quoted_columns})"
            )
        with engine.begin() as connection:
            connection.exec_driver_sql(statement)
        logger.info("数据库结构已升级: %s.%s", table, target_name)


def _ensure_owner_columns(engine: Engine) -> None:
    """给既有手工账本和回测记录补充可空所有者列。"""
    for table in ("quant_trade", "quant_backtest_run"):
        inspector = inspect(engine)
        if not inspector.has_table(table):
            continue
        columns = {item["name"] for item in inspector.get_columns(table)}
        if "user_id" not in columns:
            with engine.begin() as connection:
                connection.exec_driver_sql(
                    f"ALTER TABLE `{table}` ADD COLUMN `user_id` BIGINT NULL"
                )
            logger.info("数据库结构已升级: %s.user_id", table)
        inspector = inspect(engine)
        indexes = {item["name"] for item in inspector.get_indexes(table)}
        index_name = f"ix_{table}_user_id"
        if index_name not in indexes:
            with engine.begin() as connection:
                connection.exec_driver_sql(
                    f"CREATE INDEX `{index_name}` ON `{table}` (`user_id`)"
                )
