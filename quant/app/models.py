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
    event,
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
_WEIGHT = _money(12, 8)     # 组合目标权重:保留足够精度使权重和可审计
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
    # 当日是否风险警示股(baostock 日线的 isST 字段,逐日真实历史)。
    #
    # 这是**回测唯一应当使用的 ST 口径**。quant_stock.is_st 只有当前状态、
    # 会被改名覆盖,用它过滤历史样本是系统性前视偏差:实测抽样 8 只当前 ST 股,
    # 22464 个交易日里真正处于 ST 的只有 14.4%,其余 85.6% 会被错误剔除 ——
    # 而被剔掉的恰是后来才出问题的公司,等于让策略提前知道谁将退化,
    # 方向上高估策略表现。
    is_st: Mapped[bool | None] = mapped_column(Boolean, nullable=True)


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
    """策略信号。side: buy / sell / watch,reason 为 JSON(触发原因明细)。

    `strategy_id` 指向 `quant_strategy`,取代原先的 `strategy` 字符串列
    (见 alembic 0012)。ON DELETE CASCADE:信号是夜间任务的派生数据,策略删了
    重算不出来也不该留着悬空引用。
    """

    __tablename__ = "quant_signal"
    __table_args__ = (
        UniqueConstraint("code", "date", "strategy_id", "side", name="uq_signal"),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    code: Mapped[str] = mapped_column(String(16), index=True)
    date: Mapped[date] = mapped_column(Date, index=True)
    strategy_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_strategy.id", ondelete="CASCADE"),
        nullable=False, index=True,
    )
    side: Mapped[str] = mapped_column(String(8))
    price: Mapped[float | None] = mapped_column(_PRICE, nullable=True)
    reason: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    # 执行时的完整规格哈希。旧派生信号无法可靠反推，迁移后历史行允许为空；
    # 新信号必须由运行路径在写入时固化，不能读取当前策略后补写。
    spec_hash: Mapped[str | None] = mapped_column(
        String(64), nullable=True, index=True,
    )
    # 指向同一信号最新一版研究计划。计划自身是不可变快照，重算时新建一版并
    # 通过 supersedes_plan_id 保留版本链；删除派生信号不会删除历史计划。
    plan_id: Mapped[int | None] = mapped_column(
        _BIG_PK, ForeignKey("quant_research_plan.id", ondelete="SET NULL"),
        nullable=True, index=True,
    )


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
    """回测任务。

    `strategy_id` 用 ON DELETE RESTRICT(与 Signal/StrategyEval 的 CASCADE
    不同):回测是用户主动发起并要求可复现审计的记录,不能因为删了策略就静默
    消失。API 在删除仍被回测引用的策略时返回 409,引导用户改用「停用」。
    """

    __tablename__ = "quant_backtest_run"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    user_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    strategy_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_strategy.id", ondelete="RESTRICT"),
        nullable=False, index=True,
    )
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    costs: Mapped[dict | None] = mapped_column(JSON, nullable=True)  # 回测复现:固化当时费率
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)  # 回测所用股票池
    codes: Mapped[list | None] = mapped_column(JSON, nullable=True)
    start: Mapped[date] = mapped_column(Date)
    end: Mapped[date] = mapped_column(Date)
    metrics: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    # 回测证据快照必须在任务创建时固化。旧记录没有可靠的创建时规格与数据
    # 指纹，故这些迁移新增列可空；不得用迁移时的当前策略伪造历史证据。
    strategy_spec_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    strategy_spec_hash: Mapped[str | None] = mapped_column(
        String(64), nullable=True, index=True,
    )
    compiler_version: Mapped[str | None] = mapped_column(String(64), nullable=True)
    component_versions: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    data_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    universe_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    cost_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    execution_fingerprint: Mapped[str | None] = mapped_column(
        String(64), nullable=True, index=True,
    )
    # 作业状态:pending → running → done|failed。同步路径直接写 done。
    status: Mapped[str] = mapped_column(
        String(16), default="done", nullable=False, index=True,
    )
    error: Mapped[str | None] = mapped_column(Text, nullable=True)
    # 创建时冻结的请求上下文(codes/pool/costs/dynamic_universe 等),供 worker 重放
    request_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    started_at: Mapped[datetime | None] = mapped_column(DateTime, nullable=True)
    finished_at: Mapped[datetime | None] = mapped_column(DateTime, nullable=True)
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
    """策略批量评估结果。scope: single:xxx / pool_top50 / pool。

    ON DELETE CASCADE:与 Signal 同理,评估是定时任务的派生数据,下一轮会重算。
    """

    __tablename__ = "quant_strategy_eval"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    strategy_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_strategy.id", ondelete="CASCADE"),
        nullable=False, index=True,
    )
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    scope: Mapped[str] = mapped_column(String(64), index=True)
    batch_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)  # 评估所用股票池
    start: Mapped[date] = mapped_column(Date)
    end: Mapped[date] = mapped_column(Date)
    metrics: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    # 与 Signal 同理：只记录执行当时的规格哈希，不追随当前策略原地更新。
    spec_hash: Mapped[str | None] = mapped_column(
        String(64), nullable=True, index=True,
    )
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


