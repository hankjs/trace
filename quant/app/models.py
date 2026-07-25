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
    Numeric,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.orm import Mapped, mapped_column

from .db import Base


def _money(precision: int, scale: int) -> Numeric:
    """价格/金额列:数据库侧 DECIMAL(精确存储),Python 侧 float。

    MySQL `Float` 是单精度(约 7 位有效数字),positions.py 累加 `price*qty + fee`
    再除法求均价时,六位数持仓的精度损失超过展示用的 round(4)。改 DECIMAL 后
    存储不再截断。

    `asdecimal=False` 是刻意选择(见 logs/decisions-migrate.md D3):驱动仍返回
    float(float64,15~16 位有效数字),避免 Decimal 与 float 混算在下游
    ingest.py 重锚阈值 / positions.py / 回测 pandas / JSON 响应里抛 TypeError
    或改变响应格式。那些文件属 data 与 pool 的 scope,本次不动。
    """
    return Numeric(precision, scale, asdecimal=False)


_PRICE = _money(12, 4)      # 单价:A 股最小报价单位 0.01,前复权价留 4 位小数
_SHARES = _money(20, 2)     # 成交量/成交额:amount 可达千亿
_TRADE_QTY = _money(18, 4)  # 手工账本数量与手续费
_PCT = _money(9, 4)         # 涨跌幅
_EQUITY = _money(18, 8)     # 回测净值:累计乘除需高小数位
_MARKET_CAP = _money(20, 2)  # 总市值
# 复权因子:baostock 权威值给 6 位小数(如 0.792993 / 6.081667),
# 精度必须高于 _PRICE —— 用 close/raw_close 两个 4 位小数相除只能得到
# 约 4~5 位有效精度,那是反推的固有损失,权威值不该再被截断。
_ADJ_FACTOR = _money(16, 6)

# 自增主键:MySQL 上渲染 BIGINT AUTO_INCREMENT(全市场日频最终超 21 亿行,
# Integer 会溢出);sqlite 上必须渲染成 INTEGER —— sqlite 只把
# "INTEGER PRIMARY KEY" 当作 rowid 别名并自增,写 BIGINT 会让插入时
# id 拿不到自增值而触发 NOT NULL 失败。测试库是 sqlite,靠 variant 抹平差异。
_BIG_PK = BigInteger().with_variant(Integer, "sqlite")


class Stock(Base):
    """股票基础信息"""

    __tablename__ = "quant_stock"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)  # 如 sh.600519
    name: Mapped[str] = mapped_column(String(64), default="")
    industry: Mapped[str] = mapped_column(String(64), default="")
    is_watch: Mapped[bool] = mapped_column(Boolean, default=False)
    list_date: Mapped[date | None] = mapped_column(Date, nullable=True)
    delist_date: Mapped[date | None] = mapped_column(Date, nullable=True)
    is_st: Mapped[bool] = mapped_column(Boolean, default=False)


class WatchlistItem(Base):
    """用户自选股。股票资料共享，自选关系按共享 users.id 隔离。"""

    __tablename__ = "quant_watchlist"

    user_id: Mapped[str] = mapped_column(String(36), primary_key=True)
    code: Mapped[str] = mapped_column(String(16), primary_key=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class DailyBar(Base):
    """日线。open/high/low/close 为前复权价,raw_close 为不复权收盘价。

    自然主键 (code, date):行按 (code,date) 聚簇,区间扫描顺序命中。
    删去原代理自增 id、冗余的 ix_quant_daily_bar_code(与主键前缀重复)
    与 uq_daily_bar_code_date(换自然主键后与 PK 重复)。
    保留 ix_quant_daily_bar_date:跨股票按单日查询(选股/因子)需要。
    """

    __tablename__ = "quant_daily_bar"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)
    date: Mapped[date] = mapped_column(Date, primary_key=True, index=True)
    open: Mapped[float] = mapped_column(_PRICE)
    high: Mapped[float] = mapped_column(_PRICE)
    low: Mapped[float] = mapped_column(_PRICE)
    close: Mapped[float] = mapped_column(_PRICE)
    raw_close: Mapped[float | None] = mapped_column(_PRICE, nullable=True)
    volume: Mapped[float] = mapped_column(_SHARES, default=0)
    amount: Mapped[float] = mapped_column(_SHARES, default=0)


