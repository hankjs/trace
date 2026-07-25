# quant - A股日频研究决策工作台

面向个人投资者的日频量化研究与决策支持系统。它把行情、技术指标、基本面、
选股、策略提示、模拟回测和手工记账组织成一条可理解、可追溯的研究流程。

系统只提供信息和模拟结果：**不连接券商、不提交订单、不提供自动或半自动交易**。
所有真实买卖、仓位和风险决策均由用户确认，并在外部交易应用中手工执行；
`quant_trade` 只是已完成交易的手工账本。

| 文档 | 内容 |
|---|---|
| 本文 | 功能、启动、API、调度 |
| [`DATA-ARCHITECTURE.md`](DATA-ARCHITECTURE.md) | 数据分层、前复权价的重写风险、精度陷阱、数据源覆盖边界 |
| [`REVIEW.md`](REVIEW.md) | 代码与数据库审查基线 |
| [`DESIGN.md`](DESIGN.md) | UI 设计系统（与数据架构无关） |
| `alembic/versions/*.py` | 每次 schema 变更的设计理由（写在 docstring 里） |

## 研究工作流

1. 在“今日研究”确认行情、候选池和提示的数据日期。
2. 查看每日 Top 30 候选，或用技术面、估值、财务和行业条件组合筛选股票。
3. 查看每只股票命中的条件，以及策略为何给出入场、退出或继续观察提示。
4. 用历史回测、参数扫描和策略排行检查收益、回撤、胜率与交易频率。
5. 用户自行判断并在外部应用交易；需要复盘时再手工记录成交和持仓。

前端提供“今日研究、选股、信号提醒、策略研究、我的持仓、研究词典”等工作区。
指标、策略、信号和回测指标的中文名称及限制来自后端固定字典
`GET /api/catalog`，英文 key 只作为稳定的内部标识。

当前筛选器是结构化条件构建器，支持条件启停、AND/OR 分组、独立命中数量和浏览器本地方案保存。
**自然语言选股尚未实现**，不会把用户句子转换成筛选条件。

## 系统边界

- 策略只消费日线；T 日收盘形成的回测信号在 T+1 日开盘模拟成交。
- 盘中快照只用于页面展示和持仓估值，不用于盘中策略或交易执行。
- 回测包含费用和滑点假设，但历史模拟不代表真实可成交价格或未来收益。
- 策略提示是研究状态变化，不是买卖指令。
- 法定节假日没有单独交易日历，任务会因当日没有行情数据而自然空跑。

## 环境与启动

Python 要求 `>=3.11,<3.14`：

```bash
uv sync
uv run uvicorn app.main:app --port 8100 --reload
```

Swagger UI：`http://localhost:8100/docs`。

数据库和 JWT 默认读取上级仓库 `config.toml` 的 `[server].database_url` 与
`[server].jwt_secret`；本目录 `config.toml` 的 `[quant]` 可覆盖
`database_url`、`jwt_secret`、`cors_origins`、`snapshot_retention_days` 和
`backfill_start`。配置文件均不得提交。

后端启动时会通过 SQLAlchemy 创建缺失的 `quant_*` 表，并幂等升级估值/财报版本
唯一键以及用户所有者字段，然后启动下述自动调度任务。

从旧版共享自选和账本升级时，旧数据默认保持未归属并对普通用户不可见。确认目标用户
的 `users.id` 后，显式执行一次：

```bash
uv run python scripts/claim_legacy_user_data.py --user-id 1
```

除 `/api/health` 和 `/api/auth/login` 外，业务接口需要：

```text
Authorization: Bearer <token>
```

前端开发需要同时运行后端：

```bash
cd web
pnpm install
pnpm dev       # http://localhost:5173，/api 代理到 localhost:8100
```

生产构建：

```bash
cd web
pnpm build     # 严格 TypeScript 检查并输出 web/dist
```

`web/dist` 存在时由 FastAPI 直接托管；它是生成目录，不要手工编辑。

## 数据与防前视

