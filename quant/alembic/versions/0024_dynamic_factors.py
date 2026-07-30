"""动态因子库:因子定义表、选股配置表、日线因子值 JSON 化

## 背景

策略 DSL 重构后,因子从 7 个硬编码列变为由 `quant_factor_def` 统一描述的可扩展
集合。`quant_factor_daily` 改为单 JSON 列 `values`,以支持新增/删除因子而不改 schema。

## 本次变更

1. 新建 `quant_factor_def`:因子元数据 + DSL 表达式 + 最小 K 线数。
2. 新建 `quant_selection_config`:选股流水线权重、过滤、成交量确认等配置。
3. 将 `quant_factor_daily` 的 7 个固定 Float 列迁移到 `values` JSON 列,保持历史数据。
4. 种子数据:7 个系统因子 + 1 条默认选股配置,与现有 catalog 口径对齐。

## 降级说明

`quant_factor_daily` 的 `values` 中若包含非种子因子字段,降级时会丢失,仅保留
原 7 列能 JSON 提取出来的值。

Revision ID: 0024_dynamic_factors
Revises: 0023_task
"""
from __future__ import annotations

import json
from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0024_dynamic_factors"
down_revision: Union[str, None] = "0023_task"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

# MySQL 上仍渲染 BIGINT AUTO_INCREMENT。与 app/models.py 的 _BIG_PK 一致。
_BIG_PK = sa.BigInteger().with_variant(sa.Integer, "sqlite")


def upgrade() -> None:
    _create_factor_def()
    _create_selection_config()
    _reshape_factor_daily()
    _seed_factor_defs()
    _seed_selection_config()


def _create_factor_def() -> None:
    op.create_table(
        "quant_factor_def",
        sa.Column("id", _BIG_PK, primary_key=True, autoincrement=True, nullable=False),
        sa.Column("key", sa.String(64), nullable=False),
        sa.Column("name", sa.String(128), nullable=False),
        sa.Column("description", sa.String(512), nullable=False, server_default=""),
        sa.Column("category", sa.String(64), nullable=False, server_default=""),
        sa.Column("unit", sa.String(32), nullable=True),
        sa.Column("direction", sa.String(256), nullable=False, server_default=""),
        sa.Column("limits", sa.String(256), nullable=False, server_default=""),
        sa.Column("value_type", sa.String(16), nullable=False, server_default="number"),
        sa.Column("input_scale", sa.Float(), nullable=True),
        sa.Column("expression", sa.JSON(), nullable=False),
        sa.Column("expression_hash", sa.String(64), nullable=False, index=True),
        sa.Column("min_bars", sa.Integer(), nullable=False, server_default="1"),
        sa.Column("enabled", sa.Boolean(), nullable=False, server_default=sa.true()),
        sa.Column("is_system", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
        sa.UniqueConstraint("key", name="uq_factor_def_key"),
    )


def _create_selection_config() -> None:
    op.create_table(
        "quant_selection_config",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True, nullable=False),
        sa.Column("name", sa.String(64), nullable=False, server_default="default"),
        sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
        sa.Column("score_weights", sa.JSON(), nullable=False),
        sa.Column("vol_confirm", sa.JSON(), nullable=False),
        sa.Column("hard_filters", sa.JSON(), nullable=False),
        sa.Column("top_n", sa.Integer(), nullable=False, server_default="30"),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.func.now()),
    )