class AdjustFactor(Base):
    """复权因子(baostock query_adjust_factor 的权威值,只增不改)。

    为什么独立成表:`quant_daily_bar` 的 open/high/low/close 是**前复权价**,
    每次分红送转 baostock 会回溯重写全部历史,而 raw_close/volume/amount 是
    永不改写的事实。一张表里混了两种生命周期,增量更新在原理上就不安全
    (新尺度 bar 接到旧尺度历史 = 假跳空,REVIEW §3.1)。

    因子按除权日稀疏存储:实测 sh.600519 的 2808 行日线只对应 16 个除权日,
    全市场约 4 万行。

    为什么采集权威值而不是从 close/raw_close 反推:反推只能反推出库里**已有**
    的数据,若某股历史本身已错乱,反推的因子会连同错误一起继承,拿它当检测
    基准就是循环论证。权威值是独立的第三方基准。
    """

    __tablename__ = "quant_adjust_factor"

    code: Mapped[str] = mapped_column(String(16), primary_key=True)
    # baostock 字段名 dividOperateDate:除权除息日
    divid_operate_date: Mapped[date] = mapped_column(Date, primary_key=True)
    fore_factor: Mapped[float] = mapped_column(_ADJ_FACTOR)   # foreAdjustFactor
    back_factor: Mapped[float | None] = mapped_column(        # backAdjustFactor
        _ADJ_FACTOR, nullable=True)
    # 'baostock' = query_adjust_factor 的权威值;
    # 'sina' = 北交所自算(baostock 不覆盖北交所,见 alembic 0008)。
    # 自算值精度受 close/raw_close 的 DECIMAL(12,4) 限制,约 4~5 位有效,
    # 故审计时它的可信度低于权威值 —— 用 source 区分,不要混为一谈。
    source: Mapped[str] = mapped_column(String(16), default="baostock")


class Snapshot(Base):
    """盘中快照"""

    __tablename__ = "quant_snapshot"

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    ts: Mapped[datetime] = mapped_column(DateTime, index=True)
    price: Mapped[float] = mapped_column(_PRICE)
    pct_chg: Mapped[float | None] = mapped_column(_PCT, nullable=True)
    volume: Mapped[float | None] = mapped_column(_SHARES, nullable=True)
    amount: Mapped[float | None] = mapped_column(_SHARES, nullable=True)


class Signal(Base):
    """策略信号。side: buy / sell / watch,reason 为 JSON(触发原因明细)。"""

    __tablename__ = "quant_signal"
    __table_args__ = (
        UniqueConstraint("code", "date", "strategy", "side", name="uq_signal"),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    date: Mapped[date] = mapped_column(Date, index=True)
    strategy: Mapped[str] = mapped_column(String(64), index=True)
    side: Mapped[str] = mapped_column(String(8))
    price: Mapped[float | None] = mapped_column(_PRICE, nullable=True)
    reason: Mapped[dict | None] = mapped_column(JSON, nullable=True)


class Trade(Base):
    """手工录入的成交记录(本系统不做自动交易)"""

    __tablename__ = "quant_trade"

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    user_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    trade_date: Mapped[date] = mapped_column(Date)
    side: Mapped[str] = mapped_column(String(8))  # buy / sell
    price: Mapped[float] = mapped_column(_PRICE)
    qty: Mapped[float] = mapped_column(_TRADE_QTY)
    fee: Mapped[float] = mapped_column(_TRADE_QTY, default=0)
    note: Mapped[str] = mapped_column(Text, default="")


class BacktestRun(Base):
    """回测任务"""

    __tablename__ = "quant_backtest_run"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    user_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    strategy: Mapped[str] = mapped_column(String(64))
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    costs: Mapped[dict | None] = mapped_column(JSON, nullable=True)  # 回测复现:固化当时费率
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)  # 回测所用股票池
    codes: Mapped[list | None] = mapped_column(JSON, nullable=True)
    start: Mapped[date] = mapped_column(Date)
    end: Mapped[date] = mapped_column(Date)
    metrics: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class IndexMember(Base):
    """指数成分股名录。out_date 为 NULL 表示当前在册。"""

    __tablename__ = "quant_index_member"
    __table_args__ = (
        UniqueConstraint("index_name", "code", "in_date", name="uq_index_member"),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    index_name: Mapped[str] = mapped_column(String(16), index=True)  # hs300 / zz500
    code: Mapped[str] = mapped_column(String(16), index=True)
    in_date: Mapped[date] = mapped_column(Date)
    out_date: Mapped[date | None] = mapped_column(Date, nullable=True)


class FactorDaily(Base):
    """每日因子值(股票池向量化计算,供选股/筛选用)"""

    __tablename__ = "quant_factor_daily"
    __table_args__ = (UniqueConstraint("code", "date", name="uq_factor_code_date"),)

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    # code 不再单独建索引:与 uq_factor_code_date(code,date) 前缀完全冗余
    code: Mapped[str] = mapped_column(String(16))
    date: Mapped[date] = mapped_column(Date, index=True)
    mom20: Mapped[float | None] = mapped_column(Float, nullable=True)
    mom60: Mapped[float | None] = mapped_column(Float, nullable=True)
    rsi14: Mapped[float | None] = mapped_column(Float, nullable=True)
    atr_pct: Mapped[float | None] = mapped_column(Float, nullable=True)
    vol_ratio5: Mapped[float | None] = mapped_column(Float, nullable=True)
    ma20_slope: Mapped[float | None] = mapped_column(Float, nullable=True)
    amount_avg20: Mapped[float | None] = mapped_column(Float, nullable=True)


class ValuationSnapshot(Base):
    """每日估值快照。available_date 防止历史研究读取未来数据。"""

    __tablename__ = "quant_valuation_snapshot"
    __table_args__ = (
        UniqueConstraint(
            "code", "data_date", "available_date",
            name="uq_valuation_code_date_available",
        ),
    )

    # 全市场日频最终会超 21 亿行,Integer 主键会溢出(REVIEW 五)
    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    data_date: Mapped[date] = mapped_column(Date, index=True)
    report_period: Mapped[date | None] = mapped_column(Date, nullable=True)
    available_date: Mapped[date] = mapped_column(Date, index=True)
    source: Mapped[str] = mapped_column(String(96))
    pe_ttm: Mapped[float | None] = mapped_column(Float, nullable=True)
    pb: Mapped[float | None] = mapped_column(Float, nullable=True)
    ps_ttm: Mapped[float | None] = mapped_column(Float, nullable=True)
    dividend_yield: Mapped[float | None] = mapped_column(Float, nullable=True)
    total_market_cap: Mapped[float | None] = mapped_column(_MARKET_CAP, nullable=True)


class FundamentalSnapshot(Base):
    """财务报告版本。修订值仅从其 available_date 起参与研究。"""

    __tablename__ = "quant_fundamental_snapshot"
    __table_args__ = (
        UniqueConstraint(
            "code", "report_period", "available_date",
            name="uq_fundamental_code_period_available",
        ),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    data_date: Mapped[date] = mapped_column(Date, index=True)
    report_period: Mapped[date] = mapped_column(Date, index=True)
    available_date: Mapped[date] = mapped_column(Date, index=True)
    source: Mapped[str] = mapped_column(String(96))
    roe: Mapped[float | None] = mapped_column(Float, nullable=True)
    revenue_yoy: Mapped[float | None] = mapped_column(Float, nullable=True)
    profit_yoy: Mapped[float | None] = mapped_column(Float, nullable=True)
    gross_margin: Mapped[float | None] = mapped_column(Float, nullable=True)
    net_margin: Mapped[float | None] = mapped_column(Float, nullable=True)
    debt_ratio: Mapped[float | None] = mapped_column(Float, nullable=True)
    cashflow_ratio: Mapped[float | None] = mapped_column(Float, nullable=True)


class Pick(Base):
    """每日选股池(Top N)。factors 为当日因子快照 JSON。"""

    __tablename__ = "quant_pick"
    __table_args__ = (UniqueConstraint("date", "code", name="uq_pick_date_code"),)

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    # date 不再单独建索引:与 uq_pick_date_code(date,code) 前缀完全冗余
    date: Mapped[date] = mapped_column(Date)
    code: Mapped[str] = mapped_column(String(16), index=True)
    score: Mapped[float] = mapped_column(Float)
    rank: Mapped[int] = mapped_column(Integer)
    factors: Mapped[dict | None] = mapped_column(JSON, nullable=True)


class StrategyEval(Base):
    """策略批量评估结果。scope: single:xxx / pool_top50 / pool。"""

    __tablename__ = "quant_strategy_eval"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    strategy: Mapped[str] = mapped_column(String(64), index=True)
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    scope: Mapped[str] = mapped_column(String(64), index=True)
    batch_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)  # 评估所用股票池
    start: Mapped[date] = mapped_column(Date)
    end: Mapped[date] = mapped_column(Date)
    metrics: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    run_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now, index=True)


