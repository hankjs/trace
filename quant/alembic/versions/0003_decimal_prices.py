"""价格/金额列 Float -> DECIMAL

MySQL Float 是单精度(约 7 位有效数字),positions.py 累加 price*qty + fee 再
除法求均价,六位数持仓的精度损失超过展示用的 round(4)(REVIEW 第五节)。
逐列精度见 logs/decisions-migrate.md D4。

比率/因子列(quant_factor_daily 全部、pe_ttm/pb/ps_ttm/dividend_yield、
quant_fundamental_snapshot 全部、quant_pick.score)刻意保持 Float:
无累加精度需求,改 DECIMAL 只添麻烦。

Python 侧仍取 float(Numeric(asdecimal=False),见 D3),
故 ingest.py 的重锚阈值、positions.py 的混算、回测 pandas、JSON 响应格式
均不受影响 —— 那些文件属 data / pool 的 scope。

Revision ID: 0003_decimal_prices
Revises: 0002_user_id_uuid
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0003_decimal_prices"
down_revision: str | None = "0002_user_id_uuid"
branch_labels = None
depends_on = None


def _money(precision: int, scale: int) -> sa.Numeric:
    return sa.Numeric(precision, scale, asdecimal=False)


PRICE = _money(12, 4)
SHARES = _money(20, 2)
TRADE_QTY = _money(18, 4)
PCT = _money(9, 4)
EQUITY = _money(18, 8)
MARKET_CAP = _money(20, 2)

# (表, 列, 目标类型, nullable)
_COLUMNS = (
    ("quant_daily_bar", "open", PRICE, False),
    ("quant_daily_bar", "high", PRICE, False),
    ("quant_daily_bar", "low", PRICE, False),
    ("quant_daily_bar", "close", PRICE, False),
    ("quant_daily_bar", "raw_close", PRICE, True),
    ("quant_daily_bar", "volume", SHARES, False),
    ("quant_daily_bar", "amount", SHARES, False),
    ("quant_snapshot", "price", PRICE, False),
    ("quant_snapshot", "pct_chg", PCT, True),
    ("quant_snapshot", "volume", SHARES, True),
    ("quant_snapshot", "amount", SHARES, True),
    ("quant_signal", "price", PRICE, True),
    ("quant_trade", "price", PRICE, False),
    ("quant_trade", "qty", TRADE_QTY, False),
    ("quant_trade", "fee", TRADE_QTY, False),
    ("quant_valuation_snapshot", "total_market_cap", MARKET_CAP, True),
    ("quant_backtest_equity", "equity", EQUITY, False),
)


def upgrade() -> None:
    for table, column, target, nullable in _COLUMNS:
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                column,
                existing_type=sa.Float(),
                type_=target,
                existing_nullable=nullable,
            )


def downgrade() -> None:
    for table, column, target, nullable in reversed(_COLUMNS):
        with op.batch_alter_table(table) as batch:
            batch.alter_column(
                column,
                existing_type=target,
                type_=sa.Float(),
                existing_nullable=nullable,
            )
