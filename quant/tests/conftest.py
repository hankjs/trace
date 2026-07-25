"""测试夹具:让 SQLite 能承载生产 schema 的自增主键。

`quant_daily_bar.id` 是 `BigInteger` 自增主键(`app/models.py`)。SQLite 只对
`INTEGER PRIMARY KEY` 启用 rowid 自增,`BIGINT` 会要求显式赋值 —— 于是
`upsert_bars`(不指定 id,依赖自增)在内存库里必然 NOT NULL 失败,重锚的
「新尺度覆盖旧行」行为就无法在测试中验证。

这里只在测试进程内把 SQLite 的 BIGINT 渲染成 INTEGER,不影响生产 MySQL。
agent-migrate 把 `quant_daily_bar` 改成 (code, date) 自然主键后本文件可删。
"""
from __future__ import annotations

import sys
from pathlib import Path

from sqlalchemy import BigInteger
from sqlalchemy.ext.compiler import compiles

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))


@compiles(BigInteger, "sqlite")
def _bigint_as_integer_on_sqlite(type_, compiler, **kw):  # noqa: ANN001
    return "INTEGER"