class BacktestEquity(Base):
    """回测净值曲线"""

    __tablename__ = "quant_backtest_equity"
    __table_args__ = (
        UniqueConstraint("run_id", "date", name="uq_bt_equity_run_date"),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    run_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_backtest_run.id"), index=True
    )
    date: Mapped[date] = mapped_column(Date)
    equity: Mapped[float] = mapped_column(_EQUITY)


class Pool(Base):
    """股票池定义。kind: index(动态查指数成分)/ all(全市场按上市退市ST过滤)/ static(直查成员)。

    user_id NULL 表示系统级预置池;非空则按共享 users.id 隔离。
    """

    __tablename__ = "quant_pool"
    __table_args__ = (UniqueConstraint("user_id", "name", name="uq_pool_user_name"),)

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    kind: Mapped[str] = mapped_column(String(16), index=True)  # index / all / static
    ref: Mapped[str | None] = mapped_column(String(32), nullable=True)  # 如 hs300_zz500
    # NULL 是有意义的:表示系统级预置池,全用户共享。故此列不随
    # quant_trade/quant_backtest_run 的 user_id 一起收紧为 NOT NULL。
    user_id: Mapped[str | None] = mapped_column(String(36), nullable=True, index=True)
    name: Mapped[str] = mapped_column(String(64), default="")
    min_list_days: Mapped[int] = mapped_column(Integer, default=60)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class PoolMember(Base):
    """静态池成员。只存代码,无日期(已定,带幸存者偏差由调用方知情)。"""

    __tablename__ = "quant_pool_member"

    pool_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_pool.id"), primary_key=True
    )
    code: Mapped[str] = mapped_column(String(16), primary_key=True)


class TradeCalendar(Base):
    """交易日历。采集逻辑在 app/data/trade_calendar.py。"""

    __tablename__ = "quant_trade_calendar"

    date: Mapped[date] = mapped_column(Date, primary_key=True)
    is_open: Mapped[bool] = mapped_column(Boolean, default=True)
    source: Mapped[str] = mapped_column(String(16), default="baostock")