# 系统预置池/预置策略的 owner_id。用哨兵 UUID 而不是某个真实用户:预置内容
# 不该因为 admin 被删或换人而失去归属,也不该让「属于某人」与「系统级」两种
# 语义混淆。该值不对应 users 表的行,故 owner_id 不加 users 外键。
SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"


class Experiment(Base):
    """研究实验族:冻结的规格与假设,与日常 research_plan 分表。

    失败 trial 也保留;禁止物理删除(仅 status=archived)。strategy_id 软链,
    删策略不删账本。
    """

    __tablename__ = "quant_experiment"
    __table_args__ = (
        UniqueConstraint(
            "owner_id", "permanent_candidate_id",
            name="uq_experiment_owner_candidate",
        ),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    owner_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    permanent_candidate_id: Mapped[str] = mapped_column(String(64), nullable=False)
    family_id: Mapped[str | None] = mapped_column(String(64), nullable=True)
    title: Mapped[str] = mapped_column(String(128), nullable=False)
    hypothesis: Mapped[str] = mapped_column(Text, nullable=False)
    strategy_id: Mapped[int | None] = mapped_column(
        Integer, ForeignKey("quant_strategy.id", ondelete="SET NULL"),
        nullable=True, index=True,
    )
    frozen_spec_snapshot: Mapped[dict] = mapped_column(JSON, nullable=False)
    frozen_spec_hash: Mapped[str] = mapped_column(String(64), nullable=False)
    identity_hash: Mapped[str] = mapped_column(
        String(64), nullable=False, index=True,
    )
    validation_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    universe_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    cost_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    # design | running | completed | rejected | archived
    status: Mapped[str] = mapped_column(
        String(16), default="design", nullable=False, index=True,
    )
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime, default=datetime.now, onupdate=datetime.now,
    )


