"""Alembic 迁移链的结构断言。

覆盖 brief §3.1-3.6 的 schema 目标态。所有断言都是精确期望值,
不是「跑通不报错」—— 逐列核对类型、可空、主键、索引与 seed 数据。

取代原 tests/test_schema.py 对 app/schema.py 手写 ALTER 的测试:
该模块已删除,其逻辑吸收进 alembic 基线 revision(见 logs/decisions-migrate.md D9)。

全部在临时 sqlite 文件库上跑,不连任何 MySQL(生产库严格只读)。
"""
from __future__ import annotations

import json
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
    assert heads == {"0020_drop_stock_is_watch"}
    assert current_heads(migrated_db) == heads


def test_all_expected_tables_exist(migrated_db):
    tables = set(inspect(migrated_db).get_table_names())
    assert tables == {
        "alembic_version",
        "quant_adjust_factor",
        "quant_backtest_equity",
        "quant_backtest_run",
        "quant_daily_bar",
        "quant_data_quality_cache",
        "quant_evidence_promotion",
        "quant_experiment",
        "quant_experiment_trial",
        "quant_factor_daily",
        "quant_fundamental_snapshot",
        "quant_index_member",
        "quant_pick",
        "quant_pool",
        "quant_pool_grant",
        "quant_pool_member",
        "quant_research_plan",
        "quant_research_plan_item",
        "quant_signal",
        "quant_snapshot",
        "quant_stock",
        "quant_strategy",
        "quant_strategy_eval",
        "quant_trade",
        "quant_trade_calendar",
        "quant_user_settings",
        "quant_valuation_snapshot",
        "quant_watchlist",
    }


# --- §3.1 P0:user_id 类型 ------------------------------------------------


@pytest.mark.parametrize(
    ("table", "nullable"),
    [
        ("quant_watchlist", False),
        ("quant_trade", False),
        ("quant_backtest_run", False),
    ],
)
def test_user_id_is_varchar_36(migrated_db, table, nullable):
    """共享 users.id 是 VARCHAR(36) UUID,量化侧必须同类型,否则全线 401。

    可空性表达真实约束:系统尚未运营,库中「历史数据」经核实全是开发期测试
    垃圾(已备份后清空),没有遗留数据要兼容,故 trade/backtest_run 的 user_id
    收紧为 NOT NULL(0009)。quant_pool 例外 —— NULL 表示预置池。
    """
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
    # 自选在 quant_watchlist;主表不再挂 is_watch(0020)
    assert "is_watch" not in columns


def test_pool_member_has_no_date_column(migrated_db):
    """已定约束:池成员只存代码,不带日期。"""
    columns = _columns(migrated_db, "quant_pool_member")
    assert set(columns) == {"pool_id", "code"}
    pk = inspect(migrated_db).get_pk_constraint("quant_pool_member")
    assert pk["constrained_columns"] == ["pool_id", "code"]


def test_trade_calendar_shape(migrated_db):
    columns = _columns(migrated_db, "quant_trade_calendar")
    assert set(columns) == {"date", "is_open", "source"}
    pk = inspect(migrated_db).get_pk_constraint("quant_trade_calendar")
    assert pk["constrained_columns"] == ["date"]


def test_backtest_run_has_costs_and_pool_id(migrated_db):
    """回测可复现:固化当时费率(REVIEW 2.2)。"""
    columns = _columns(migrated_db, "quant_backtest_run")
    assert "costs" in columns
    assert "pool_id" in columns


