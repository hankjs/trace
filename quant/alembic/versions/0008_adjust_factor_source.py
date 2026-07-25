"""复权因子加 source 列:区分 baostock 权威值与北交所自算值

## 背景:baostock 不覆盖北交所

`quant_stock` 里有 330 只北交所股票(`bj.9200xx` / `bj.430xxx` / `bj.830xxx`),
来自 akshare `stock_info_a_code_name()`,是真实在交易的标的
(如 `bj.920000` 安徽凤凰 2026-07-24 收盘 14.18)。但 baostock 完全没有它们:

- `bj.` 前缀 → `10004011 股票代码未标识sh或sz`
- 换成 `sh.920000` / `sz.920000` → 参数校验通过,但**返回 0 行**

所以全市场因子采集时这 330 只必然失败(实测 660 次失败日志)。

## 补数来源与因子自算

新浪源 `ak.stock_zh_a_daily(symbol='bj920000', adjust=...)` 可用,且**同时提供
不复权与前复权**,因子可交叉算出:

    bj920000 首行 不复权 9.23 / 前复权 8.70 → 因子 0.942579
    bj920000 末行 不复权 14.18 / 前复权 14.18 → 因子 1.0

末行因子为 1.0 说明它与 baostock 的复权口径一致(都以最新日为基准的前复权),
所以两个来源的数据放同一张表不会混口径。

## 为什么要 source 列

自算因子的精度受 `close`/`raw_close` 的 `DECIMAL(12,4)` 限制,只有约 4~5 位
有效数字(两个 4 位小数相除),而 baostock 权威值是干净的 6 位小数。审计时
两者可信度不同,必须能区分 —— 否则「全库尺度与权威因子一致」这个结论会被
悄悄稀释成「一致或自证一致」。

已有 41222 行默认标记为 'baostock'(它们确实全部来自 query_adjust_factor)。

Revision ID: 0008_adjust_factor_source
Revises: 0007_adjust_factor
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0008_adjust_factor_source"
down_revision: str | None = "0007_adjust_factor"
branch_labels = None
depends_on = None


def upgrade() -> None:
    with op.batch_alter_table("quant_adjust_factor") as batch:
        batch.add_column(sa.Column(
            "source", sa.String(16), nullable=False,
            server_default="baostock"))


def downgrade() -> None:
    with op.batch_alter_table("quant_adjust_factor") as batch:
        batch.drop_column("source")
