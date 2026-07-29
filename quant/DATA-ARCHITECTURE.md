# 数据架构设计

本文档说明 quant 的数据分层、复权价的处理方式、以及各数据源的覆盖边界。

`DESIGN.md` 是 UI 设计系统，与本文档无关。数据模型的逐次变更依据记录在
`alembic/versions/*.py` 的 docstring 里，本文档是它们的总览与设计理由。

## 1. 三种数据的生命周期

区分它们是理解本系统数据模型的前提。**同一张表里混放不同生命周期的数据，是
本项目历史上多个数据腐蚀缺陷的共同根因。**

| 类别 | 表 | 会不会被改写 | 备份价值 |
|---|---|---|---|
| **原始事实** | `quant_daily_bar` 的 `raw_close`/`volume`/`amount`、`quant_adjust_factor`、`quant_trade_calendar`、`quant_stock` | 永不改写 | 高（重拉需数小时） |
| **外部版本指标** | `quant_valuation_snapshot`、`quant_fundamental_snapshot` | 按交易日或报告版本追加；同版本可被数据源修订 | 中（可重拉，但需保留 `source`/`available_date`） |
| **派生视图** | `quant_daily_bar` 的 `open/high/low/close`（前复权）、`quant_factor_daily`、`quant_pick`、`quant_signal`、`quant_strategy_eval`、`quant_backtest_*` | 会被重算或重写 | 低（可从原始事实重建） |
| **用户数据** | `quant_trade`、`quant_watchlist`、`quant_user_settings`、`quant_pool`、`quant_pool_member` | 只由用户改 | 最高（不可再生） |

**自选股**：真源是 `quant_watchlist(user_id, code)`。`quant_stock` 上早期的 `is_watch`
列已删除（`0020_drop_stock_is_watch`）；API 响应里的 `is_watch` 由当前用户的
watchlist join 计算，不是股票主表字段。

实践含义：

- 派生数据坏了删掉重算即可，不必备份；用户数据只有几 KB 却最该备份。当前备份是
  一刀切（929 MB 里绝大部分是可重建的日线），有优化空间。
- 业务代码不应写原始事实表 —— 写入只经 `app/data/`。历史上 `scripts/backfill_pool.py`
  的 `Base.metadata.create_all()` 越界建表，把生产库搞成「新表已存在但列不全」的
  混合状态，导致 Alembic 无法接管（处置脚本见 `scripts/prepare_alembic_takeover.py`）。

### 为什么 PE/ROE 使用 snapshot，而不是查询时从日线计算

`quant_daily_bar` 只有价格、成交量和成交额。可靠计算 PE/PB/PS/ROE 还需要带公告与
修订时间的 TTM 归母净利润、平均净资产、营业收入、历史总股本等事实；这些原始财务
报表当前没有入库。只用收盘价动态计算会在财报修订、增发和分红时产生看似精确、实则
口径错误的值。

因此当前边界是：

- `quant_valuation_snapshot` 按交易日保存数据源给出的 PE(TTM)、PB、PS(TTM) 和总市值；
- `quant_fundamental_snapshot` 按 `report_period + available_date` 保存 ROE、增长率、利润率、
  负债率和现金流质量指标，修订后新增可用版本，防止历史研究看到未来值；
- 技术指标仍从日线计算，可随时重建，不混入上述两张外部版本指标表。

#### `valuation.report_period`：预留列，不是 baostock 日 K 字段

估值表与财务表共用「版本化 snapshot」形状，因此估值侧也有可空的 `report_period`
（「这条估值数字锚定哪一期财报」）。**它不是 baostock 批量日 K 带来的列**——日 K 的
`peTTM`/`pbMRQ`/`psTTM` 是市场日频口径，接口不返回对应报告期。

| 表 | `report_period` | 当前状态 |
|---|---|---|
| `quant_fundamental_snapshot` | **必填**，报告期版本主键的一部分 | 在用 |
| `quant_valuation_snapshot` | **可空**，schema 预留 | 日频 pe 源写 `NULL`；勿为填而填假报告期 |

何时才会有值：若将来接入「基于最新年报/季报口径」的估值源，再写入对应报告期。在那
之前保持 `NULL` 是正确语义（「无报告期锚点的日频市场估值」），不是数据缺漏。

