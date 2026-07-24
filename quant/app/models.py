"""数据库模型。所有表统一 quant_ 前缀,与 server 的表完全隔离。"""
from __future__ import annotations

from datetime import date, datetime

from sqlalchemy import (
    JSON,
    BigInteger,
    Boolean,
    Date,
    DateTime,
    Float,
    ForeignKey,
    Integer,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.orm import Mapped, mapped_column

from .db import Base


class Stock(Base):
    """股票基础信息"""

    __tablename__ = "quant_stock"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)  # 如 sh.600519
    name: Mapped[str] = mapped_column(String(64), default="")
    industry: Mapped[str] = mapped_column(String(64), default="")
    is_watch: Mapped[bool] = mapped_column(Boolean, default=False)


class DailyBar(Base):
    """日线。open/high/low/close 为前复权价,raw_close 为不复权收盘价。"""

    __tablename__ = "quant_daily_bar"
    __table_args__ = (UniqueConstraint("code", "date", name="uq_daily_bar_code_date"),)

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    date: Mapped[date] = mapped_column(Date, index=True)
    open: Mapped[float] = mapped_column(Float)
    high: Mapped[float] = mapped_column(Float)
    low: Mapped[float] = mapped_column(Float)
    close: Mapped[float] = mapped_column(Float)
    raw_close: Mapped[float | None] = mapped_column(Float, nullable=True)
    volume: Mapped[float] = mapped_column(Float, default=0)
    amount: Mapped[float] = mapped_column(Float, default=0)


class Snapshot(Base):
    """盘中快照"""

    __tablename__ = "quant_snapshot"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    ts: Mapped[datetime] = mapped_column(DateTime, index=True)
    price: Mapped[float] = mapped_column(Float)
    pct_chg: Mapped[float | None] = mapped_column(Float, nullable=True)
    volume: Mapped[float | None] = mapped_column(Float, nullable=True)
    amount: Mapped[float | None] = mapped_column(Float, nullable=True)


class Signal(Base):
    """策略信号。side: buy / sell / watch,reason 为 JSON(触发原因明细)。"""

    __tablename__ = "quant_signal"
    __table_args__ = (
        UniqueConstraint("code", "date", "strategy", "side", name="uq_signal"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    date: Mapped[date] = mapped_column(Date, index=True)
    strategy: Mapped[str] = mapped_column(String(64), index=True)
    side: Mapped[str] = mapped_column(String(8))
    price: Mapped[float | None] = mapped_column(Float, nullable=True)
    reason: Mapped[dict | None] = mapped_column(JSON, nullable=True)


class Trade(Base):
    """手工录入的成交记录(本系统不做自动交易)"""

    __tablename__ = "quant_trade"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    trade_date: Mapped[date] = mapped_column(Date)
    side: Mapped[str] = mapped_column(String(8))  # buy / sell
    price: Mapped[float] = mapped_column(Float)
    qty: Mapped[float] = mapped_column(Float)
    fee: Mapped[float] = mapped_column(Float, default=0)
    note: Mapped[str] = mapped_column(Text, default="")


class BacktestRun(Base):
    """回测任务"""

    __tablename__ = "quant_backtest_run"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    strategy: Mapped[str] = mapped_column(String(64))
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    codes: Mapped[list | None] = mapped_column(JSON, nullable=True)
    start: Mapped[date] = mapped_column(Date)
    end: Mapped[date] = mapped_column(Date)
    metrics: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class BacktestEquity(Base):
    """回测净值曲线"""

    __tablename__ = "quant_backtest_equity"
    __table_args__ = (
        UniqueConstraint("run_id", "date", name="uq_bt_equity_run_date"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    run_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_backtest_run.id"), index=True
    )
    date: Mapped[date] = mapped_column(Date)
    equity: Mapped[float] = mapped_column(Float)
