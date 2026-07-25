"""Alembic 迁移链的结构断言。

覆盖 brief §3.1-3.6 的 schema 目标态。所有断言都是精确期望值,
不是「跑通不报错」—— 逐列核对类型、可空、主键、索引与 seed 数据。

取代原 tests/test_schema.py 对 app/schema.py 手写 ALTER 的测试:
该模块已删除,其逻辑吸收进 alembic 基线 revision(见 logs/decisions-migrate.md D9)。

全部在临时 sqlite 文件库上跑,不连任何 MySQL(生产库严格只读)。
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from sqlalchemy import create_engine, inspect, text

QUANT_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(QUANT_DIR))


def _alembic(db_path: Path, *args: str, env: dict | None = None):
    url = f"sqlite+pysqlite:///{db_path}"
    return subprocess.run(
        [sys.executable, "-m", "alembic", "-x", f"db_url={url}", *args],
        cwd=QUANT_DIR, capture_output=True, text=True,
        env={**os.environ, **(env or {})},
    )


@pytest.fixture(scope="module")
def migrated_db():
    """全新库跑完 alembic upgrade head 的结果,模块内复用(迁移较慢)。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "migrated.db"
        result = _alembic(db_path, "upgrade", "head")
        assert result.returncode == 0, (
            f"alembic upgrade head 失败:\n{result.stdout}\n{result.stderr}")
        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        yield engine
        engine.dispose()


def _columns(engine, table: str) -> dict[str, dict]:
    return {col["name"]: col for col in inspect(engine).get_columns(table)}


# --- §3.2 迁移链本身 -------------------------------------------------------


def test_migration_chain_is_single_linear_head(migrated_db):
    """必须只有一个 head,否则日常 upgrade head 会报 multiple heads。"""
    from app.migrations import current_heads, expected_heads

    heads = expected_heads()
    assert heads == {"0006_user_id_not_null"}
    assert current_heads(migrated_db) == heads


def test_all_expected_tables_exist(migrated_db):
    tables = set(inspect(migrated_db).get_table_names())
    assert tables == {
        "alembic_version",
        "quant_backtest_equity",
        "quant_backtest_run",
        "quant_daily_bar",
        "quant_factor_daily",
        "quant_fundamental_snapshot",
        "quant_index_member",
        "quant_pick",
        "quant_pool",
        "quant_pool_member",
        "quant_signal",
        "quant_snapshot",
        "quant_stock",
        "quant_strategy_eval",
        "quant_trade",
        "quant_trade_calendar",
        "quant_valuation_snapshot",
        "quant_watchlist",
    }


# --- §3.1 P0:user_id 类型 ------------------------------------------------


@pytest.mark.parametrize(
    ("table", "nullable"),
    [
        ("quant_watchlist", False),
        ("quant_trade", True),
        ("quant_backtest_run", True),
        ("quant_pool", True),
    ],
)
def test_user_id_is_varchar_36(migrated_db, table, nullable):
    """共享 users.id 是 VARCHAR(36) UUID,量化侧必须同类型,否则全线 401。"""
    column = _columns(migrated_db, table)["user_id"]
    assert str(column["type"]).upper() == "VARCHAR(36)"
    assert bool(column["nullable"]) is nullable


def test_watchlist_primary_key_is_user_id_and_code(migrated_db):
    """user_id 是复合主键的一部分,改类型必须正确重建 PK。"""
    pk = inspect(migrated_db).get_pk_constraint("quant_watchlist")
    assert pk["constrained_columns"] == ["user_id", "code"]


def test_uuid_user_id_round_trips(migrated_db):
    """36 位 UUID 能完整存取,不被截断。"""
    uuid = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"
    assert len(uuid) == 36
    with migrated_db.begin() as conn:
        conn.execute(
            text("INSERT INTO quant_watchlist (user_id, code, created_at) "
                 "VALUES (:u, 'sh.600519', '2026-07-25 00:00:00')"),
            {"u": uuid},
        )
        stored = conn.execute(
            text("SELECT user_id FROM quant_watchlist WHERE code = 'sh.600519'")
        ).scalar_one()
        conn.execute(text("DELETE FROM quant_watchlist WHERE code = 'sh.600519'"))
    assert stored == uuid


# --- §3.3 DECIMAL ---------------------------------------------------------