只有在后续引入完整的利润表、资产负债表、现金流量表、股本与分红事实表后，才适合把
这些比率改成查询时或物化视图计算。当前再建一张“计算后基础数据表”只会复制现有两张
snapshot 表的职责。

## 2. 前复权价：为什么它是最麻烦的一列

`quant_daily_bar` 的 `open/high/low/close` 存的是**前复权价**，而前复权价的定义
依赖基准日：baostock 以最新交易日为基准，所以**每次分红送转，全部历史价格都会被
上游回溯重写**。

这带来一个原理性问题：**增量更新在设计上就不安全**。拉最近 10 天 upsert 时，若
这期间发生分红，新数据是新基准下的价格，而库里 2015 年以来的历史仍是旧基准 ——
同一列跨两个尺度，产生**假跳空**。全市场回填实测检出 6 例真实错乱
（`sh.600039`、`sh.600161`、`sh.600188`、`sh.600875`、`sh.601136`、`sz.003035`）。

### 应对：因子独立成表 + 两层检测

`quant_adjust_factor(code, divid_operate_date, fore_factor, back_factor, source)`
按除权日**稀疏**存储（`sh.600519` 的 2808 行日线只对应 17 个除权日，压缩 175:1；
全市场约 4.2 万行）。

#### `fore_factor` 与 `back_factor`

baostock `query_adjust_factor` 一次返回前复权因子（`foreAdjustFactor`）与后复权因子
（`backAdjustFactor`），入库时成对写入，源数据原样保留。

| 列 | baostock 字段 | 本系统用途 |
|---|---|---|
| **`fore_factor`** | `foreAdjustFactor` | **当前唯一使用路径**：批量日 K 换算前复权、尺度审计、重锚检测 |
| **`back_factor`** | `backAdjustFactor` | **入库保留、业务不读**。本系统日线 `open/high/low/close` 存的是**前复权价**，不构造后复权序列 |

`source='sina'`（北交所自算）通常只写 `fore_factor`，`back_factor` 为空。若日后需要与
券商后复权对账或展示后复权净值，再消费 `back_factor`，不要为「填满列」去反推假值。

重锚检测分两层，权威基准优先：

| 层 | 函数 | 能发现什么 |
|---|---|---|
| 1 | `audit_scale_against_factors` | **既存**错乱 —— 库中历史与权威因子对不上 |
| 2 | `detect_reanchor` | **本次增量**引入的错乱 —— 新旧批次的反推因子不一致 |

**为什么必须有第 1 层**：`detect_reanchor` 是「库中反推因子 vs 新拉反推因子」的
自比对，两边都来自 `close/raw_close`。若某股入库时历史就已错乱，两边会一致地错
下去 —— 拿反推值当基准是循环论证。权威因子是独立的第三方基准。

**为什么反推仍然保留**：因子表缺该股数据时（`no_factors`）它是唯一手段。不能因为
缺基准就假定尺度正确。

### 一个仍然存在的设计约束

因子表的存在**没有**消除前复权价被重写的问题，只是让它可被检测。彻底的解法是
「只存原始价 + 因子，前复权按需计算」，代价是 `load_bars_df` 全路径改造、
`open/high/low` 的 raw 值需重新采集（现在只存了复权后的）、以及每次读取多一次
join 与乘法。当前选择的是渐进方案：读取路径不动，先建立可检测性。

## 3. 数据精度：一个反复踩到的陷阱

价格列是 `DECIMAL(12,4)`（`_PRICE`，见 `app/models.py`）。这个精度对价格本身够用，
但**用它反推复权因子时，误差量级取决于股价**：

| 标的 | 股价 | 相邻因子相对变化的噪声 |
|---|---|---|
| `sh.600519` 茅台 | ~1300 元 | ~1e-06 |
| `bj.920000` 安徽凤凰 | ~10 元 | P50 = 2.1e-04，P90 = 1.1e-03，**P99 = 1.9e-03** |

后果是**用高价股验证得出的阈值对低价股完全失效**。这个陷阱在本项目踩了两次：

