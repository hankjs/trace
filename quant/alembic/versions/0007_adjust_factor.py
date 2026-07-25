"""复权因子表:把「会被重写的前复权价」与「永不改写的事实」分离

## 为什么需要这张表

`quant_daily_bar` 一行里混了两种生命周期完全不同的数据:

| 列 | 语义 | 会不会被改写 |
|---|---|---|
| `raw_close` / `volume` / `amount` | 不复权事实 | **永不改写** |
| `open` / `high` / `low` / `close` | 前复权价 | **每次分红送转,全部历史被重写** |

后果是增量更新在原理上就不安全:拉最近 10 天 upsert,若这期间发生分红,
baostock 返回新基准下的前复权价,而库里 2015 年以来的历史仍是旧基准 ——
同一列跨两个尺度,产生假跳空(REVIEW §3.1;全市场回填实测检出 6 例)。

## 为什么存权威值而不是从 close/raw_close 反推

反推在数学上可行且已验证正确(sh.600519 的 16 个除权日与 baostock
`query_adjust_factor` 六位小数逐位吻合),但有一个根本盲区:**它只能反推出
库里已有的数据**。若某股历史本身已经错乱,反推出的因子会连同错误一起
继承,拿它当检测基准就成了循环论证。权威值是独立的第三方基准。

精度上也有差别:`close` 与 `raw_close` 都是 `DECIMAL(12,4)`,两个 4 位小数
相除只能得到约 4~5 位有效精度(实测因子在小数第 6 位抖动,那是舍入噪声),
而权威值是干净的 6 位小数。故本表用 `DECIMAL(16,6)`。

## 稀疏性

按除权日存储,不是每日一行:实测 `sh.600519` 的 2808 行日线只对应 16 个
除权日(压缩 175:1),抽样 200 只全市场估算约 4 万行。

Revision ID: 0007_adjust_factor
Revises: 0006_user_id_not_null
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0007_adjust_factor"
down_revision: str | None = "0006_user_id_not_null"
branch_labels = None
depends_on = None

# 与 app/models.py 的 _ADJ_FACTOR 保持一致。
_FACTOR = sa.Numeric(16, 6, asdecimal=False)


def upgrade() -> None:
    op.create_table(
        "quant_adjust_factor",
        sa.Column("code", sa.String(16), primary_key=True, nullable=False),
        # baostock 字段 dividOperateDate:除权除息日
        sa.Column("divid_operate_date", sa.Date(), primary_key=True,
                  nullable=False),
        sa.Column("fore_factor", _FACTOR, nullable=False),   # foreAdjustFactor
        sa.Column("back_factor", _FACTOR, nullable=True),    # backAdjustFactor
    )


def downgrade() -> None:
    op.drop_table("quant_adjust_factor")