@pytest.mark.parametrize(
    ("table", "column", "expected"),
    [
        ("quant_daily_bar", "open", "NUMERIC(12, 4)"),
        ("quant_daily_bar", "high", "NUMERIC(12, 4)"),
        ("quant_daily_bar", "low", "NUMERIC(12, 4)"),
        ("quant_daily_bar", "close", "NUMERIC(12, 4)"),
        ("quant_daily_bar", "raw_close", "NUMERIC(12, 4)"),
        ("quant_daily_bar", "volume", "NUMERIC(20, 2)"),
        ("quant_daily_bar", "amount", "NUMERIC(20, 2)"),
        ("quant_snapshot", "price", "NUMERIC(12, 4)"),
        ("quant_snapshot", "pct_chg", "NUMERIC(9, 4)"),
        ("quant_signal", "price", "NUMERIC(12, 4)"),
        ("quant_trade", "price", "NUMERIC(12, 4)"),
        ("quant_trade", "qty", "NUMERIC(18, 4)"),
        ("quant_trade", "fee", "NUMERIC(18, 4)"),
        ("quant_valuation_snapshot", "total_market_cap", "NUMERIC(20, 2)"),
        ("quant_backtest_equity", "equity", "NUMERIC(18, 8)"),
    ],
)
def test_price_columns_are_decimal(migrated_db, table, column, expected):
    assert str(_columns(migrated_db, table)[column]["type"]).upper() == expected


@pytest.mark.parametrize(
    ("table", "column"),
    [
        ("quant_factor_daily", "mom20"),
        ("quant_factor_daily", "amount_avg20"),
        ("quant_valuation_snapshot", "pe_ttm"),
        ("quant_valuation_snapshot", "dividend_yield"),
        ("quant_fundamental_snapshot", "roe"),
        ("quant_pick", "score"),
    ],
)
def test_ratio_columns_stay_float(migrated_db, table, column):
    """比率/因子列刻意保持 Float(decisions D4),改 DECIMAL 只添麻烦。"""
    assert str(_columns(migrated_db, table)[column]["type"]).upper() == "FLOAT"


def test_decimal_columns_return_float_not_decimal():
    """asdecimal=False:Python 侧取 float,避免下游 Decimal/float 混算 TypeError。"""
    from app.models import DailyBar, Trade

    for column in (DailyBar.close, DailyBar.volume, Trade.price, Trade.fee):
        assert column.type.asdecimal is False


def test_decimal_precision_exceeds_what_mysql_float_could_hold(migrated_db):
    """8 位有效数字的价格必须无损往返。

    诚实说明测试边界:sqlite 把 NUMERIC 当 float64 存,所以这里验证的是
    「声明的精度足够 + 取回来仍是精确的 float」。真正消除单精度截断的是 MySQL 侧
    DECIMAL(12,4) 的列类型(由 test_price_columns_are_decimal 断言),
    本用例佐证该精度需求确实超出旧 Float(单精度)的能力。
    """
    import numpy as np

    price = 1234.5678  # 8 位有效数字
    assert float(np.float32(price)) != price  # 旧 Float(单精度)存不住

    with migrated_db.begin() as conn:
        conn.execute(
            text("INSERT INTO quant_trade "
                 "(user_id, code, trade_date, side, price, qty, fee, note) "
                 "VALUES ('u', 'sh.600519', '2026-07-24', 'buy', :p, 100, 0, '')"),
            {"p": price},
        )
    try:
        with migrated_db.connect() as conn:
            stored = conn.execute(text(
                "SELECT price FROM quant_trade WHERE code = 'sh.600519'")).scalar_one()
        assert stored == price
        assert isinstance(stored, float)  # asdecimal=False:不是 Decimal
    finally:
        with migrated_db.begin() as conn:
            conn.execute(text("DELETE FROM quant_trade WHERE code = 'sh.600519'"))


# --- §3.4 quant_daily_bar 自然主键 ---------------------------------------


def test_daily_bar_uses_natural_primary_key(migrated_db):
    pk = inspect(migrated_db).get_pk_constraint("quant_daily_bar")
    assert pk["constrained_columns"] == ["code", "date"]
    assert "id" not in _columns(migrated_db, "quant_daily_bar")


def test_daily_bar_redundant_indexes_are_dropped(migrated_db):
    """ix_..._code 与 uq_daily_bar_code_date 都与新 PK 前缀重复;date 索引保留。"""
    inspector = inspect(migrated_db)
    index_names = {i["name"] for i in inspector.get_indexes("quant_daily_bar")}
    unique_names = {
        u["name"] for u in inspector.get_unique_constraints("quant_daily_bar")}
    assert "ix_quant_daily_bar_code" not in index_names
    assert "uq_daily_bar_code_date" not in index_names | unique_names
    assert "ix_quant_daily_bar_date" in index_names


def test_daily_bar_rejects_duplicate_code_date(migrated_db):
    """自然主键必须真的挡住重复 (code, date)。"""
    from sqlalchemy.exc import IntegrityError

    insert = text(
        "INSERT INTO quant_daily_bar "
        "(code, date, open, high, low, close, raw_close, volume, amount) "
        "VALUES ('sh.600519', '2026-07-24', 1400, 1420, 1390, 1410, 1410, 1000, 1e6)"
    )
    with migrated_db.begin() as conn:
        conn.execute(insert)
    try:
        with pytest.raises(IntegrityError):
            with migrated_db.begin() as conn:
                conn.execute(insert)
    finally:
        with migrated_db.begin() as conn:
            conn.execute(text("DELETE FROM quant_daily_bar WHERE code = 'sh.600519'"))


