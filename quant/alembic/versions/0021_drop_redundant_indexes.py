"""删除三个被唯一约束前缀覆盖的冗余单列索引。

Revision ID: 0021_drop_redundant_indexes
Revises: 0020_drop_stock_is_watch
Create Date: 2026-07-29

ix_quant_signal_code、ix_quant_index_member_index_name、
ix_quant_backtest_equity_run_id 分别被同表复合唯一约束的左前缀完全覆盖:
uq_signal(code,...)、uq_index_member(index_name,...)、
uq_bt_equity_run_date(run_id,date)。前缀查询走唯一索引即可,单列索引纯重复;
quant_signal / quant_backtest_equity 是千万行级大表,冗余索引带来实打实的
写入放大与存储成本。与 FactorDaily.code / Pick 的既有清理同理。

注意:存量生产库实际从未建出这三个索引(早期建库路径与 models 漂移),
因此 upgrade/downgrade 均按「索引存在才操作」执行,对缺失环境等价于只推进
版本号;新建库(含 parity 校验的临时 sqlite)从 0001 起会建出索引,再由此
revision 删除,两条路径终态一致。
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
from sqlalchemy import inspect

revision: str = "0021_drop_redundant_indexes"
down_revision: Union[str, None] = "0020_drop_stock_is_watch"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_TARGETS = [
    ("ix_quant_signal_code", "quant_signal", ["code"]),
    ("ix_quant_index_member_index_name", "quant_index_member", ["index_name"]),
    ("ix_quant_backtest_equity_run_id", "quant_backtest_equity", ["run_id"]),
]


def _existing_indexes(table: str) -> set[str]:
    return {ix["name"] for ix in inspect(op.get_bind()).get_indexes(table)}


def upgrade() -> None:
    for name, table, _cols in _TARGETS:
        if name in _existing_indexes(table):
            op.drop_index(name, table_name=table)


def downgrade() -> None:
    for name, table, cols in reversed(_TARGETS):
        if name not in _existing_indexes(table):
            op.create_index(name, table, cols)
