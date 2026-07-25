"""schema 兼容层(临时):在 agent-migrate 的新列/新表落地前,让采集逻辑可用。

背景:`quant_stock.list_date/delist_date/is_st` 与 `quant_trade_calendar`
由 agent-migrate 负责在 `app/models.py` 中定义(见 `logs/notify-migrate.md`),
本 scope 不得改 models.py。为了不被阻塞,这里在运行时补上等价映射:

- 若 `app/models.py` 已有对应列/模型 → 本模块什么都不做(纯 no-op);
- 若还没有 → 用 SQLAlchemy 的 `add_mapped_attribute` / 一次性声明补齐,
  列定义与 `logs/notify-migrate.md` 中约定的完全一致。

migrate 落地后本文件可整体删除,调用方无需改动(它们只 import 名字)。
"""
from __future__ import annotations

from datetime import date

from sqlalchemy import Boolean, Column, Date, String
from sqlalchemy.orm import Mapped, add_mapped_attribute, mapped_column

from .. import models
from ..db import Base

# --- quant_stock 生命周期列 ---------------------------------------------------

_STOCK_LIFECYCLE_COLUMNS = {
    "list_date": lambda: Column("list_date", Date, nullable=True),
    "delist_date": lambda: Column("delist_date", Date, nullable=True),
    "is_st": lambda: Column("is_st", Boolean, nullable=False, default=False,
                            server_default="0"),
}


def ensure_stock_lifecycle_columns() -> list[str]:
    """确保 Stock 有 list_date / delist_date / is_st,返回本次补充的列名。"""
    added = []
    for name, factory in _STOCK_LIFECYCLE_COLUMNS.items():
        if name not in models.Stock.__table__.c:
            add_mapped_attribute(models.Stock, name, factory())
            added.append(name)
    return added


ensure_stock_lifecycle_columns()


# --- quant_trade_calendar ----------------------------------------------------

TradeCalendar = getattr(models, "TradeCalendar", None)

if TradeCalendar is None:  # pragma: no branch - migrate 落地后走 else
    class TradeCalendar(Base):  # type: ignore[no-redef]
        """交易日历。migrate 落地同名模型后本定义自动让位。"""

        __tablename__ = "quant_trade_calendar"

        date: Mapped[date] = mapped_column(Date, primary_key=True)
        is_open: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)
        source: Mapped[str] = mapped_column(String(16), default="baostock")

    models.TradeCalendar = TradeCalendar  # 让后续 import 走同一个类