def test_other_redundant_indexes_are_dropped(migrated_db):
    """factor_daily.code 与 pick.date 索引各自与唯一键前缀冗余(scope-gap 3.6)。"""
    inspector = inspect(migrated_db)
    factor = {i["name"] for i in inspector.get_indexes("quant_factor_daily")}
    pick = {i["name"] for i in inspector.get_indexes("quant_pick")}
    assert "ix_quant_factor_daily_code" not in factor
    assert "ix_quant_factor_daily_date" in factor
    assert "ix_quant_pick_date" not in pick
    assert "ix_quant_pick_code" in pick


# --- §3.5 新表 / 新列 / seed --------------------------------------------


def test_stock_has_listing_columns(migrated_db):
    columns = _columns(migrated_db, "quant_stock")
    assert str(columns["list_date"]["type"]).upper() == "DATE"
    assert str(columns["delist_date"]["type"]).upper() == "DATE"
    assert bool(columns["list_date"]["nullable"]) is True
    assert bool(columns["delist_date"]["nullable"]) is True
    assert bool(columns["is_st"]["nullable"]) is False


def test_pool_member_has_no_date_column(migrated_db):
    """已定约束:池成员只存代码,不带日期。"""
    columns = _columns(migrated_db, "quant_pool_member")
    assert set(columns) == {"pool_id", "code"}
    pk = inspect(migrated_db).get_pk_constraint("quant_pool_member")
    assert pk["constrained_columns"] == ["pool_id", "code"]


def test_trade_calendar_shape(migrated_db):
    columns = _columns(migrated_db, "quant_trade_calendar")
    assert set(columns) == {"date", "is_open"}
    pk = inspect(migrated_db).get_pk_constraint("quant_trade_calendar")
    assert pk["constrained_columns"] == ["date"]


def test_backtest_run_has_costs_and_pool_id(migrated_db):
    """回测可复现:固化当时费率(REVIEW 2.2)。"""
    columns = _columns(migrated_db, "quant_backtest_run")
    assert "costs" in columns
    assert "pool_id" in columns


def test_strategy_eval_has_indexed_batch_id(migrated_db):
    """排行榜混批修复依赖 batch_id(REVIEW 4.2)。"""
    columns = _columns(migrated_db, "quant_strategy_eval")
    assert str(columns["batch_id"]["type"]).upper() == "VARCHAR(36)"
    index_names = {
        i["name"] for i in inspect(migrated_db).get_indexes("quant_strategy_eval")}
    assert "ix_quant_strategy_eval_batch_id" in index_names


def test_system_pools_are_seeded(migrated_db):
    """预置系统级池,user_id 全为 NULL,id 固定供代码引用。"""
    with migrated_db.connect() as conn:
        rows = conn.execute(text(
            "SELECT id, kind, ref, user_id, name, min_list_days "
            "FROM quant_pool ORDER BY id"
        )).all()
    assert [tuple(row) for row in rows] == [
        (1, "index", "hs300_zz500", None, "沪深300+中证500", 0),
        (2, "all", None, None, "全部A股", 60),
        (3, "index", "hs300", None, "沪深300", 0),
        (4, "index", "zz500", None, "中证500", 0),
    ]


def test_pool_name_is_unique_per_user(migrated_db):
    """同一用户不能有两个同名池。"""
    from sqlalchemy.exc import IntegrityError

    insert = text(
        "INSERT INTO quant_pool (kind, ref, user_id, name, min_list_days, created_at) "
        "VALUES ('static', NULL, 'user-a', '我的池', 60, '2026-07-25 00:00:00')"
    )
    with migrated_db.begin() as conn:
        conn.execute(insert)
    try:
        with pytest.raises(IntegrityError):
            with migrated_db.begin() as conn:
                conn.execute(insert)
    finally:
        with migrated_db.begin() as conn:
            conn.execute(text("DELETE FROM quant_pool WHERE user_id = 'user-a'"))


def test_snapshot_pk_types_upgraded_to_bigint():
    """全市场日频会超 21 亿行,Integer 主键会溢出(REVIEW 五)。"""
    from app.models import FundamentalSnapshot, ValuationSnapshot

    for model in (ValuationSnapshot, FundamentalSnapshot):
        assert model.id.type.__class__.__name__ == "BigInteger"


# --- §3.1 NOT NULL 收紧(默认 no-op) -----------------------------------