行情以 baostock 前复权数据写入 `open/high/low/close`，不复权收盘写入
`raw_close`；策略、因子、信号和回测统一使用前复权序列。**前复权价的基准是最新
交易日，所以每次分红送转上游会回溯重写全部历史价格** —— 这使增量更新在原理上
不安全（新旧批次跨两个复权尺度会产生假跳空）。系统用 `quant_adjust_factor` 的
权威因子加两层重锚检测应对，设计理由、精度陷阱与数据源覆盖边界见
[`DATA-ARCHITECTURE.md`](DATA-ARCHITECTURE.md)。

北交所（330 只 `bj.` 代码）baostock 完全不覆盖，日线走 akshare 新浪源、复权因子
由 `close/raw_close` 自算并标 `source='sina'`，可信度低于权威值。自选股日线会使用
AkShare 做对账，东财接口不可用时部分行情能力会降级到新浪。

主要数据表：

| 表 | 内容 |
|---|---|
| `quant_stock`、`quant_index_member` | 股票基础资料（含上市/退市日、ST 标记）和沪深300/中证500成分历史 |
| `quant_adjust_factor` | 复权因子，按除权日稀疏存储；`source` 区分 baostock 权威值与自算值 |
| `quant_trade_calendar` | 交易日历（替代按星期判断，避免节假日误采与假告警） |
| `quant_pool`、`quant_pool_member` | 股票池组；预置池为 `kind='index'`/`'all'` 动态解析，自定义池只存代码 |
| `quant_watchlist` | 按共享 `users.id` 隔离的用户自选关系 |
| `quant_daily_bar`、`quant_snapshot` | 前复权日线（含 `raw_close`）和展示用盘中快照 |
| `quant_factor_daily`、`quant_pick` | 每日技术因子和 Top 30 候选 |
| `quant_valuation_snapshot` | PE(TTM)、PB、PS(TTM)、股息率、总市值 |
| `quant_fundamental_snapshot` | ROE、收入/利润增长、毛利率、净利率、负债率、现金流质量 |
| `quant_signal` | 单标的策略提示及结构化原因 |
| `quant_strategy_eval` | 周度策略批量评估，同批以 `batch_id` 标识 |
| `quant_backtest_run`、`quant_backtest_equity` | 按用户隔离的回测参数、费率快照、指标和净值曲线 |
| `quant_trade` | 按用户隔离的外部已完成成交手工记录 |

估值和财务快照同时保存 `data_date`、报告期、`available_date` 与来源；同一报告期
后续修订会按新的 `available_date` 保留为独立版本，不覆盖当时已经公开的旧值。
历史筛选只读取 `available_date <= 研究日` 的记录，未到披露/可用日的财务报告不会
提前进入结果，避免用后来发布的数据研究过去。没有可靠公告日期的降级数据以同步日
作为 `available_date`，宁可少用历史数据，也不引入未来信息。历史估值源失败时不会
降级为当前快照；结构化筛选只接受研究日前 7 个自然日内的估值，更旧记录按缺失处理。

自动估值和财务任务优先同步自选与最近候选，最多 30 只，**远未覆盖全市场**（日线已覆盖
5584 只，而估值与财务快照表当前为空）。筛选页会逐条件显示覆盖数量并警告不完整范围；
需要扩大覆盖时，由管理员通过下述接口按明确 `universe` 和 `max_codes` 分批同步。

手动同步接口仅允许 JWT 中 `can_admin=true` 的管理员调用。示例：

```bash
# 显式股票；也可使用 universe=watchlist|pool|hs300|zz500|all
curl -X POST \
  'http://localhost:8100/api/admin/sync-fundamentals?codes=600519,sz.000001&max_codes=20' \
  -H 'Authorization: Bearer <token>'

# 单股历史估值数据量较大，默认不启用
curl -X POST \
  'http://localhost:8100/api/admin/sync-fundamentals?codes=600519&valuation_history=true' \
  -H 'Authorization: Bearer <token>'
```

## 组合筛选

`POST /api/selection/screener` 支持基础信息、技术面和基本面字段，字段、单位、
输入换算和允许的操作符以 `/api/catalog` 的 `filter_fields` 为准。支持的研究范围包括
`pool`、`hs300_zz500`、`hs300`、`zz500`、`watchlist` 和 `all`。