1. 因子变化判定阈值取 `1e-4`（用茅台验证时看起来合理），对 `bj.920000` 把 1353 行
   日线判出 **898 个假除权日**，真实除权只有 5 次。
2. 改成 `0.01` 后又漏了另一头：该股 2026-05-25 有次真实除权，因子变化仅 **0.53%**，
   被当噪声丢弃，导致因子表最后一行停在一年前，审计核对最新 bar 时取到过时因子，
   **误报 87/330 只**。

根因是低价股的噪声（P99 0.19%）与小额分红的真实信号（0.53%）**量级重叠**，单一
固定相对阈值无法干净分离 —— 调阈值只是在「漏报噪声」与「漏报除权」之间换一头错。

当前处置（`FACTOR_CHANGE_TOLERANCE = 0.01` + 两项补偿）：

- **末行锚点**：`derive_adjust_factors` 额外存最新交易日的实际因子，让审计最常做的
  「核对最新 bar」变精确。
- **分源容差**：`audit_scale_against_factors` 对 `source='baostock'` 用严格阈值
  （`REANCHOR_TOLERANCE = 0.001`），对 `source='sina'` 用 `DERIVED_FACTOR_TOLERANCE
  = 0.012`。

**已接受的代价，必须明确**：自算因子只保证检出 **>1%** 的尺度错乱，中间日期的小额
分红漏记不报警。它不与权威值同等可信 —— 这就是 `source` 列存在的意义，否则
「全库尺度与权威因子一致」这个结论会被悄悄稀释成「一致或自证一致」。

## 4. ST 状态：两个字段，只有一个能用于回测

| 字段 | 语义 | 用途 |
|---|---|---|
| `quant_daily_bar.is_st` | **逐日**真实历史（baostock 日线的 `isST`） | **回测与历史筛选唯一正确的口径** |
| `quant_stock.is_st` | 当前状态快照，改名即覆盖 | 仅供 UI 展示 |

用当前状态过滤历史样本是**系统性前视偏差** —— 等于用「今天知道谁变差了」去筛研究日当时的候选池。实测抽样 8 只当前 ST 股（各 2808 行，共 22464 个交易日）：

| 股票 | 真正 ST 的交易日 | 首次 ST |
|---|---|---|
| `sh.600053` *ST九鼎 | 58 天（2%） | 2026-04-30 |
| `sh.600082` ST海泰 | 70 天（2%） | 2026-04-14 |
| `sh.600107` *ST尔雅 | 299 天（11%） | 2025-05-06 |
| `sh.600136` ST明诚 | 1025 天（37%） | 2022-05-06 |
| **合计** | **3233 / 22464 = 14.4%** | |

即 **85.6% 的交易日被当前标记错误剔除**。`sh.600053` 只有 2% 的时间是 ST，但它 2015-2026 的全部 2808 行都进不了样本。

偏差方向是**高估策略表现**：被剔掉的恰是后来才出问题的公司，而研究日当时它们看起来完全正常、会被策略正常选中。这与 `engine.py` 的提前建仓、财报缺 `available_date` 属于同一类错误，只是入口不同。ST 股集中在小市值高波动区间，恰是动量与均值回归策略最易选中处，所以 4.7% 的股票占比低估了实际影响。

`NULL` 的含义是「未采集」而非「非 ST」。过滤时必须显式区分 —— 把 `NULL` 当 `False` 会让未采集的行被当成「确认非 ST」放进样本。

**严格口径（开发阶段已启用）**：`all_market_pool` **只**纳入当日 `is_st IS FALSE` 的代码；`TRUE` / `NULL` / 无 bar 一律排除，**不再**回退 `quant_stock.is_st`。动态池 `pool_eligibility_matrix` 在窗口内缺 ST 时直接 `MissingPoolHistoryError`。

覆盖率通过 `app/data/quality.py` 与 `GET /api/market/data-quality` 暴露；回测结果在 `metrics.data_quality` 中标注 `st_history_incomplete` 与字段覆盖。未回填 ST 的标的会被排除出历史全 A 池，宁可池子变小，也不引入前视偏差。

补齐路径有两条，会自然收敛：