def test_not_null_revision_is_noop_by_default(migrated_db):
    """默认路径下 user_id 仍可空,让遗留数据先能被认领脚本处理。"""
    for table in ("quant_trade", "quant_backtest_run"):
        assert bool(_columns(migrated_db, table)["user_id"]["nullable"]) is True


def test_not_null_revision_refuses_while_orphan_rows_exist():
    """开关打开但仍有 user_id IS NULL 时必须报错中止,不能把遗留数据锁成不可见。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "orphan.db"
        assert _alembic(
            db_path, "upgrade", "0005_pools_and_new_columns").returncode == 0

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        with engine.begin() as conn:
            conn.execute(text(
                "INSERT INTO quant_trade "
                "(user_id, code, trade_date, side, price, qty, fee, note) "
                "VALUES (NULL, 'sh.600519', '2026-07-24', 'buy', 10, 100, 1, '')"
            ))
        engine.dispose()

        result = _alembic(
            db_path, "upgrade", "head",
            env={"QUANT_ENFORCE_USER_ID_NOT_NULL": "1"},
        )
        assert result.returncode != 0
        assert "user_id IS NULL" in result.stderr


def test_not_null_revision_tightens_when_enabled_and_data_claimed():
    """认领完毕后开关生效,user_id 变 NOT NULL。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "claimed.db"
        assert _alembic(
            db_path, "upgrade", "0005_pools_and_new_columns").returncode == 0

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        with engine.begin() as conn:
            conn.execute(text(
                "INSERT INTO quant_trade "
                "(user_id, code, trade_date, side, price, qty, fee, note) VALUES "
                "('3f2504e0-4f89-11d3-9a0c-0305e82c3301', 'sh.600519', "
                "'2026-07-24', 'buy', 10, 100, 1, '')"
            ))
        engine.dispose()

        result = _alembic(
            db_path, "upgrade", "head",
            env={"QUANT_ENFORCE_USER_ID_NOT_NULL": "1"},
        )
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        try:
            for table in ("quant_trade", "quant_backtest_run"):
                assert bool(_columns(engine, table)["user_id"]["nullable"]) is False
        finally:
            engine.dispose()


# --- §3.2 既有库 stamp 路径与回滚 ---------------------------------------


def test_legacy_database_upgrades_incrementally_preserving_data():
    """改造前的既有库应能增量升级而不重建,遗留数据保留。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "legacy.db"
        # 先建出基线形态(模拟改造前的既有库)
        assert _alembic(db_path, "upgrade", "0001_baseline").returncode == 0

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        # 基线形态:BIGINT user_id、带代理 id 的 daily_bar
        assert str(_columns(engine, "quant_trade")["user_id"]["type"]).upper() \
            == "BIGINT"
        assert "id" in _columns(engine, "quant_daily_bar")
        with engine.begin() as conn:
            # 遗留数据:user_id IS NULL 的成交(对应生产库现存的 3 条)
            conn.execute(text(
                "INSERT INTO quant_trade "
                "(user_id, code, trade_date, side, price, qty, fee, note) "
                "VALUES (NULL, 'sh.600519', '2026-07-24', 'buy', 1400.5, 100, 5, '')"
            ))
            conn.execute(text(
                "INSERT INTO quant_daily_bar "
                "(code, date, open, high, low, close, raw_close, volume, amount) "
                "VALUES ('sh.600519', '2026-07-24', 1400, 1420, 1390, 1410, "
                "1410, 1000, 1410000)"
            ))
        engine.dispose()

        assert _alembic(db_path, "upgrade", "head").returncode == 0

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        try:
            # 升级后:user_id 变 VARCHAR(36)、daily_bar 无代理列、数据仍在
            assert str(_columns(engine, "quant_trade")["user_id"]["type"]).upper() \
                == "VARCHAR(36)"
            assert "id" not in _columns(engine, "quant_daily_bar")
            with engine.connect() as conn:
                assert conn.execute(text(
                    "SELECT user_id, code, price FROM quant_trade")).one() \
                    == (None, "sh.600519", 1400.5)
                # 换主键时整表重建,数据不能丢
                assert conn.execute(text(
                    "SELECT code, close FROM quant_daily_bar")).one() \
                    == ("sh.600519", 1410.0)
        finally:
            engine.dispose()


def test_downgrade_returns_to_baseline():
    """downgrade 链可用(回滚方案的前提,见 logs/migration-plan.md)。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "rollback.db"
        assert _alembic(db_path, "upgrade", "head").returncode == 0
        result = _alembic(db_path, "downgrade", "0001_baseline")
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        try:
            assert str(_columns(engine, "quant_trade")["user_id"]["type"]).upper() \
                == "BIGINT"
            assert "id" in _columns(engine, "quant_daily_bar")
            assert "quant_pool" not in inspect(engine).get_table_names()
        finally:
            engine.dispose()
