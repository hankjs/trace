"""既有数据库的小范围幂等迁移。"""
from sqlalchemy import create_engine, inspect

from app.schema import upgrade_research_schema


def test_owner_column_migration_is_idempotent_for_legacy_tables():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    with engine.begin() as connection:
        connection.exec_driver_sql(
            "CREATE TABLE quant_trade (id BIGINT PRIMARY KEY)"
        )
        connection.exec_driver_sql(
            "CREATE TABLE quant_backtest_run (id INTEGER PRIMARY KEY)"
        )

    upgrade_research_schema(engine)
    upgrade_research_schema(engine)

    inspector = inspect(engine)
    for table in ("quant_trade", "quant_backtest_run"):
        columns = {item["name"] for item in inspector.get_columns(table)}
        indexes = {item["name"] for item in inspector.get_indexes(table)}
        assert "user_id" in columns
        assert f"ix_{table}_user_id" in indexes