- **日常调度自动补**：每晚的 `ingest_daily` 走 `fetch_daily_bars`（已带 `isST`），每次拉最近 10 天，所以新数据一定有值，并会回补最近 10 天的历史行。
- **一次性回填**：`scripts/backfill_is_st.py` 只 `UPDATE` 这一列、不碰价格列、不走重锚检查，按股票粒度断点续跑。剩余约 4956 只，单进程 0.15s 间隔约 25 分钟。

> **教训（写在这里以免重犯）**：最初我用 `backfill_pool.py --force-rescale all` 四分片并发全量重拉 1138 万行来补这一列 —— 其余 8 列的值完全不变，重拉纯属浪费，还要重走重锚判定去动已验证一致的价格序列。代价是真实的：触发 baostock 限速惩罚，出口 IP 被拉黑（错误码 `10001011`），采集中断。根因与硬性上限见下文「baostock 连接与限速」。
>
> 两条原则：**为补新增列不要重拉整行**；**严禁对 baostock 并发连接**。它是免费公益数据源，限速按 IP 计 —— 换 IP 绕过既可能招致更长封禁，也不厚道。

**一处刻意的例外**：`selection/pipeline.py` 的 `run_selection` 仍按当前名称判定 ST。它跑的是**当日**选股，当日的当前状态就是正确状态，不存在前视问题。只有历史回溯（回测、历史筛选）才必须用逐日字段。

## 5. 数据源覆盖边界

**没有单一数据源覆盖全部 A 股。** 这不是配置问题，是必须在设计里承认的事实。

| 数据源 | 覆盖 | 不覆盖 |
|---|---|---|
| **baostock** | 沪深两市日线（前复权 + 不复权）、复权因子权威值、指数成分、交易日历、证券资料 | **北交所**（`bj.` 前缀报 `10004011`；换 `sh.`/`sz.` 前缀参数校验通过但**返回 0 行**） |
| **akshare 东财** | 全市场快照、盘后对账 | 不稳定，会整段限流（实测连主板都 `ConnectionError`） |
| **akshare 新浪** | 北交所日线（`adjust=''` 与 `'qfq'` 都有） | 部分老北交所代码（`bj.430xxx`/`bj.830xxx`）返回 `JSONDecodeError` |

### baostock 连接与限速（硬性约束）

官方文档：<https://www.baostock.com/mainContent?file=pythonAPI.md>
（本地包 `baostock>=0.9` 的 demo / `security/history.py` 与文档一致；本环境实测 `00.9.30`。）

baostock 官方对连接访问有明确配额；**超限会按出口 IP 进入黑名单**，表现为
登录/查询失败（常见错误码 `10001011` 服务不可用）。项目内已多次因超额或并发
被封 IP，回填与运维脚本必须把下列规则当硬约束，而不是「尽量友好」：

| 规则 | 说明 |
|---|---|
| **每日 API 请求 ≤ 5 万次** | 按调用次数计（不是按股票只数、不是按返回行数）。 |
| **禁止并发连接** | 同一时刻只能有一个进程/会话在访问。多进程分片（`--shards N`）、多机并行、定时任务与手动回填叠跑，都会触发黑名单。 |
| **超限 → 黑名单** | 当日额度用尽或并发被抓到后，该出口 IP 进入控制名单，后续请求持续失败，需等解禁或换干净出口（见 `scripts/run_via_socks.py`，仅作运维兜底，不是常态采集手段）。 |
| **优先批量接口，禁止无谓单条** | 凡有「按日全市场 / 空 code 全表」接口的，**一次拉全量**；不要为省本地解析而改成按 code 循环。见下表。 |

#### 批量 vs 单条（接口选型）

