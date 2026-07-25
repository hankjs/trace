"""迁移共用工具:让同一套 revision 在 MySQL 与 sqlite 上都能正确执行。

价格/金额列在数据库侧是精确 DECIMAL,Python 侧仍取 float
(`asdecimal=False`,见 logs/decisions-migrate.md D3)。
"""
from __future__ import annotations

import sqlalchemy as sa


def money(precision: int, scale: int) -> sa.Numeric:
    return sa.Numeric(precision, scale, asdecimal=False)


PRICE = money(12, 4)       # 单价
SHARES = money(20, 2)      # 成交量 / 成交额
TRADE_QTY = money(18, 4)   # 手工账本数量与手续费
PCT = money(9, 4)          # 涨跌幅
EQUITY = money(18, 8)      # 回测净值
MARKET_CAP = money(20, 2)  # 总市值


def big_pk() -> sa.types.TypeEngine:
    """自增主键类型。

    MySQL 渲染 BIGINT AUTO_INCREMENT;sqlite 必须渲染 INTEGER ——
    sqlite 只把 "INTEGER PRIMARY KEY" 当作 rowid 别名并自增,
    写 BIGINT 会让插入时 id 拿不到自增值而触发 NOT NULL 失败。
    与 app/models.py 的 `_BIG_PK` 保持一致,否则 parity 校验会报不一致。
    """
    return sa.BigInteger().with_variant(sa.Integer, "sqlite")
