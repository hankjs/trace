"""策略从代码常量改为可管理的表:新增 quant_strategy,三张表改 strategy_id 外键

## 为什么改

策略此前只存在于代码里:算法在 `app/strategy/strategies/*.py`,元数据在
`app/catalog.py` 的 `STRATEGIES` 字典,两者靠 `NAME` 字符串对齐。用户能选策略、
能在回测页临时改参数,但**改完的参数无处安放** —— 关掉页面就没了,更不可能让
夜间信号引擎按用户自己的参数出信号。

要让用户保存自己的参数组合,策略就必须是数据行。同时现有 6 个策略成为全用户
共享的公共策略,归属模型与股票池完全一致(`is_system` + `owner_id`,见 0011)。

## 新结构

```
quant_strategy(id, owner_id NOT NULL, is_system, name, template, kind,
               params JSON, enabled, created_at)
UNIQUE(owner_id, name)
```

`template` 指向算法模块(ma_cross / breakout / ...),`params` 只存用户显式
覆盖的键。算法逻辑仍在代码里 —— 这一步只是把「参数组合」变成数据。后续的
规则构建器沿用同一张表:新增 `template='rule'` 加一列 `rules JSON`。

**不建 grant 表。** 当前需求只有「公共」和「我的」两档,`is_system OR
owner_id = 我` 就够。`quant_pool_grant` 那样的定向分享等真有需求再补,不预先
建一张没人写的表。系统策略同样归哨兵 UUID
`00000000-0000-0000-0000-000000000000`,理由见 0011。

## 三张表的 strategy 字符串列改外键

`quant_signal` / `quant_backtest_run` / `quant_strategy_eval` 原先各存一个
`strategy VARCHAR(64)`。保留字符串列 + 新增 `strategy_id` 会有两个真相,
且「同名不同参数的两个用户策略」根本无法用字符串区分,故字符串列删除。

回填按 `template` 对齐到 6 条系统策略。**匹配不上的行直接删除**:库中历史
数据经核实全是开发期测试数据(见 0009 与 tests/test_schema.py 的记录),没有
要兼容的生产数据;置空则违反 NOT NULL,保留字符串列又回到两个真相。删除条数
打印到迁移日志。

两种 ON DELETE 刻意不同:

- signal / strategy_eval 用 **CASCADE** —— 定时任务的派生数据,下一轮重算;
- backtest_run 用 **RESTRICT** —— 用户主动发起、要求可复现审计的记录,不能
  因为删了策略就静默消失。API 在删除仍被回测引用的策略时返回 409,引导用户
  改用「停用」(`enabled=0`)。

`uq_signal` 随之从 `(code, date, strategy, side)` 重建为
`(code, date, strategy_id, side)`。

Revision ID: 0012_strategy_table
Revises: 0011_pool_owner_and_grant
"""
from __future__ import annotations

import logging

import sqlalchemy as sa
from alembic import op

revision: str = "0012_strategy_table"
down_revision: str | None = "0011_pool_owner_and_grant"
branch_labels = None
depends_on = None

logger = logging.getLogger("alembic.runtime.migration")

SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"

# 预置策略 = 现有 6 个代码模板各一条,params 留空表示全用模板默认值。
# id 写死:与 0005 的预置池同样的理由 —— 前端与文档要能引用稳定编号。
# name/kind 与 app/catalog.py 的 STRATEGY_TEMPLATES、模块的 KIND 一致,
# tests/test_catalog.py 交叉校验二者不漂移。
PRESET_STRATEGIES = [
    (1, "ma_cross", "single", "双均线趋势策略"),
    (2, "breakout", "single", "价格突破策略"),
    (3, "mean_reversion", "single", "上升趋势中的超跌反弹策略"),
    (4, "volume_breakout", "single", "缩量整理后的放量突破策略"),
    (5, "momentum_rotation", "portfolio", "强势股票轮动策略"),
    (6, "multifactor_hold", "portfolio", "多指标综合评分持有策略"),
]

# 三张表的 (表名, ON DELETE 行为, 旧 strategy 列是否有索引)。
# ON DELETE 的取舍见模块文档字符串;backtest_run.strategy 从来没建过索引
# (只按 user_id 查历史回测),降级时也不该凭空补一个。
_REFERRING_TABLES = [
    ("quant_signal", "CASCADE", True),
    ("quant_backtest_run", "RESTRICT", False),
    ("quant_strategy_eval", "CASCADE", True),
]