| 能力 | 批量接口（优先） | 单条接口（仅在无批量时用） | 请求量直觉 |
|---|---|---|---|
| **某日全 A 股日 K** | `query_daily_history_k_AStock(date)` | `query_history_k_data_plus(code, …)` 按只循环 | 批量 **1 次/交易日** ≈ 全市场；单条 ≈ `N 只 × 复权次数` |
| **某日全 ETF 日 K** | `query_daily_history_k_ETF(date)` | 同上按 code | 同上 |
| **某日全市场复权因子** | `query_daily_adjust_factor(date)` | `query_adjust_factor(code, start, end)` | 批量 **1 次/日**；单条 ≈ N 只 |
| **证券资料** | `query_stock_basic()`（code 可空 = 全表） | `query_stock_basic(code=…)` | 已按全表用 |
| **交易日历** | `query_trade_dates(start, end)` | — | 已按区间用 |
| **指数成分** | `query_hs300_stocks` / `query_zz500_stocks`（可带 `date`） | — | 已按指数用 |
| **任意区间单票历史 K** | —（**无**多 code 批量） | `query_history_k_data_plus` | 仅适合：补单票、重锚全历史、非整日全市场场景 |

要点：

1. **`query_history_k_data_plus` 的 `code` 只能是单只**（SDK 校验长度 = 9，如 `sh.600000`），**不能**逗号拼多只。要省请求数，应改用「按日全市场」接口，而不是幻想多 code 参数。
2. **盘后增量 / 全市场按日回填** 应走 `query_daily_history_k_AStock`：一天全 A 只要 **1 次** API；用单条循环拉 5000 只则是 **5000+ 次**，极易顶破日配额并触发黑名单。
3. **多年全历史** 也可按交易日循环批量接口：约 `交易日数` 次请求，而不是 `股票数 × 2（前复权+不复权）`。例如 7 年 ≈ 1700 次 vs 单条全市场 ≈ 数万次。
4. **批量日 K 的字段与复权口径**（demo / 返回列）：
   `date,code,open,high,low,close,preclose,volume,amount,adjustflag,turn,tradestatus,pctChg,peTTM,pbMRQ,psTTM,pcfNcfTTM,isST`。
   接口**没有** `adjustflag` 入参（与 `query_history_k_data_plus` 不同），返回里的 `adjustflag` 是列；落地前须与单票前复权/`raw_close` 口径对齐验证（本系统 `open/high/low/close` 要前复权，`raw_close` 要不复权）—— 通常需配合 `query_daily_adjust_factor` 或库内权威因子换算，**不能**默认与 `adjustflag=2` 的单票结果逐字段相等。
5. 批量结果一页上限约 **20000** 行（SDK 写死 `per_page_count`，不分页）；全 A 日 K 与当日因子量级在此内。

#### 本仓库现状与工程落点

**已做对的**

- **进程内串行**：`app/data/baostock_client.py` 用 `_query_lock` 串行化查询；
  `login_session()` 可重入引用计数，批量任务只 login 一次。
- **调度单会话**：`scheduler.job_daily_bars` 整段包在一次 `login_session()` 内，
  不另起并发 worker。
- **元数据已批量**：`fetch_stock_basic` / `fetch_trade_dates` / 成分股查询。

**仍偏单条、应优先改掉的（请求浪费大）**

- 日线：`fetch_daily_bars(code, …)` → 底层 `query_history_k_data_plus`，且每只 **2 次**（前复权 + 不复权）。盘后 `job_daily_bars`、回填脚本、`backfill_is_st` 都走这条路径。
- 复权因子：`fetch_adjust_factors(code, …)` → `query_adjust_factor` 按 code；应用 `query_daily_adjust_factor` 做按日全市场同步（或历史按日循环）。

**脚本纪律**

- `backfill_pool.py` / `backfill_is_st.py` 在未切批量接口前：默认单进程 + 每只 sleep；
  `--shards` 仅用于**不同机器/不同时刻**切分任务列表，**禁止**同一出口 IP 同时跑多个 shard。
- 新采集代码默认选型：**先查上表有没有批量接口**；没有再单条，并写清为何不能批量。
- 动手前估算请求量：批量按「交易日数」；单条按「股票数 × 每只次数（含重试）」；
  确认不顶破当日 5 万。宁可分多天，也不要为「跑快一点」开并发。

### 北交所（330 只）的处理

> **优先级：最低。** 用户没有北交所交易权限，bj. 标的不参与实际决策。北交所数据
> 缺口（日线滞后、`list_date` 缺失、因子自算精度低）不阻塞任何工作，修复排期永远
> 排在沪深问题之后；监控告警应把 bj. 缺口降级为提示而非错误。