def test_dynamic_strategy_persistence_shape(migrated_db):
    """当前策略是唯一事实源；历史执行证据另存快照、哈希和指纹。"""
    strategy = _columns(migrated_db, "quant_strategy")
    assert {
        "spec_schema_version", "spec", "spec_hash", "research_status",
        "updated_at",
    } <= set(strategy)
    assert all(not strategy[name]["nullable"] for name in (
        "spec_schema_version", "spec", "spec_hash", "research_status",
        "updated_at",
    ))

    backtest = _columns(migrated_db, "quant_backtest_run")
    assert {
        "strategy_spec_snapshot", "strategy_spec_hash", "compiler_version",
        "component_versions", "data_fingerprint", "universe_fingerprint",
        "cost_fingerprint", "execution_fingerprint",
    } <= set(backtest)
    # 0014 前的历史运行没有可靠创建时快照，迁移不能用当前策略伪造证据。
    assert all(backtest[name]["nullable"] for name in (
        "strategy_spec_snapshot", "strategy_spec_hash", "compiler_version",
        "component_versions", "data_fingerprint", "universe_fingerprint",
        "cost_fingerprint", "execution_fingerprint",
    ))

    plan = _columns(migrated_db, "quant_research_plan")
    assert {"strategy_spec_snapshot", "strategy_spec_hash"} <= set(plan)
    assert plan["strategy_spec_snapshot"]["nullable"]
    assert plan["strategy_spec_hash"]["nullable"]
    assert _columns(migrated_db, "quant_signal")["spec_hash"]["nullable"]
    assert _columns(migrated_db, "quant_strategy_eval")["spec_hash"]["nullable"]

    expected_indexes = {
        "quant_strategy": {
            "ix_quant_strategy_spec_hash",
            "ix_quant_strategy_research_status",
        },
        "quant_backtest_run": {
            "ix_quant_backtest_run_strategy_spec_hash",
            "ix_quant_backtest_run_execution_fingerprint",
        },
        "quant_research_plan": {"ix_quant_research_plan_strategy_spec_hash"},
        "quant_signal": {"ix_quant_signal_spec_hash"},
        "quant_strategy_eval": {"ix_quant_strategy_eval_spec_hash"},
    }
    for table, names in expected_indexes.items():
        actual = {item["name"] for item in inspect(migrated_db).get_indexes(table)}
        assert names <= actual


def test_strategy_eval_has_indexed_batch_id(migrated_db):
    """排行榜混批修复依赖 batch_id(REVIEW 4.2)。"""
    columns = _columns(migrated_db, "quant_strategy_eval")
    assert str(columns["batch_id"]["type"]).upper() == "VARCHAR(36)"
    index_names = {
        i["name"] for i in inspect(migrated_db).get_indexes("quant_strategy_eval")}
    assert "ix_quant_strategy_eval_batch_id" in index_names


def test_system_pools_are_seeded(migrated_db):
    """预置系统级池:owner_id 为哨兵 UUID、is_system=1,id 固定供代码引用。"""
    with migrated_db.connect() as conn:
        rows = conn.execute(text(
            "SELECT id, kind, ref, owner_id, is_system, name, min_list_days "
            "FROM quant_pool ORDER BY id"
        )).all()
    sys_id = "00000000-0000-0000-0000-000000000000"
    assert [tuple(row) for row in rows] == [
        (1, "index", "hs300_zz500", sys_id, 1, "沪深300+中证500", 0),
        (2, "all", None, sys_id, 1, "全部A股", 60),
        (3, "index", "hs300", sys_id, 1, "沪深300", 0),
        (4, "index", "zz500", sys_id, 1, "中证500", 0),
    ]


def test_pool_name_is_unique_per_owner(migrated_db):
    """同一属主不能有两个同名池。"""
    from sqlalchemy.exc import IntegrityError

    insert = text(
        "INSERT INTO quant_pool "
        "(kind, ref, owner_id, is_system, name, min_list_days, created_at) "
        "VALUES ('static', NULL, 'user-a', 0, '我的池', 60, '2026-07-25 00:00:00')"
    )
    with migrated_db.begin() as conn:
        conn.execute(insert)
    try:
        with pytest.raises(IntegrityError):
            with migrated_db.begin() as conn:
                conn.execute(insert)
    finally:
        with migrated_db.begin() as conn:
            conn.execute(text("DELETE FROM quant_pool WHERE owner_id = 'user-a'"))


def test_system_pool_name_uniqueness_actually_holds(migrated_db):
    """系统池同名也必须被拦住 —— 这是改掉 user_id NULL 的核心动机。

    旧结构用 `user_id IS NULL` 表示系统级,而 SQL 里 NULL 互不相等,
    `UniqueConstraint("user_id","name")` 对预置池**完全失效**:实测可插入
    3 条同名系统池而不报错。保护恰好在最需要的地方失灵(预置池全用户共用,
    重复的影响面最大)。改 owner_id NOT NULL 后约束才真正生效。
    """
    from sqlalchemy.exc import IntegrityError

    sys_id = "00000000-0000-0000-0000-000000000000"
    dup = text(
        "INSERT INTO quant_pool "
        "(kind, ref, owner_id, is_system, name, min_list_days, created_at) "
        "VALUES ('all', NULL, :owner, 1, '全部A股', 60, '2026-07-25 00:00:00')"
    ).bindparams(owner=sys_id)
    with pytest.raises(IntegrityError):
        with migrated_db.begin() as conn:
            conn.execute(dup)