> **注意实现现状**：股票池已抽象为 `pool_id` + `kind` 分派（见
> [`DATA-ARCHITECTURE.md`](DATA-ARCHITECTURE.md) 第 5 节），但目前只贯通到
> `/api/backtest`。筛选与选股接口后端仍接受上面这组 `universe` 字符串，而前端
> `api.ts` 已改用 `pool_id` —— 这是一处**未收口的不一致**。

```json
{
  "date": "2026-07-24",
  "universe": "pool",
  "logic": "and",
  "groups": [
    {
      "id": "quality_value",
      "logic": "and",
      "conditions": [
        {"id": "pe", "field": "pe_ttm", "operator": "between", "value": 0, "value_to": 25},
        {"id": "roe", "field": "roe", "operator": "gte", "value": 0.15}
      ]
    },
    {
      "id": "trend",
      "logic": "and",
      "conditions": [
        {"id": "ma", "field": "ma_bull", "operator": "eq", "value": true}
      ]
    }
  ],
  "limit": 100
}
```

响应包含组合命中数量、每条条件的独立命中数量、每只股票的字段值、命中/未命中
条件和财务数据可用日期；缺少历史指数成分时会明确报错，不会用当前成分替代。
旧的 `GET /api/selection/screener` 简单筛选接口仍保留兼容。
`is_st` 现为 `quant_stock` 的独立列，由证券资料同步时按名称与上市状态判定
（当前 592 只）。但它仍是**当前状态的快照，没有历史**：`quant_stock` 每只股票只有
一行，改名为 `*ST` 会覆盖旧值。所以历史筛选和回测无法还原研究日当时的风险警示状态
—— 一只 2020 年正常、2024 年才被 ST 的股票，回测 2020 年区间时也会被当前的 ST 标记
剔除。这是已知的口径缺口，若要修需要给 ST 状态加时间维度（类似
`quant_index_member` 的 `in_date`/`out_date` 形态）。

## 内置策略与回测

系统注册 6 个策略：

| key | 中文名称 | 类型 |
|---|---|---|
| `ma_cross` | 双均线趋势策略 | 单只股票 |
| `breakout` | 价格突破策略 | 单只股票 |
| `mean_reversion` | 上升趋势中的超跌反弹策略 | 单只股票 |
| `volume_breakout` | 缩量整理后的放量突破策略 | 单只股票 |
| `momentum_rotation` | 强势股票轮动策略 | 股票组合 |
| `multifactor_hold` | 多指标综合评分持有策略 | 股票组合 |

默认参数和参数范围由 `/api/catalog` 与 `/api/backtest/strategies` 返回。
回测默认费用为双边佣金万 2.5、卖出印花税 0.05%、滑点万 1，可在 `costs`
中覆盖；收益和回撤从初始资金起算，包含首日建仓成本。组合策略留空 `codes` 时，
使用回测区间内的历史指数成分，并按每个交易日的在册状态决定可选股票，不使用当前
成分替代历史。单标的策略支持参数网格扫描；策略排行读取最近一轮批量评估。