它们来自 `akshare.stock_info_a_code_name()`，是真实在交易的标的
（`bj.920000` 2026-07-24 收盘 14.18，成交额 6907 万），不是废弃代码。

走新浪源补齐：日线 278,150 行，因子由 `close/raw_close` 自算并标 `source='sina'`。
末行因子为 1.0 证明新浪与 baostock 同为「最新日为基准的前复权」口径，两来源放同一
张表不会混口径 —— 这一点在动手前验证过，否则不该混。

`sync_adjust_factors` 默认跳过 `bj.`（否则每轮白发 330 次注定失败的请求）。

### 覆盖边界如何影响护栏

阈值型护栏的分母必须是**理论上可以补全的集合**。`kind='all'` 的 `list_date` 缺失
护栏最初拿全表股票数当分母，而北交所永远补不上 `list_date`（baostock 无数据），
缺失率恒为 5.9% 超过 5% 阈值 —— **默认口径永久抛 `IncompleteListingDataError`**。

改为只统计「有日线的股票」后恢复正常。教训：**数据源的固有缺口会让护栏永久触发，
最终逼人放宽阈值 —— 那就等于没有护栏。**

## 6. 股票池抽象

### 可见性：为什么不用 `user_id IS NULL`

```
quant_pool(id, kind, ref, name, min_list_days, owner_id NOT NULL, is_system)
quant_pool_grant(pool_id, user_id, can_edit)
quant_pool_member(pool_id, code)              ← 池与股票,只存代码
```

可见性 = `is_system` OR `owner_id` 是我 OR `grant` 里有我的行，收成
`api/pools.py` 的 `visible_to(user_id)` 一个函数。

早先用 `user_id IS NULL` 表示「系统级共享」，有三个问题：

1. **唯一约束失效** —— `UniqueConstraint("user_id", "name")` 对预置池完全不起作用：SQL 里 NULL 互不相等，实测可插入 3 条同名系统池而不报错，而用户池的同名被正确拦住。**保护恰好在最需要它的地方失灵** —— 预置池是全用户共用的，重复的影响面最大。
2. 可见性条件 `(user_id IS NULL) OR (user_id = :uid)` 散落 5 处，漏一次就是越权读取或漏掉预置池。
3. 只能表达「我的」和「所有人的」，没有「分享给特定用户」。

两个设计取舍：

- **系统池归哨兵 UUID**（`00000000-...`），不指向 `users` 表的真实行。预置池不该因 admin 被删或换人而失去归属，也不该让「属于某人」与「系统级」混淆。代价是 `owner_id` 不能加 `users` 外键。
- **系统池不在 `grant` 表插行**，靠 `is_system` 短路。否则每个新用户注册都要批量插授权行、新增系统池还要回填所有存量用户，漏一步就有人看不到预置池。`grant` 表只存真实的分享关系。
- **删除池受引用保护**：`backtest_run`/`strategy_eval`/`research_plan` 的 `pool_id` 刻意不加外键（与策略 `spec` JSON 内嵌 `pool_id` 同理，见第 7 节），删池 API 改为先查这三张表的引用，有引用返回 409（`api/pools.py`），避免历史审计快照指向不存在的池。spec JSON 与实验快照（`experiment.universe_snapshot`）内的 `pool_id` 扫不到，属已知残留风险。

### kind 的解析口径

设计意图是把股票池统一抽象为 `pool_id` + `kind` 分派：

| kind | 成员解析 | point-in-time |
|---|---|---|
| `index` | `quant_index_member` 的 `in_date`/`out_date` | ✅ 正确 |
| `all` | `list_date <= day - min_list_days AND (delist_date IS NULL OR delist_date > day) AND NOT is_st` | ✅ 正确 |
| `static` | `quant_pool_member` 直查 | ❌ **无成员历史** |

`static` 池只存代码不存日期，所以用它回测历史区间**带幸存者偏差**。这是已接受的
取舍（对手挑的池子符合直觉），前端在池编辑页与回测结果页各有一处标注。

### 主表完整性：日线与名录的对账

`kind='all'` 解析依赖 `quant_stock` 的 `list_date`/`delist_date`，但主表有两类
固有漂移（2026-07 实测各命中一次）：