def test_system_strategies_are_seeded(migrated_db):
    """预置公共策略:6 个算法模板各一条,归哨兵 UUID、is_system=1、id 固定。"""
    with migrated_db.connect() as conn:
        rows = conn.execute(text(
            "SELECT id, owner_id, is_system, name, template, kind, enabled "
            "FROM quant_strategy ORDER BY id"
        )).all()
    sys_id = "00000000-0000-0000-0000-000000000000"
    assert [tuple(row) for row in rows] == [
        (1, sys_id, 1, "双均线趋势策略", "ma_cross", "single", 1),
        (2, sys_id, 1, "价格突破策略", "breakout", "single", 1),
        (3, sys_id, 1, "上升趋势中的超跌反弹策略", "mean_reversion", "single", 1),
        (4, sys_id, 1, "缩量整理后的放量突破策略", "volume_breakout", "single", 1),
        (5, sys_id, 1, "强势股票轮动策略", "momentum_rotation", "portfolio", 1),
        (6, sys_id, 1, "多指标综合评分持有策略", "multifactor_hold", "portfolio", 1),
    ]


def test_system_strategies_have_complete_hashed_specs(migrated_db):
    """六个公共策略必须是可校验的完整 StrategySpec，而非模板名占位。"""
    from app.strategy.spec import validate_strategy_spec

    with migrated_db.connect() as conn:
        rows = conn.execute(text(
            "SELECT template, kind, spec_schema_version, spec, spec_hash, "
            "research_status, updated_at FROM quant_strategy "
            "WHERE is_system = 1 ORDER BY id"
        )).mappings().all()

    assert len(rows) == 6
    for row in rows:
        raw_spec = row["spec"]
        spec = raw_spec if isinstance(raw_spec, dict) else json.loads(raw_spec)
        result = validate_strategy_spec(spec)
        assert result.valid, (row["template"], result.capability)
        assert spec["kind"] == row["kind"]
        assert row["spec_schema_version"] == spec["schema_version"] == 1
        assert row["spec_hash"] == result.spec_hash
        assert row["research_status"] == "unverified"
        assert row["updated_at"] is not None


def test_init_sql_uses_same_strategy_specs_and_head():
    """空库初始化种子必须与运行时预置规格、Alembic head 完全一致。"""
    from app.strategy.presets import SYSTEM_STRATEGY_SPECS
    from app.strategy.spec import canonical_spec_json, strategy_spec_hash

    source = (QUANT_DIR / "sql" / "init.sql").read_text()
    assert "Schema revision: 0016_experiment_registry" in source
    assert "VALUES ('0016_experiment_registry');" in source
    for template, spec in SYSTEM_STRATEGY_SPECS.items():
        canonical = canonical_spec_json(spec).replace("'", "''")
        assert f"'{template}'" in source
        assert f"'{canonical}'" in source
        assert f"'{strategy_spec_hash(spec)}'" in source


def test_legacy_strategy_params_migrate_into_complete_spec():
    """已有用户策略的显式参数必须写进规格，不能退回系统默认值。"""
    from app.strategy.presets import get_preset_spec
    from app.strategy.spec import strategy_spec_hash

    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "legacy-strategy.db"
        result = _alembic(db_path, "upgrade", "0013_research_plan")
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        params = {
            "entry": 55,
            "exit": 15,
            "max_entry_premium": 0.03,
            "risk_overlay": {
                "enabled": True,
                "type": "fixed_pct",
                "value": 0.1,
                "atr_period": 14,
            },
        }
        with engine.begin() as conn:
            conn.execute(text(
                "INSERT INTO quant_strategy "
                "(owner_id, is_system, name, template, kind, params, enabled, "
                " created_at) VALUES "
                "('user-a', 0, '自定义突破', 'breakout', 'single', :params, 1, "
                " '2026-07-25 00:00:00')"
            ), {"params": json.dumps(params)})
        engine.dispose()

        result = _alembic(db_path, "upgrade", "head")
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        try:
            with engine.connect() as conn:
                row = conn.execute(text(
                    "SELECT template, params, spec, spec_hash, research_status "
                    "FROM quant_strategy WHERE owner_id = 'user-a'"
                )).mappings().one()
            migrated_spec = (
                row["spec"] if isinstance(row["spec"], dict)
                else json.loads(row["spec"])
            )
            expected = get_preset_spec("breakout", params)
            assert migrated_spec == expected.model_dump(mode="json")
            assert row["spec_hash"] == strategy_spec_hash(expected)
            assert row["research_status"] == "unverified"
            assert json.loads(row["params"]) == params
        finally:
            engine.dispose()


