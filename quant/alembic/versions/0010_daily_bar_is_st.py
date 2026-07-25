"""日线增加 is_st:回测的 ST 口径必须逐日,不能用当前状态

## 为什么需要这一列

`quant_stock.is_st` 只有**当前状态**——每只股票一行,改名为 `*ST` 会覆盖旧值。
用它过滤历史样本是系统性前视偏差:你用「今天知道谁变差了」去筛研究日当时的
候选池。

实测抽样 8 只当前 ST 股(各 2808 行,2015 至今共 22464 个交易日):

| 股票 | 真正 ST 的交易日 | 首次 ST |
|---|---|---|
| sh.600053 *ST九鼎 | 58 天(2%) | 2026-04-30 |
| sh.600082 ST海泰 | 70 天(2%) | 2026-04-14 |
| sh.600107 *ST尔雅 | 299 天(11%) | 2025-05-06 |
| sh.600136 ST明诚 | 1025 天(37%) | 2022-05-06 |
| **合计** | **3233/22464 = 14.4%** | |

也就是 **85.6% 的交易日被当前标记错误剔除**。`sh.600053` 只有 2% 的时间是 ST,
但它 2015-2026 的全部 2808 行都进不了回测样本。

偏差方向是**高估策略表现**:被剔掉的恰是后来才出问题的公司,而研究日当时它们
看起来完全正常、会被策略正常选中。这与 `engine.py` 的提前建仓、
`fundamental_snapshot` 缺 `available_date` 是同一类错误,只是入口不同。

## 为什么存在日线表而不是单独的区间表

`isST` 本就是 baostock 日线接口(`query_history_k_data_plus`)的字段,拆成
`(code, in_date, out_date)` 区间表反而要额外维护区间提取逻辑。且现有采集对每只
股票已经拉两次(前复权 + 不复权),把 `isST` 加进 `fields` 是**零额外请求**。

`quant_stock.is_st` 保留,但语义收窄为「仅供 UI 展示的当前状态」。

## 回填

新列对既有 1138 万行为 NULL,需一次全量重拉补齐(约 2 小时)。NULL 的含义是
「未采集」而非「非 ST」,过滤时应显式区分,不能把 NULL 当 False 用。

Revision ID: 0010_daily_bar_is_st
Revises: 0009_tighten_not_null
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision: str = "0010_daily_bar_is_st"
down_revision: str | None = "0009_tighten_not_null"
branch_labels = None
depends_on = None


def upgrade() -> None:
    with op.batch_alter_table("quant_daily_bar") as batch:
        # 可空:既有行未采集过 isST,NULL 表示「未知」而非「非 ST」
        batch.add_column(sa.Column("is_st", sa.Boolean(), nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("quant_daily_bar") as batch:
        batch.drop_column("is_st")