1. **有日线但主表缺失的孤儿 code** —— 批量日线按日全市场落库（含已退市股），而
   akshare 名录只含在市股，退市股永远进不了主表（实测 167 只）。它们缺名称与
   上市日，且被池解析静默排除。
2. **在册但退市未标** —— 名录同步（周六）失败期间退市的股票 `delist_date` 停在
   NULL，被池解析误当在市股（实测 35 只）。

`ingest.reconcile_stock_master` 用 baostock 证券资料（含已退市股的完整生命周期）
修这两类：补插孤儿行、补标 `delist_date`/`is_st`，查不到元数据的（如北交所）跳过。
已挂进 `import_stock_list` 尾部随周六任务例行执行。

### 已收口的实现

`pool_id` 已贯通 `/api/backtest` 与 `/api/selection/screener`，解析统一走
`universe.resolve_pool`。早先 `screener.py` 有一套独立的 `universe` 字符串分支
（`pool` / `hs300_zz500` / `hs300` / `zz500` / `watchlist` / `all`），与
`universe.py` 各自实现 `in_date`/`out_date` 条件，两处会漂移；更隐蔽的是它的
`universe='all'` 是**全表无过滤**，与 `kind='all'`（剔 ST/退市/新股）同名不同义。

`watchlist` 不是一种 `kind` 而是独立开关 `watchlist_only`：自选是用户关系不是
股票池，做成池会引入「自选变化时池成员如何同步」的新问题。

### 空结果必须与数据缺口区分

`kind='index'` 解析为空时，若查询日早于名录最早 `in_date`，抛
`MissingIndexHistoryError` 而非返回空池。理由是两种成因的用户解读完全不同：
返回空池会被读成「这天没有符合条件的股票」，而真相是**成分数据没回填到那么早**。
这个护栏在池抽象收口时曾一度丢失（旧代码的 `allow_current_fallback` 分支被删除
后未补），是收口过程中发现并补回的。

`kind='all'` / `'static'` 不做此判断 —— 它们返回空可能是合法结果。

## 7. 策略抽象

```
quant_strategy(id, owner_id, is_system, name, kind,
               spec_schema_version, spec, spec_hash, research_status, enabled)
quant_signal(..., strategy_id → quant_strategy, spec_hash)        ← ON DELETE CASCADE
quant_strategy_eval(..., strategy_id → quant_strategy, spec_hash) ← ON DELETE CASCADE
quant_backtest_run(..., strategy_id → quant_strategy,
                   strategy_spec_snapshot, strategy_spec_hash,
                   compiler_version, component_versions,
                   data/universe/cost/execution_fingerprint)      ← ON DELETE RESTRICT
quant_research_plan(..., strategy_spec_snapshot, strategy_spec_hash)
```

`0014_dynamic_strategy_spec` 之后，`quant_strategy.spec` 是当前完整策略定义的唯一事实
来源。用户编辑时原地更新这一个 JSON，不创建草稿、发布记录或策略历史版本；历史可复现
由每次回测和研究计划自己的完整规格快照承担。

可见性沿用第 6 节股票池那套 `is_system` + `owner_id`（哨兵 UUID 归属系统行），
收成 `app/strategy/store.py` 的 `visible_to(user_id)`。三个取舍值得记住：

- **没有 `grant` 表。** 池有「分享给特定用户」的需求，策略当前只需要「公共」和
  「我的」两档。不预先建一张没人写的表 —— 真有需求时照 `quant_pool_grant` 补。
- **规则在数据库，代码只提供通用组件。** 受控 `StrategySpec` AST 经严格校验后编译为
  单标的目标仓位或组合目标权重，再交给原有 T+1 撮合。新增普通策略只写数据库 JSON，
  不新增策略专用 Python 文件，也不按策略名进入不同执行分支。
- **`kind`、`spec_hash` 和能力状态由服务端派生。** `kind` 保留为带索引的查询列，供
  夜间任务筛出所有启用的 single / portfolio 策略；客户端不能伪造这些派生值。