def test_dynamic_strategy_migration_refuses_unknown_template_before_ddl():
    """未知模板必须在 ALTER TABLE 前中止，避免 MySQL 留下半套 0014 schema。"""
    with tempfile.TemporaryDirectory() as tmp:
        db_path = Path(tmp) / "unknown-strategy.db"
        result = _alembic(db_path, "upgrade", "0013_research_plan")
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        with engine.begin() as conn:
            conn.execute(text(
                "INSERT INTO quant_strategy "
                "(owner_id, is_system, name, template, kind, params, enabled, "
                " created_at) VALUES "
                "('user-a', 0, '未知策略', 'missing_template', 'single', '{}', 1, "
                " '2026-07-25 00:00:00')"
            ))
        engine.dispose()

        result = _alembic(db_path, "upgrade", "head")
        assert result.returncode != 0
        assert "未知模板" in result.stderr

        engine = create_engine(f"sqlite+pysqlite:///{db_path}")
        try:
            assert "spec" not in _columns(engine, "quant_strategy")
            with engine.connect() as conn:
                assert conn.execute(text(
                    "SELECT version_num FROM alembic_version"
                )).scalar_one() == "0013_research_plan"
                assert conn.execute(text(
                    "SELECT COUNT(*) FROM quant_strategy "
                    "WHERE template = 'missing_template'"
                )).scalar_one() == 1
        finally:
            engine.dispose()


def test_seeded_strategies_match_code_templates(migrated_db):
    """seed 的 template/kind 必须与代码里的模块一致。

    迁移里的 seed 是写死的常量,模块改名或改 KIND 时不会自动跟着变 ——
    漂移会让策略行指向不存在的模板(信号引擎跳过、回测直接报错)。
    """
    from app.strategy.strategies import REGISTRY

    with migrated_db.connect() as conn:
        rows = conn.execute(text(
            "SELECT template, kind FROM quant_strategy WHERE is_system = 1"
        )).all()
    assert {row[0] for row in rows} == set(REGISTRY)
    for template, kind in rows:
        assert kind == REGISTRY[template].KIND


def test_strategy_name_is_unique_per_owner(migrated_db):
    """同一属主不能有两个同名策略(不同用户可以同名)。"""
    from sqlalchemy.exc import IntegrityError

    insert = text(
        "INSERT INTO quant_strategy "
        "(owner_id, is_system, name, template, kind, params, "
        " spec_schema_version, spec, spec_hash, research_status, enabled, "
        " created_at, updated_at) "
        "SELECT :owner, 0, '我的策略', template, kind, params, "
        " spec_schema_version, spec, spec_hash, 'unverified', 1, "
        " '2026-07-25 00:00:00', '2026-07-25 00:00:00' "
        "FROM quant_strategy WHERE id = 1"
    )
    with migrated_db.begin() as conn:
        conn.execute(insert.bindparams(owner="user-a"))
    try:
        with pytest.raises(IntegrityError):
            with migrated_db.begin() as conn:
                conn.execute(insert.bindparams(owner="user-a"))
        # 换个属主同名则允许
        with migrated_db.begin() as conn:
            conn.execute(insert.bindparams(owner="user-b"))
    finally:
        with migrated_db.begin() as conn:
            conn.execute(text(
                "DELETE FROM quant_strategy WHERE owner_id IN ('user-a','user-b')"))


@pytest.mark.parametrize(
    ("table", "ondelete"),
    [
        # 派生数据随策略删除;回测是用户资产,RESTRICT 挡住删除(见 alembic 0012)
        ("quant_signal", "CASCADE"),
        ("quant_strategy_eval", "CASCADE"),
        ("quant_backtest_run", "RESTRICT"),
    ],
)
def test_strategy_id_foreign_keys(migrated_db, table, ondelete):
    """三张表的 strategy 字符串列已换成 strategy_id 外键,且 NOT NULL。"""
    columns = _columns(migrated_db, table)
    assert "strategy" not in columns, f"{table} 仍有旧的 strategy 字符串列"
    assert bool(columns["strategy_id"]["nullable"]) is False

    fks = inspect(migrated_db).get_foreign_keys(table)
    match = [fk for fk in fks if fk["constrained_columns"] == ["strategy_id"]]
    assert match, f"{table}.strategy_id 缺外键"
    assert match[0]["referred_table"] == "quant_strategy"
    assert (match[0]["options"].get("ondelete") or "").upper() == ondelete