def _reshape_factor_daily() -> None:
    # 阶段 1:新增 JSON 列
    with op.batch_alter_table("quant_factor_daily") as batch:
        batch.add_column(sa.Column("values", sa.JSON(), nullable=True))

    # 阶段 2:把旧 7 列迁移到 JSON,按数据库方言分批/逐行处理
    conn = op.get_bind()
    dialect = conn.dialect.name
    columns = ("mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5", "ma20_slope", "amount_avg20")
    values_col = "`values`" if dialect == "mysql" else '"values"'

    if dialect == "mysql":
        # MySQL JSON_OBJECT 会把 NULL 存成 JSON null,不符合「稀疏存储」语义。
        # 先 JSON_OBJECT 全量填充,再对每列把 JSON null 的键用 JSON_REMOVE 删掉。
        fields = ", ".join(f"'{c}', {c}" for c in columns)
        min_id, max_id = conn.execute(
            sa.text("SELECT COALESCE(MIN(id), 0), COALESCE(MAX(id), 0) FROM quant_factor_daily")
        ).one()
        batch_size = 50000
        if min_id > 0 and max_id > 0:
            for start in range(min_id, max_id + 1, batch_size):
                end = min(start + batch_size - 1, max_id)
                conn.execute(
                    sa.text(
                        f"UPDATE quant_factor_daily SET {values_col} = JSON_OBJECT({fields}) "
                        "WHERE id BETWEEN :start AND :end"
                    ),
                    {"start": start, "end": end},
                )
            for col in columns:
                for start in range(min_id, max_id + 1, batch_size):
                    end = min(start + batch_size - 1, max_id)
                    conn.execute(
                        sa.text(
                            f"UPDATE quant_factor_daily SET {values_col} = "
                            f"JSON_REMOVE({values_col}, '$.{col}') "
                            f"WHERE id BETWEEN :start AND :end "
                            f"AND JSON_TYPE(JSON_EXTRACT({values_col}, '$.{col}')) = 'NULL'"
                        ),
                        {"start": start, "end": end},
                    )
    else:
        # SQLite:json_object 不跳过 NULL,测试库数据量小,逐行用 Python 构建稀疏 JSON。
        rows = conn.execute(
            sa.text(
                "SELECT id, mom20, mom60, rsi14, atr_pct, vol_ratio5, ma20_slope, amount_avg20 "
                "FROM quant_factor_daily"
            )
        ).fetchall()
        for row in rows:
            values = {c: row._mapping[c] for c in columns if row._mapping[c] is not None}
            values_json = json.dumps(values, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
            conn.execute(
                sa.text(f"UPDATE quant_factor_daily SET {values_col} = :values WHERE id = :id"),
                {"values": values_json, "id": row._mapping["id"]},
            )

    # 阶段 3:补齐空值为空对象,然后删掉旧列并把 values 改为 NOT NULL
    if dialect == "mysql":
        conn.execute(sa.text(f"UPDATE quant_factor_daily SET {values_col} = CAST('{{}}' AS JSON) WHERE {values_col} IS NULL"))
    else:
        conn.execute(sa.text(f"UPDATE quant_factor_daily SET {values_col} = '{{}}' WHERE {values_col} IS NULL"))

    with op.batch_alter_table("quant_factor_daily") as batch:
        for col in columns:
            batch.drop_column(col)
        batch.alter_column("values", existing_type=sa.JSON(), nullable=False)


def _seed_factor_defs() -> None:
    """系统预置 7 个日频因子;is_system=1 避免用户误删,enabled=1 默认启用。"""
    factor_def = sa.table(
        "quant_factor_def",
        sa.column("key", sa.String),
        sa.column("name", sa.String),
        sa.column("description", sa.String),
        sa.column("category", sa.String),
        sa.column("unit", sa.String),
        sa.column("direction", sa.String),
        sa.column("limits", sa.String),
        sa.column("value_type", sa.String),
        sa.column("input_scale", sa.Float),
        sa.column("expression", sa.JSON),
        sa.column("expression_hash", sa.String),
        sa.column("min_bars", sa.Integer),
        sa.column("enabled", sa.Boolean),
        sa.column("is_system", sa.Boolean),
    )
    now = sa.func.now()
    op.execute(factor_def.insert().values([
        {
            "key": "mom20",
            "name": "近20日涨跌幅",
            "description": "当前收盘价相对20个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "数值越高表示近期走势越强",
            "limits": "使用复权日线计算，仅描述过去20个交易日，不代表未来收益。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 20},
            "expression_hash": "92dadb2caeb4523b4d7298763ea2ac2ffb8690f07dfc3b8d3f7c3712473a3d93",
            "min_bars": 21,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "mom60",
            "name": "近60日涨跌幅",
            "description": "当前收盘价相对60个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "数值越高表示中期走势越强",
            "limits": "使用复权日线计算，短期反转时可能滞后。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 60},
            "expression_hash": "9cd7a0c98b501e1e104f899aa4d5dc30197276e393eed5f1634f50541dbce69b",
            "min_bars": 61,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "rsi14",
            "name": "近期强弱程度（RSI 14）",
            "description": "比较近14日上涨和下跌力度，取值通常为0至100。",
            "category": "趋势与动量",
            "unit": "0-100",
            "direction": "高值偏强、低值偏弱，不能简单等同买卖点",
            "limits": "强趋势中可长期处于高位或低位，应结合趋势和估值判断。",
            "value_type": "number",
            "input_scale": 1.0,
            "expression": {"op": "rsi", "input": {"op": "field", "name": "close"}, "window": 14},
            "expression_hash": "5e36305481c85e09ac34a265bf1f94be2a5c8f89d775a1c6eefc1566a5e966b4",
            "min_bars": 15,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "atr_pct",
            "name": "日常价格波动幅度",
            "description": "14日平均真实波幅占当前收盘价的比例。",
            "category": "风险与波动",
            "unit": "%",
            "direction": "数值越高表示价格波动通常越大",
            "limits": "反映历史波动，不预测方向；停牌或异常价格会影响口径。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {
                "op": "divide",
                "left": {
                    "op": "atr",
                    "high": {"op": "field", "name": "high"},
                    "low": {"op": "field", "name": "low"},
                    "close": {"op": "field", "name": "close"},
                    "window": 14,
                },
                "right": {"op": "field", "name": "close"},
            },
            "expression_hash": "1cceae6cd71ca38caef6398a424b8f8feee10e1722628cb7029a6abe7ff44d64",
            "min_bars": 15,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "vol_ratio5",
            "name": "成交量相对5日平均",
            "description": "当日成交量相对过去5日平均成交量的倍数。",
            "category": "成交与流动性",
            "unit": "倍",
            "direction": "大于1表示成交量高于近期平均",
            "limits": "日频近似量比，与行情软件的盘中量比口径不同。",
            "value_type": "number",
            "input_scale": 1.0,
            "expression": {
                "op": "volume_ratio",
                "input": {"op": "field", "name": "volume"},
                "window": 5,
                "shift": 1,
            },
            "expression_hash": "c9867e7be9f7e04e044733ce5a0f41f0a0a794af10f8a410c4d63d7bb7dba7b3",
            "min_bars": 6,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "ma20_slope",
            "name": "20日平均价格趋势",
            "description": "20日均线相对5个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "正值表示20日均线向上",
            "limits": "均线是滞后指标，快速转折时反应较慢。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {
                "op": "subtract",
                "left": {
                    "op": "divide",
                    "left": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
                    "right": {
                        "op": "shift",
                        "input": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
                        "periods": 5,
                    },
                },
                "right": {"op": "literal", "value": 1},
            },
            "expression_hash": "0a909dfc00af30c35fb9cb3c0341f69b2743216e3cf6a43ee140bfddd289751c",
            "min_bars": 25,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "amount_avg20",
            "name": "近20日日均成交额",
            "description": "近20个交易日成交额的平均值。",
            "category": "成交与流动性",
            "unit": "亿元",
            "direction": "数值越高通常表示交易更活跃",
            "limits": "成交活跃不等同于公司质量或价格上涨。",
            "value_type": "number",
            "input_scale": 100_000_000.0,
            "expression": {
                "op": "rolling_mean",
                "input": {"op": "field", "name": "amount"},
                "window": 20,
                "shift": 0,
            },
            "expression_hash": "5fdc2b2d77752d93bb91eeaa7b83a7fc29719f216702a5829451e6b2907a38d7",
            "min_bars": 20,
            "enabled": True,
            "is_system": True,
        },
    ]))

    # 只有首次插入时才需要更新时间戳;func.now() 已在上面设置,此处不重复。
    op.execute(
        sa.text(
            "UPDATE quant_factor_def SET updated_at = created_at"
        )
    )