- **`template` / `params` 仅为迁移兼容字段。** 六个旧系统模板已转换为完整 spec 种子；
  旧客户端参数在 API 边界一次性转换成完整规格，实时编译器不读取模板模块。
- **历史证据按内容寻址。** `spec_hash` 是规范化完整规格的 SHA-256；回测另外保存编译器、
  组件、数据、股票池和费用指纹。策略页面只把哈希完全相同的回测计为当前证据。

### 两种 ON DELETE 不同，是因为两种数据的性质不同

`signal` / `strategy_eval` 是定时任务的**派生数据**，删了下一轮重算，用 CASCADE。
`backtest_run` 是用户主动发起、要求可复现审计的**资产**，不能因为删了策略就静默
消失，用 RESTRICT —— API 在删除仍被回测引用的策略时返回 409，引导改用「停用」。

### 定时任务跑所有启用的策略，成本是乘法

信号引擎的成本是「股票数 × 启用策略数」。策略从 4 个固定模板变成「所有用户启用的
策略」，这个乘数就落到了夜间流水线上，为此做了两件事：

1. **循环倒置**。原实现是 `for 策略: for 股票: load_bars_df(...)`，同一只股票的
   日线按策略数重复查库。改成「每只股票加载一次，再跑全部策略」，查库次数与策略数
   解耦（`tests/test_signal_engine.py` 用调用计数把这个顺序钉住，防止回退）。
2. **启用数单独限额**（每用户 10 条，总数 50 条）。存着不跑的策略只占一行，跑起来
   的每一个都要乘进全市场股票数。

评估跨用户跑，但信号列表与策略排行**按可见性过滤** —— 否则别人的策略名和参数会
出现在你的页面上。

## 8. 迁移与 Schema 管理

Schema 由 Alembic 管理（`alembic/versions/`），**启动时不再建表或改表**。
`scripts/verify_migration_parity.py` 校验「全新库 `upgrade head`」与「`models.py`
的 `create_all`」产出一致，防止两条路径漂移。

生产迁移的顺序约束与实测踩坑记录见 `logs/FINAL_REPORT.md`，其中三条值得在设计层
记住：

- `0001_baseline` 是「从零建库」定义，对已有表的库必须先 `stamp` 而非 `upgrade`；
- 云数据库的 `net_read_timeout`（腾讯云 CDB 默认 30 秒）会让千万行表的 DDL 在客户端
  侧断开，而 MySQL `ALTER TABLE` 的原子性保证服务端回滚、数据无损。需用长超时会话
  单独执行（`scripts/apply_daily_bar_pk.py`）；
- 逐列 `MODIFY COLUMN` 的 revision 中断后可安全续跑（幂等），会留下「部分列已转换」
  的中间状态。

## 9. 当前数据规模（2026-07-27 实测）

| 项 | 数值 |
|---|---|
| `quant_daily_bar` | 11,382,977 行 / 5,584 只 / 2015-01-05 起 |
| 其中北交所 | 330 只 |
| `quant_adjust_factor` | 42,716 行（`baostock` 41,222 / `sina` 1,494） |
| `quant_factor_daily` | 4,970 行（全市场当日因子） |
| `quant_stock.industry` | 5,531 只有值 / 128 个行业分类 |
| `quant_valuation_snapshot` | 5,531 行 / 2026-07-24；当日 5,530 根日线全部覆盖 |
| `quant_fundamental_snapshot` | 214,567 行 / 5,584 只 / 46 个报告期（2015-03-31 至 2026-06-30） |
| `kind='all'` 解析结果 | 5,300 只 |
| 尺度审计 | 北交所 330 只全部一致；权威源抽样 400 只全部一致 |

估值初始化只写最新交易日，不追拉 2015 年以来的每日历史；每天约 5,500 行的历史回填
请求和存储成本都高，且会显著增加数据源封禁风险。交易日 18:30 的日常任务会从当前日期
开始逐日积累。当前源没有 TTM 股息率，`dividend_yield` 保持空值，不拿其他口径冒充。

财务指标已回填日线范围内全部 46 个季度报告期。`2026-06-30` 仍处披露期，截至实测日
只有 21 只有数据；每周任务刷新最近 5 个报告期，会继续补充新披露和修订版本。