def test_signal_unique_constraint_uses_strategy_id(migrated_db):
    """uq_signal 从 (code,date,strategy,side) 重建为按 strategy_id。"""
    uniques = {
        u["name"]: u["column_names"]
        for u in inspect(migrated_db).get_unique_constraints("quant_signal")
    }
    assert uniques["uq_signal"] == ["code", "date", "strategy_id", "side"]


def test_research_plan_snapshot_shape(migrated_db):
    """研究计划固化版本、日期、规则和回测证据，不依赖当前策略参数重算。"""
    columns = _columns(migrated_db, "quant_research_plan")
    required = {
        "strategy_id", "strategy_name", "template", "strategy_version",
        "params_snapshot", "plan_type", "data_date", "generated_at",
        "next_execution_date", "status", "status_reason", "entry_observation",
        "risk_rules", "take_profit", "native_exit", "portfolio_summary",
        "backtest_run_id", "backtest_evidence", "supersedes_plan_id",
    }
    assert required <= set(columns)
    assert all(not columns[name]["nullable"] for name in (
        "strategy_id", "strategy_version", "params_snapshot", "plan_type",
        "data_date", "generated_at", "status", "entry_observation",
        "risk_rules", "take_profit", "native_exit", "backtest_evidence",
    ))


def test_research_plan_items_are_unique_per_stock(migrated_db):
    columns = _columns(migrated_db, "quant_research_plan_item")
    assert {"plan_id", "code", "previous_weight", "target_weight",
            "change_type", "reasons", "risk_snapshot"} <= set(columns)
    assert "score_details" in columns
    uniques = {
        item["name"]: item["column_names"]
        for item in inspect(migrated_db).get_unique_constraints(
            "quant_research_plan_item")
    }
    assert uniques["uq_research_plan_item"] == ["plan_id", "code"]


def test_signal_points_to_latest_research_plan(migrated_db):
    columns = _columns(migrated_db, "quant_signal")
    assert columns["plan_id"]["nullable"] is True
    fks = inspect(migrated_db).get_foreign_keys("quant_signal")
    match = [fk for fk in fks if fk["constrained_columns"] == ["plan_id"]]
    assert match and match[0]["referred_table"] == "quant_research_plan"
    assert (match[0]["options"].get("ondelete") or "").upper() == "SET NULL"


def test_snapshot_pk_types_upgraded_to_bigint():
    """全市场日频会超 21 亿行,Integer 主键会溢出(REVIEW 五)。"""
    from app.models import FundamentalSnapshot, ValuationSnapshot

    for model in (ValuationSnapshot, FundamentalSnapshot):
        assert model.id.type.__class__.__name__ == "BigInteger"


# --- §3.1 NOT NULL 收紧(默认 no-op) -----------------------------------


def test_user_owned_columns_are_not_null_after_0009(migrated_db):
    """0009 无条件收紧 user_id / batch_id 为 NOT NULL。

    0006 曾设计成需环境变量显式开启的 no-op,理由是「人类无从决定时机」——
    当时生产库有遗留的 user_id IS NULL 行。现在时机明确:系统未运营,那批行
    经核实全是开发期测试数据(已备份后清空),schema 应表达真实约束。
    """
    for table in ("quant_trade", "quant_backtest_run"):
        assert bool(_columns(migrated_db, table)["user_id"]["nullable"]) is False
    assert bool(_columns(migrated_db, "quant_strategy_eval")
                ["batch_id"]["nullable"]) is False


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
    """改造前的既有库应能增量升级而不重建,数据保留。

    注意 user_id 必须有值:0009 收紧为 NOT NULL 并自带前置校验,仍有 NULL 行
    时会报错中止(那条路径由 test_not_null_revision_refuses_while_orphan_rows_exist
    覆盖)。系统未运营,不存在需要跨过这一步的遗留数据。
    """
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
            conn.execute(text(
                "INSERT INTO quant_trade "
                "(user_id, code, trade_date, side, price, qty, fee, note) "
                "VALUES (42, 'sh.600519', '2026-07-24', 'buy', 1400.5, 100, 5, '')"
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
                    == ("42", "sh.600519", 1400.5)
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