## API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 无需登录的健康检查 |
| POST/GET | `/api/auth/login`、`/api/auth/me` | 登录与当前用户 |
| GET | `/api/catalog` | 固定中文指标、筛选字段、策略、信号和回测指标字典 |
| GET | `/api/market/stocks?q=&limit=` | 按中文名、六位代码或标准代码搜索股票；空查询只返回自选 |
| GET | `/api/market/kline`、`/api/market/snapshot` | 日线和自选股最新展示价格 |
| GET/POST/DELETE | `/api/watchlist`、`/api/watchlist/{code}` | 自选股管理 |
| GET | `/api/selection/picks` | 每日 Top 30 候选及新进/调出 |
| GET/POST | `/api/selection/screener` | 简单筛选兼容接口 / 结构化组合筛选 |
| GET | `/api/signals` | 带股票中文名、策略中文名和中文原因的提示查询 |
| GET/POST/DELETE | `/api/portfolio/trades`、`/api/portfolio/trades/{id}` | 手工成交记账 |
| GET | `/api/portfolio/positions` | 均价成本、参考市值与盈亏 |
| GET | `/api/backtest/strategies`、`/api/backtest/leaderboard` | 策略目录和评估排行 |
| POST | `/api/backtest`、`/api/backtest/sweep` | 同步回测和单标的参数扫描 |
| GET | `/api/backtest/{id}` | 已保存回测、净值曲线和当时使用的股票池 |
| GET/POST/PATCH/DELETE | `/api/pools`、`/api/pools/{id}` | 股票池组管理；预置池只读 |
| GET/POST/DELETE | `/api/pools/{id}/members`、`/api/pools/{id}/members/{code}` | 池成员增删；预置池返回当日解析成分 |
| POST | `/api/admin/backfill`、`/api/admin/import-stocks` | 管理员：日线回填和股票资料导入 |
| POST | `/api/admin/sync-index-members` | 管理员：同步沪深300和中证500成分 |
| POST | `/api/admin/sync-adjust-factors` | 管理员：采集复权因子权威值（默认跳过北交所） |
| POST | `/api/admin/sync-trade-calendar` | 管理员：同步交易日历 |
| POST | `/api/admin/backfill-list-dates` | 管理员：回填上市日期（`kind='all'` 解析的前置） |
| POST | `/api/admin/sync-fundamentals` | 管理员：同步估值与财务数据，同进程任务互斥 |
| POST | `/api/admin/run-selection`、`/api/admin/run-signals`、`/api/admin/run-eval`、`/api/admin/snapshot` | 管理员：手动触发研究任务 |

## 自动调度

APScheduler 使用 `Asia/Shanghai` 时区：

- 周一至周五 16:30：串行执行股票池/自选日线增量、因子与 Top 30 选股、
  自选加候选的单标的策略提示；周五追加近一年策略批量评估。任一股票行情更新失败时，
  当晚流水线中止，不发布基于部分股票的候选结果。
- 周一至周五 9:30-15:00 每 30 分钟：仅采集自选股盘中快照。
- 每月 1 日 08:30：同步交易日历（先于成分同步，后续任务依赖它判定交易日）。
- 每月 1 日 09:00：同步沪深300和中证500成分名录。
- 每周六 08:00：同步证券资料（股票名称、上市/退市日、ST 标记）。
- 周一至周五 18:30：同步自选加最近候选的估值快照，最多 30 只。
- 每月 2 日 19:00：同步同一有限范围的财务报告。

复权因子（`/api/admin/sync-adjust-factors`）目前**未加入自动调度**，需在分红季后
手动触发。因子只在除权日变化，日常增量不需要它；但若长期不同步，
`audit_scale_against_factors` 的权威基准会滞后于实际除权，退化为「无法核对最新 bar」。

## 验证与目录

```bash
uv run pytest tests/
cd web && pnpm build
curl http://localhost:8100/api/health
```

```text
app/
├── api/                 # FastAPI 路由
├── data/                # 行情、指数成分、估值和财务数据采集
├── factors/             # 每日技术因子
├── indicators/          # MA、EMA、MACD、RSI、ATR、量比
├── selection/           # Top 30 pipeline 与结构化筛选器
├── strategy/strategies/ # 6 个单标的/组合策略
├── backtest/            # vectorbt 回测、参数扫描和批量评估
├── portfolio/           # 手工成交和持仓推导
├── catalog.py           # 固定中英文字典与用户说明
├── models.py            # quant_* SQLAlchemy 模型
├── migrations.py        # Alembic 版本检查(启动时不再建表改表)
├── scheduler.py         # 自动研究任务
├── scheduler_lock.py    # 调度器跨进程互斥(多副本不重复采集)
└── main.py              # FastAPI 入口与前端静态托管

alembic/versions/        # Schema 迁移;每个 revision 的 docstring 记录设计理由

web/src/
├── views/               # 页面工作区
├── components/          # 可复用界面组件
├── catalog.ts           # 目录加载与前端降级文案
└── api.ts               # API 与共享 TypeScript 类型
```