def upgrade() -> None:
    op.create_table(
        "quant_strategy",
        sa.Column("id", sa.Integer(), primary_key=True,
                  autoincrement=True, nullable=False),
        sa.Column("owner_id", sa.String(36), nullable=False),
        sa.Column("is_system", sa.Boolean(), nullable=False,
                  server_default=sa.false()),
        sa.Column("name", sa.String(64), nullable=False),
        sa.Column("template", sa.String(32), nullable=False),
        sa.Column("kind", sa.String(16), nullable=False),
        sa.Column("params", sa.JSON(), nullable=True),
        sa.Column("enabled", sa.Boolean(), nullable=False,
                  server_default=sa.true()),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.UniqueConstraint("owner_id", "name", name="uq_strategy_owner_name"),
    )
    op.create_index("ix_quant_strategy_owner_id", "quant_strategy", ["owner_id"])
    op.create_index("ix_quant_strategy_is_system", "quant_strategy", ["is_system"])
    op.create_index("ix_quant_strategy_template", "quant_strategy", ["template"])
    op.create_index("ix_quant_strategy_kind", "quant_strategy", ["kind"])
    op.create_index("ix_quant_strategy_enabled", "quant_strategy", ["enabled"])

    strategy = sa.table(
        "quant_strategy",
        sa.column("id", sa.Integer),
        sa.column("owner_id", sa.String),
        sa.column("is_system", sa.Boolean),
        sa.column("name", sa.String),
        sa.column("template", sa.String),
        sa.column("kind", sa.String),
        sa.column("params", sa.JSON),
        sa.column("enabled", sa.Boolean),
        sa.column("created_at", sa.DateTime),
    )
    op.execute(strategy.insert().values([
        {"id": sid, "owner_id": SYSTEM_OWNER_ID, "is_system": True,
         "name": name, "template": template, "kind": kind,
         "params": {}, "enabled": True, "created_at": sa.func.now()}
        for sid, template, kind, name in PRESET_STRATEGIES
    ]))

    for table, ondelete, has_index in _REFERRING_TABLES:
        _migrate_referring_table(table, ondelete, has_index)

    _rebuild_signal_unique_constraint()


def _migrate_referring_table(table: str, ondelete: str, has_index: bool) -> None:
    """strategy 字符串列 -> strategy_id 外键:加列、回填、删孤儿行、收紧。"""
    with op.batch_alter_table(table) as batch:
        batch.add_column(sa.Column("strategy_id", sa.Integer(), nullable=True))

    # 相关子查询而不是 UPDATE...JOIN:后者 sqlite 不支持,测试库跑不过
    op.execute(sa.text(
        f"UPDATE {table} SET strategy_id = ("
        "  SELECT s.id FROM quant_strategy s"
        f" WHERE s.is_system = 1 AND s.template = {table}.strategy)"
    ))

    # 回填不到的是废弃策略名的历史行(全为开发期测试数据),删掉并记录条数
    result = op.get_bind().execute(
        sa.text(f"DELETE FROM {table} WHERE strategy_id IS NULL"))
    if result.rowcount:
        logger.warning(
            "0012: %s 删除 %d 行无法对齐 quant_strategy 的历史数据",
            table, result.rowcount)

    with op.batch_alter_table(table) as batch:
        batch.alter_column("strategy_id", existing_type=sa.Integer(),
                           nullable=False)
        batch.create_index(f"ix_{table}_strategy_id", ["strategy_id"])
        batch.create_foreign_key(
            f"fk_{table}_strategy_id", "quant_strategy",
            ["strategy_id"], ["id"], ondelete=ondelete)
        if table != "quant_signal":
            # signal 的 strategy 列要等唯一约束重建后才能删(约束含该列)
            if has_index:
                batch.drop_index(f"ix_{table}_strategy")
            batch.drop_column("strategy")


def _rebuild_signal_unique_constraint() -> None:
    """uq_signal: (code, date, strategy, side) -> (code, date, strategy_id, side)"""
    with op.batch_alter_table("quant_signal") as batch:
        batch.drop_constraint("uq_signal", type_="unique")
        batch.create_unique_constraint(
            "uq_signal", ["code", "date", "strategy_id", "side"])
        batch.drop_index("ix_quant_signal_strategy")
        batch.drop_column("strategy")


def downgrade() -> None:
    """反向回填 template 字符串。

    只有系统策略能还原成有意义的字符串列(其 template 就是旧的 strategy 值);
    用户自建策略的行没有对应的旧字符串,回填成 `template` 后会与同模板的系统
    策略混同 —— 这是降级不可避免的信息损失,不是 bug。
    """
    with op.batch_alter_table("quant_signal") as batch:
        batch.add_column(sa.Column("strategy", sa.String(64), nullable=True))
        batch.drop_constraint("uq_signal", type_="unique")

    for table, _ondelete, has_index in _REFERRING_TABLES:
        if table != "quant_signal":
            with op.batch_alter_table(table) as batch:
                batch.add_column(
                    sa.Column("strategy", sa.String(64), nullable=True))

        op.execute(sa.text(
            f"UPDATE {table} SET strategy = ("
            "  SELECT s.template FROM quant_strategy s"
            f" WHERE s.id = {table}.strategy_id)"
        ))
        with op.batch_alter_table(table) as batch:
            batch.alter_column("strategy", existing_type=sa.String(64),
                               nullable=False)
            if has_index:
                batch.create_index(f"ix_{table}_strategy", ["strategy"])
            batch.drop_constraint(f"fk_{table}_strategy_id", type_="foreignkey")
            batch.drop_index(f"ix_{table}_strategy_id")
            batch.drop_column("strategy_id")

    with op.batch_alter_table("quant_signal") as batch:
        batch.create_unique_constraint(
            "uq_signal", ["code", "date", "strategy", "side"])

    for index in ("enabled", "kind", "template", "is_system", "owner_id"):
        op.drop_index(f"ix_quant_strategy_{index}", table_name="quant_strategy")
    op.drop_table("quant_strategy")