class ExperimentTrial(Base):
    """实验族中的一次具体回测(含 error/no_trades)。不可物理删除。"""

    __tablename__ = "quant_experiment_trial"
    __table_args__ = (
        UniqueConstraint(
            "experiment_id", "trial_index", name="uq_experiment_trial_index",
        ),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    experiment_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_experiment.id", ondelete="RESTRICT"),
        nullable=False, index=True,
    )
    trial_index: Mapped[int] = mapped_column(Integer, nullable=False)
    param_patch: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    backtest_run_id: Mapped[int | None] = mapped_column(
        Integer, ForeignKey("quant_backtest_run.id", ondelete="RESTRICT"),
        nullable=True, index=True,
    )
    # ok | no_trades | error | rejected
    outcome: Mapped[str] = mapped_column(
        String(16), default="error", nullable=False, index=True,
    )
    metrics_summary: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    error: Mapped[str | None] = mapped_column(Text, nullable=True)
    data_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    universe_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    cost_fingerprint: Mapped[str | None] = mapped_column(String(64), nullable=True)
    execution_fingerprint: Mapped[str | None] = mapped_column(
        String(64), nullable=True,
    )
    oos_revealed_at: Mapped[datetime | None] = mapped_column(DateTime, nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class Strategy(Base):
    """用户可管理的当前完整策略规格。

    归属模型与 `Pool` 完全一致(见 alembic 0011):可见性 = `is_system`
    OR `owner_id` 是我。**不建 grant 表** —— 当前只需要「公共」和「我的」两
    档,定向分享等真有需求再照 `quant_pool_grant` 补,不预先建一张没人写的表。

    `spec` 是当前完整 StrategySpec 的唯一事实来源，用户编辑时原地更新；
    `spec_hash` 是规范化规格的 SHA-256。`template` / `params` 仅在迁移期保留，
    新执行路径不得按具体模板名分支。`kind` 仍是服务端从规格派生的检索列。
    """

    __tablename__ = "quant_strategy"
    __table_args__ = (
        UniqueConstraint("owner_id", "name", name="uq_strategy_owner_name"),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    # NOT NULL:系统策略归 SYSTEM_OWNER_ID,不用 NULL 表达「无主」(NULL 会让
    # UniqueConstraint 失效 —— MySQL 里 NULL 互不相等,详见 alembic 0011)
    owner_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    is_system: Mapped[bool] = mapped_column(Boolean, default=False, index=True)
    name: Mapped[str] = mapped_column(String(64), default="")
    template: Mapped[str] = mapped_column(String(32), index=True)  # ma_cross / breakout ...
    kind: Mapped[str] = mapped_column(String(16), index=True)  # single / portfolio
    # 空 dict 表示全用模板默认参数;只存用户显式覆盖的键,模板默认值改动能自动生效
    params: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    spec_schema_version: Mapped[int] = mapped_column(Integer, default=1)
    spec: Mapped[dict] = mapped_column(JSON, nullable=False)
    spec_hash: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    research_status: Mapped[str] = mapped_column(
        String(32), default="unverified", index=True,
    )
    # 停用的策略不进夜间信号引擎和批量评估,但历史信号/回测记录保留。
    # 删除策略会牵连历史(见下方三张表的外键),故「停用」是常规操作、删除是例外。
    enabled: Mapped[bool] = mapped_column(Boolean, default=True, index=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime, default=datetime.now, onupdate=datetime.now,
    )


@event.listens_for(Strategy, "before_insert")
def _populate_legacy_strategy_spec(_mapper, _connection, target: Strategy) -> None:
    """让迁移期 legacy fixture/写入在入库前转换为完整 StrategySpec。"""
    from .strategy.presets import get_preset_spec
    from .strategy.spec import parse_strategy_spec, strategy_spec_hash

    spec = (
        parse_strategy_spec(target.spec)
        if target.spec else get_preset_spec(target.template, target.params)
    )
    target.spec = spec.model_dump(mode="json")
    target.spec_schema_version = spec.schema_version
    target.spec_hash = strategy_spec_hash(spec)
    target.kind = spec.kind
    target.research_status = target.research_status or "unverified"
    target.updated_at = target.updated_at or target.created_at or datetime.now()


class ResearchPlan(Base):
    """策略研究计划的不可变版本快照。

    策略可能改名、改参数甚至被删除，计划仍要能按生成当时的语义解释，因此
    策略名称、模板、版本哈希和完整生效参数均在本表固化。owner_id / is_system
    也是可见性快照，避免历史计划因当前策略行变化而越权或丢失。

    规则计算结果使用有稳定字段契约的 JSON 对象保存：不同模板的真实语义不同，
    强行拆成一组所有模板共用的可空价格列反而会诱导调用方伪造价格区间。组合
    逐股变化是重复结构，单独放在 quant_research_plan_item 中便于完整返回。
    """

    __tablename__ = "quant_research_plan"

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    owner_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    strategy_is_system: Mapped[bool] = mapped_column(Boolean, default=False, index=True)
    # 刻意不建策略外键：历史计划必须在策略删除后继续保留并展示当时实例 ID。
    strategy_id: Mapped[int] = mapped_column(Integer, nullable=False, index=True)
    strategy_name: Mapped[str] = mapped_column(String(64), nullable=False)
    template: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    strategy_kind: Mapped[str] = mapped_column(String(16), nullable=False)
    strategy_version: Mapped[str] = mapped_column(String(64), nullable=False)
    params_snapshot: Mapped[dict] = mapped_column(JSON, nullable=False)
    # 新计划直接固化完整 StrategySpec；旧计划只保存了 legacy 参数快照，无法
    # 在迁移时证明原始规格，因此兼容历史行允许为空。
    strategy_spec_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    strategy_spec_hash: Mapped[str | None] = mapped_column(
        String(64), nullable=True, index=True,
    )

    plan_type: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    code: Mapped[str | None] = mapped_column(String(16), nullable=True, index=True)
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)
    data_date: Mapped[date] = mapped_column(Date, nullable=False, index=True)
    generated_at: Mapped[datetime] = mapped_column(DateTime, nullable=False, index=True)
    next_execution_date: Mapped[date | None] = mapped_column(Date, nullable=True)
    valid_until: Mapped[date | None] = mapped_column(Date, nullable=True)
    signal_type: Mapped[str] = mapped_column(String(32), nullable=False)
    status: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    status_reason: Mapped[dict] = mapped_column(JSON, nullable=False)
    price_adjustment: Mapped[str] = mapped_column(String(16), nullable=False,
                                                  default="forward")
    signal_price: Mapped[float | None] = mapped_column(_PRICE, nullable=True)

    entry_observation: Mapped[dict] = mapped_column(JSON, nullable=False)
    risk_rules: Mapped[list] = mapped_column(JSON, nullable=False)
    take_profit: Mapped[dict] = mapped_column(JSON, nullable=False)
    native_exit: Mapped[list] = mapped_column(JSON, nullable=False)
    exit_hits: Mapped[list] = mapped_column(JSON, nullable=False)
    portfolio_summary: Mapped[dict | None] = mapped_column(JSON, nullable=True)

    backtest_run_id: Mapped[int | None] = mapped_column(
        Integer, ForeignKey("quant_backtest_run.id", ondelete="RESTRICT"),
        nullable=True, index=True,
    )
    backtest_evidence: Mapped[dict] = mapped_column(JSON, nullable=False)
    product_boundary: Mapped[str] = mapped_column(Text, nullable=False)
    revision: Mapped[int] = mapped_column(Integer, nullable=False, default=1)
    supersedes_plan_id: Mapped[int | None] = mapped_column(
        _BIG_PK, ForeignKey("quant_research_plan.id", ondelete="SET NULL"),
        nullable=True, index=True,
    )


class ResearchPlanItem(Base):
    """组合调仓计划的逐股变化快照。"""

    __tablename__ = "quant_research_plan_item"
    __table_args__ = (
        UniqueConstraint("plan_id", "code", name="uq_research_plan_item"),
    )

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    plan_id: Mapped[int] = mapped_column(
        _BIG_PK, ForeignKey("quant_research_plan.id", ondelete="CASCADE"),
        nullable=False, index=True,
    )
    code: Mapped[str] = mapped_column(String(16), nullable=False, index=True)
    previous_weight: Mapped[float] = mapped_column(_WEIGHT, nullable=False, default=0)
    target_weight: Mapped[float] = mapped_column(_WEIGHT, nullable=False, default=0)
    change_type: Mapped[str] = mapped_column(String(16), nullable=False, index=True)
    score: Mapped[float | None] = mapped_column(Float, nullable=True)
    score_details: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    rank: Mapped[int | None] = mapped_column(Integer, nullable=True)
    eligible: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True)
    reasons: Mapped[list] = mapped_column(JSON, nullable=False)
    risk_snapshot: Mapped[dict | None] = mapped_column(JSON, nullable=True)


