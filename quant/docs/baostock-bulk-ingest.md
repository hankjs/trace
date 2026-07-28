# baostock 批量采集方案

> 与限速硬约束配套。权威规则摘要见 [`DATA-ARCHITECTURE.md`](../DATA-ARCHITECTURE.md) §5「baostock 连接与限速」。  
> 官方 API：<https://www.baostock.com/mainContent?file=pythonAPI.md>  
> 本地 SDK 参考：`baostock` 包 `security/history.py`、`demo/demo_daily_k_4_AStock.py`（本环境 `00.9.30`）。

## 1. 为什么要批量

| 约束 | 含义 |
|---|---|
| 每日 API ≤ **5 万**次 | 按**请求次数**计，不是按行数 |
| **禁止并发** | 同出口 IP 多进程/多 shard 会进黑名单（常见 `10001011`） |
| 单票接口 | `query_history_k_data_plus(code, …)` 的 code **只能一只** |

现状盘后对池内每只调 `fetch_daily_bars`（前复权 + 不复权 = **2 次/只**），全市场很容易上千～上万次/日。  
按日全市场接口一天只要 **1～2 次**。

## 2. 接口选型

| 能力 | 优先（批量） | 仅兜底（单条） |
|---|---|---|
| 某日全 A 日 K | `query_daily_history_k_AStock(date)` | `query_history_k_data_plus` |
| 某日全 ETF 日 K | `query_daily_history_k_ETF(date)` | 同上 |
| 某日复权因子 | `query_daily_adjust_factor(date)` | `query_adjust_factor(code, …)` |
| 证券资料 / 日历 / 成分 | 已批量 | — |
| 北交所 | 无 baostock → akshare | — |

启发式：

```text
if 覆盖 ≈ 全日全市场 或 交易日数 ≪ 股票数:
    按日批量
else:  # 单票补洞、重锚全历史
    query_history_k_data_plus
```

## 3. 复权口径（落地前必须 spike）

本系统：

- `open/high/low/close` = **前复权**
- `raw_close` = **不复权**
- `is_st` = 逐日 isST

批量日 K **没有** `adjustflag` 入参（与单票不同），返回列含 `adjustflag`。  
**不能**默认等于现有 `adjustflag=2` + `3` 两次单票结果。

推荐路径：

1. 批量日 K → 对照单票确认是否不复权  
2. 批量或库内权威因子 → 合成前复权  
3. 抽 20 只 × 5 日与 `fetch_daily_bars` 对齐（阈值可参考 `REANCHOR_TOLERANCE`）

## 4. 分场景

### 4.1 盘后增量（P2，收益最大）

| | 现状 | 目标 |
|---|---|---|
| 入口 | `job_daily_bars` → 每只 `ingest_daily` | `ingest_market_day(day)` |
| 请求 | N×2 | **1～2 / 交易日** |

步骤：`fetch_market_daily_bars` →（可选）`fetch_market_adjust_factors` → 换算 → 批量 upsert；分红尺度变化的 code 再 `safe_backfill` 单票全历史。北交所/akshare 对账不变。

### 4.2 复权因子

按交易日 `query_daily_adjust_factor`，不要按 code 扫全表。

### 4.3 多年历史 / is_st 补列

**本仓库不做**——历史数据由外部流程处理，按日历史回填脚本已移除。  
若将来需要：按交易日循环批量约 `年数 × 250` 次，而非 `股票数 × 2`；断点记 `last_done_trade_date`；**禁止**同 IP 多 shard。

### 4.4 仍用单条

Admin 单票回填、`safe_backfill` 重锚、小池短区间补洞。

### 4.5 盘中当日 K（akshare 临时值）

- 盘中每 30 分钟：`stock_zh_a_spot_em` 一次全市场快照 → 拼出截至当前的当日 OHLCV → **只写展示表**（洁净约束见 §7）。
- 盘后 16:30：baostock 权威日 K 无条件覆盖 + 对账告警（接入 §4.1 批量链路后，覆盖为 1～2 次调用）。
- 盘中链路失败（东财限流等）直接跳过本轮，展示层标注快照时间，不补救写。

## 5. 代码落点

| 层 | 内容 | 状态 |
|---|---|---|
| `baostock_client` | `fetch_market_daily_bars` / `fetch_market_adjust_factors`；保留单票 API | **已实现**（P1） |
| `ingest` | `raw_to_qfq`（换算公式单点，待 P0 验证）；`ingest_market_day`；`sync_adjust_factors_for_day`（因子按日） | **已实现**（P2/P3），开关默认关闭，待 P0 spike 验证后开启 |
| `scheduler` | `job_daily_bars` 按 `[quant] bulk_daily_bars` 开关切批量（默认 `false`，走原按 code 路径）；因子按日同步在 `ingest_market_day` 内完成（同一次批量因子请求，不重复调用） | **已实现**，待 P0 spike 验证后开启 |
| 脚本 | `spike_bulk_vs_single.py`（P0 真网对照）；旧按 code 脚本降级/弃用 | spike **已写出，未执行**；按日历史回填脚本已移除（历史数据外部处理，见 §4.3） |

## 6. 实施顺序

| 阶段 | 内容 |
|---|---|
| P0 Spike | 真网 1 日批量 vs 单票对照，写死换算公式 |
| P1 | 客户端封装 + 单测 |
| P2 | 盘后切批量 |
| P3 | 因子按日 |
| P4 | ~~按日历史回填脚本~~ 取消——历史数据由外部流程处理（§4.3） |
| P5 | is_st 并入按日；文档改「已切换」 |
| P6 | 盘中临时当日 K（§4.5，依赖 P2 的覆盖能力） |

## 7. 基础数据洁净（硬约束）

权威存储与临时展示**物理隔离**，akshare/东财/新浪数据永不污染基础表：

1. `quant_daily_bar` / `quant_adjust_factor` 只由 baostock 权威链路写入（盘后批量、单票 `safe_backfill`）。
   唯一例外：北交所日线与因子走新浪源，行内标 `source='sina'`，可识别可剔除。
2. 盘中临时数据（30 分钟刷新的当日 K，见 §4.5）只进 `quant_snapshot` 类展示表，
   仅用于展示与估值；因子、选股、回测**不得消费**。
3. 盘后权威覆盖是**无条件整行 upsert**，不是「只补缺」——防止某天窗盘后残留的盘中半根线被当成完整日 K。
   覆盖后跑差异对账，超阈值告警。
4. 除权日盘中价与库内前复权历史存在口径跳变，属预期，盘后覆盖后自愈。
5. 「展示源 vs 权威源」差异纳入 `app/data/quality.py` 数据质量报表监控。

## 8. 纪律

1. 同出口 IP 禁止并发  
2. 整段任务一次 `login_session`  
3. SOCKS/`run_via_socks.py` 只作**被封后**兜底  
4. 写库批量 upsert；动手前按「交易日数」估请求量 ≤ 5 万  
5. 北交所不进 baostock 批量  