def _seed_selection_config() -> None:
    """默认选股配置:与当前生产选股口径一致,作为配置化改造的起点。"""
    selection_config = sa.table(
        "quant_selection_config",
        sa.column("name", sa.String),
        sa.column("is_active", sa.Boolean),
        sa.column("score_weights", sa.JSON),
        sa.column("vol_confirm", sa.JSON),
        sa.column("hard_filters", sa.JSON),
        sa.column("top_n", sa.Integer),
    )
    op.execute(selection_config.insert().values([{
        "name": "default",
        "is_active": True,
        "score_weights": {"mom20": 0.5, "mom60": 0.3, "ma20_slope": 0.2},
        "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.05},
        "hard_filters": [
            {"type": "exclude_st"},
            {"type": "exclude_suspended"},
            {"type": "min_bars", "value": 120},
            {"type": "factor_gte", "factor": "amount_avg20", "value": 50000000},
            {"type": "row_flag", "field": "above_ma20", "value": True},
        ],
        "top_n": 30,
    }]))


def downgrade() -> None:
    # 1. 恢复 factor_daily 的 7 个 Float 列
    with op.batch_alter_table("quant_factor_daily") as batch:
        for col in ("mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5", "ma20_slope", "amount_avg20"):
            batch.add_column(sa.Column(col, sa.Float(), nullable=True))

    conn = op.get_bind()
    dialect = conn.dialect.name
    values_col = "`values`" if dialect == "mysql" else '"values"'

    for col in ("mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5", "ma20_slope", "amount_avg20"):
        if dialect == "mysql":
            expr = (
                f"CASE "
                f"WHEN JSON_TYPE(JSON_EXTRACT({values_col}, '$.{col}')) = 'NULL' THEN NULL "
                f"WHEN JSON_EXTRACT({values_col}, '$.{col}') IS NULL THEN NULL "
                f"ELSE CAST(JSON_UNQUOTE(JSON_EXTRACT({values_col}, '$.{col}')) AS FLOAT) "
                f"END"
            )
            conn.execute(sa.text(f"UPDATE quant_factor_daily SET `{col}` = {expr}"))
        else:
            conn.execute(sa.text(f"UPDATE quant_factor_daily SET {col} = json_extract({values_col}, '$.{col}')"))

    with op.batch_alter_table("quant_factor_daily") as batch:
        batch.drop_column("values")

    # 2. 删除本次新增表
    op.drop_table("quant_selection_config")
    op.drop_table("quant_factor_def")