class Pool(Base):
    """股票池定义。kind: index(动态查指数成分)/ all(全市场按上市退市ST过滤)/ static(直查成员)。

    可见性 = `is_system` OR `owner_id` 是我 OR `quant_pool_grant` 里有我的行。

    早先用 `user_id IS NULL` 表示系统级共享,有三个问题:
    1. **唯一约束失效** —— MySQL 里 NULL 互不相等,实测可插入 3 条同名系统池
       而不报错(用户池则正确拦住)。保护恰好在最需要的地方失灵;
    2. 每个查询都要写 `(user_id IS NULL) OR (user_id = :uid)`,漏一次就是
       越权或漏数据;
    3. 只能表达「我的」和「所有人的」,没有「分享给特定用户」。
    """

    __tablename__ = "quant_pool"
    __table_args__ = (
        UniqueConstraint("owner_id", "name", name="uq_pool_owner_name"),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    kind: Mapped[str] = mapped_column(String(16), index=True)  # index / all / static
    ref: Mapped[str | None] = mapped_column(String(32), nullable=True)  # 如 hs300_zz500
    # NOT NULL:系统池归 SYSTEM_OWNER_ID,不再用 NULL 表达「无主」
    owner_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    # 显式标记而非靠 owner_id 推断。系统池不给每个用户插授权行(新用户注册
    # 零成本,新增系统池也不必回填存量用户),靠这一列短路可见性判断。
    is_system: Mapped[bool] = mapped_column(Boolean, default=False, index=True)
    name: Mapped[str] = mapped_column(String(64), default="")
    min_list_days: Mapped[int] = mapped_column(Integer, default=60)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)


class PoolGrant(Base):
    """池的共享授权。只存**真实的**分享关系 —— 系统池靠 Pool.is_system 短路,
    不在此表插行,否则每个新用户注册都要批量插授权、新增系统池要回填存量用户。
    """

    __tablename__ = "quant_pool_grant"

    pool_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("quant_pool.id", ondelete="CASCADE"), primary_key=True)
    user_id: Mapped[str] = mapped_column(String(36), primary_key=True, index=True)
    can_edit: Mapped[bool] = mapped_column(Boolean, default=False)
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
