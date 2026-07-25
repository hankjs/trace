# quant 代码与数据库审查报告

审查日期：2026-07-25 · 范围：`quant/` 全量（约 7200 行 Python + Vue 看板）· 来源：两份独立审查合并去重，全部结论已回读代码核实。

## 结论摘要

系统的架构分层、鉴权模型、交易成本口径和 point-in-time 财报关联都做得比同类项目扎实。但存在 **1 个使用户隔离功能完全失效的 P0**，以及 **4 类会污染回测结论的正确性缺陷**。回测数字目前不可直接用于决策。

按修复优先级：

| # | 级别 | 问题 | 位置 |
|---|------|------|------|
| 1 | P0 | 用户 ID 类型不兼容，隔离功能全线 401 | `app/auth.py:92` |
| 2 | P0 | 回测提前一天建仓，虚增收益 | `app/backtest/engine.py:188` |
| 3 | P1 | 排行榜只返回最后一个策略 | `app/backtest/evaluate.py:136` |
| 4 | P1 | 采集 Session 中毒，一只坏票拖垮整晚 | `app/scheduler.py:60` |
| 5 | P1 | 全市场回填绕过前复权重锚检查 | `scripts/backfill_pool.py:105` |
| 6 | P1 | 调度器与 DDL 缺跨进程互斥 | `app/main.py:29` |
| 7 | P1 | 超卖成交入库但计算时静默截断 | `app/portfolio/trades.py:12` |
| 8 | P1 | 回测记录不足以复现 | `app/backtest/engine.py:393` |
| 9 | P2 | 指标口径：回撤/Sharpe/年化基数 | `app/backtest/engine.py:127` |
| 10 | P2 | 无涨跌停可成交性检查、无整手约束 | `app/backtest/engine.py:193` |
| 11 | P2 | 选股打分无截面标准化 | `app/selection/pipeline.py:37` |
| 12 | P2 | GET screener 全市场 N+1 | `app/selection/screener.py:59` |
| 13 | P2 | 价格/金额用单精度 Float | `app/models.py:54` |
| 14 | P3 | 密码超 72 字节返回 500 | `app/auth.py:33` |
| 15 | P3 | limit 缺下界、无版本化迁移、时区不一致 | 见下文 |

---

## 一、P0：用户隔离功能当前完全不可用

共享 `users.id` 是 `VARCHAR(36)` UUID（`crates/hank-db/src/lib.rs:505`），而所有量化表的 `user_id` 是 `BIGINT`，且 `app/auth.py:92` 强制 `int(claims.get("sub"))`，失败即抛 401。

```python
def user_id_from_claims(claims: dict) -> int:
    try:
        user_id = int(claims.get("sub"))   # sub 是 36 位 UUID → ValueError
    except (TypeError, ValueError) as exc:
        raise HTTPException(401, "登录凭证缺少有效用户标识") from exc
```

真实库中唯一用户的 id 是非数字 UUID，因此**自选股、持仓、结构化选股筛选、保存/读取回测全部返回 401**。`scripts/claim_legacy_user_data.py:22` 同样只接受整数，所以库中现存的 3 条成交和 3 条回测（`user_id IS NULL`）按文档也无法认领。

修复需要一次性数据迁移：`user_id` 改 `VARCHAR(36)`、去掉 `int()` 转换、认领脚本同步改造。注意 `app/auth.py:65` 那句 `verify_sub: False` 的注释声称"历史上是数字 sub"——这个假设本身就是本 bug 的来源，应一并纠正。

> 前一轮审查逐条核对了路由鉴权表与 IDOR（结论是无未授权端点、无越权读取，且有测试覆盖跨用户场景），但只验证了授权**逻辑**，没有验证 id **类型**能否跑通。授权正确而类型不通，表现就是所有人都被拒绝——比越权更难从代码审查中看出来。

## 二、回测正确性

### 2.1 P0：提前一天建仓（`engine.py:188-189`）

```python
if len(idx) and p.reindex(idx).iloc[0] == 1.0 and not x.iloc[0]:
    e.iloc[0] = True  # 起点前已持仓:首日开盘价合成建仓
```

判断的是窗口首日的仓位**水平**，而非"起点之前是否已持仓"。若策略正好在首日由 0 翻 1（该仓位用当天收盘价算出），就用当天开盘价成交了一笔当天才知道的信号。主路径 `_signals_from_positions` 已在 T+1 正确发单，这行是多出来的一笔。

构造数据实测：bar 29 前仓位 0、bar 30 起为 1，bar 30 `open=10, close=20`。窗口起点落在 bar 30 时 `total_return=0.9993`；起点提前则为 `-0.0003`。排行榜每只样本股都受影响。正确条件是判断 `start` **前一根** bar 的仓位。

`scripts/check_no_lookahead.py` 只截断比对 `positions()` 输出，覆盖不到信号→成交的映射，是覆盖盲区而非 bug。组合路径 `_portfolio_sim`（`engine.py:286-291`）先 shift 再截断，是正确的。

### 2.2 P1：保存的回测无法复现（`engine.py:393`）

`BacktestRun`（`models.py:110`）不保存 `costs`；`params` 只存用户显式填写的值，未固化当时实际生效的默认值；`codes` 存的是请求列表而非剔除数据不足标的后的实际样本。默认参数或费率一改，历史结果无法审计或复算。

### 2.3 P2：指标口径

| 项 | 现状 | 问题 |
|---|---|---|
| 年化基数 | 252 | A股约 242。242 根 bar 涨 20% 报 `0.2091` 而非 `0.20` |
| `max_drawdown` | `cummax().clip(lower=initial_cash)` | 是"相对初始资金"回撤而非峰谷回撤。净值 `[0.5, 0.4]` 报 `-0.6`，真实峰谷 `-0.20`，对一开始就亏的策略系统性夸大 |
| `sharpe` | `mean/std*sqrt(252)` | 无无风险利率，实为对零的信息比率 |
| 边界 | 空序列 `IndexError` | 3 根 bar 时年化指数为 252/3，数值无意义 |
| 组合 `win_rate` | 各标的胜率算术平均 | 未按交易数加权 |

### 2.4 P2：撮合真实性

- **无涨跌停可成交性检查**。突破/动量策略恰在大涨次日入场，而那天开盘常一字板买不到。方向固定向上的系统性虚增。
- **无 100 股整手约束**（`init_cash=1.0` + `size_type="percent"`，全程碎股）。
- 停牌 bar reindex 成 NaN 后 vectorbt 直接丢单，信号被静默吞掉而非顺延。
- `engine.py:252-253` `.ffill().fillna(1.0)`：窗口起点无数据的标的被记为净值恰好 1.0 的空仓现金，把等权平均拉向零收益。
- `engine.py:303-314` 印花税用两趟探测，费率钉在首轮的 `(row,col)` 单元上；若改费后订单集变化，税会落在已不是卖出的格子上。影响小但该不变量无测试。

**做对的部分**：六个策略的 `.shift(1)` 全部正确；估值/财报确实按 `available_date` 关联（`screener.py:424,440`，`tests/test_screener.py:98` 有覆盖）；指数成分按 `in_date/out_date` 做了 point-in-time；交易成本（双边佣金、卖出印花税、滑点）都在撮合内扣除。

**残留生存者偏差**：`api/backtest.py:109` 仅在 `codes` 为空时启用 `dynamic_universe`，用户自带代码列表时 `eligibility=None`，全窗口可交易（含入指前）。

## 三、数据层

### 3.1 P1：前复权重锚检测有洞（`ingest.py:96-120`）

思路正确（10 天重叠窗口比对 close），两个洞：

1. `if stored and ...` 把 `stored is None` 与 `0.0` 一并跳过——新股、历史缺口、或首次 ingest 后无重叠行时，直接把新尺度 bar 接到旧尺度历史上，**无任何告警**。
2. **`scripts/backfill_pool.py:105` 全市场回填直接调 `ingest.backfill`，完全绕过重锚检查**；而 `_done_codes`（`backfill_pool.py:40-52`）会把覆盖度看起来完整的代码永久标记 done，尺度错乱的股票再也不会被修，除非手工删表。

后果：单只代码的 `open/high/low/close` 可能横跨两种复权尺度，产生假跳空。下游无人检测——`load_bars_df` 直读、指标与回测直接消费 `close`。`raw_close` 已入库（`ingest.py:63`）但**全代码库无读者**；它不随分红变化，用它反推复权因子远比信任单个重叠 close 稳健。

### 3.2 P1：采集 Session 中毒（`scheduler.py:60-95`）

一个 Session 跑完 800 只股票，逐股 swallow 异常。某只 flush 失败后 Session 进入 `PendingRollbackError`，后续每只都失败，最终 `bars["failed"]`（`scheduler.py:139`）中止整条 pipeline——一只坏票拖垮整晚。且该门限只看异常，返回空 DataFrame 的代码算 succeeded。

### 3.3 P1：调度器与 DDL 缺跨进程互斥（`main.py:29`）

每个 FastAPI 实例都在 lifespan 里 `start_scheduler()`，`fundamentals.py:33` 的 `threading.Lock` 也仅单进程有效。多 worker / 多副本 / 滚动部署会重复抓取与写入：快照无唯一键，选股的 delete/insert 互相竞争，策略评估产出重复批次。启动时 `create_all` + `ALTER TABLE` 同样存在并发 DDL 竞态。建议调度器拆成单实例进程或用 MySQL advisory lock，并改用一次性版本化迁移。

### 3.4 无交易日历

`_is_weekday`（`scheduler.py:44-47`）把节假日当交易日，靠"当日无数据"兜底。兜底不干净：节假日会拿节前旧 bar 与 akshare 对账，刷出成批假告警。baostock 的 `query_trade_dates` 未使用。

### 3.5 其他

- **ST/退市过滤失效**：过滤依赖 `Stock.name` 子串（`pipeline.py:117`、`screener.py:489`），但 `import_stock_list`（`ingest.py:41-44`）只 insert 不 update，初次导入后改名为 `*ST` 的股票永不被过滤，退市也不标记。
- **baostock 会话不可重入**：`scripts/backfill_universe.py:31` 外层开 `login_session()`，`universe.sync_all_indices`（`universe.py:63`）内层再开一个，内层 `finally` 会 logout 并置 `_logged_in = False`，外层剩余工作在未登录状态下跑。`own_login = not _logged_in`（`baostock_client.py:54`）在锁外读取，存在竞态。锁又横跨整个 fetch（55-77 行），并发的 admin backfill 会被整段串行阻塞。
- **数据校验薄**：`upsert_bars` 只丢 NaN close（`ingest.py:52`），零/负价、`high < low`、零成交量停牌行照存。回测不过滤停牌，停牌日以 0% 收益进入序列。
- **量比分母未防零**：`indicators/__init__.py:50`，停牌股得 `inf`，能通过 NaN 过滤并满足任何 `vol_ratio5 >=` 条件。
- **时区不一致**：scheduler 用 `Asia/Shanghai`，但 `ingest.py:86,128,180` 与快照时间戳用裸 `date.today()`/`datetime.now()`（服务器本地）。非 CST 主机上 15:00 盘中门限与快照时间戳对不上，`cleanup_snapshots` 保留窗口切错。
- `sync_index_members` 末尾统一 commit（`universe.py:54`）而 `upsert_stock` 逐只 commit（`ingest.py:32`），中途失败会留下有股票无成分记录。

## 四、账本与选股

### 4.1 P1：超卖成交入库但计算时静默截断

`trades.py:12` 只校验价格和数量为正；`positions.py:55` 遇超卖用 `min(卖出量, 持仓量)` 截断。持有 100 股却录入卖出 200 股，**库里保留 200 股成交，盈亏只按 100 股算**，两边永久不一致。应在写入事务中校验整条时间序列、拒绝负持仓，或明确实现做空账本。对零持仓的卖出更彻底：`avg=0.0, sell_qty=0`，只留一行日志，录错的数据凭空消失。

其余账目正确：移动平均成本法一致应用，买入费用计入成本，卖出费用冲减已实现，成本按比例结转，`total_realized_pnl` 正确含已平仓。

### 4.2 P1：排行榜只返回最后一个策略（`evaluate.py:136-144`）

`run_at` 是 `default=datetime.now`，在每个对象**实例化**时求值，而循环每次迭代要跑完整回测（数分钟）。十几行的 `run_at` 相差数分钟，`leaderboard` 用 `WHERE run_at == latest_run` 精确相等，只捞回最后一条。

内存 SQLite 复现中同批 3 条相差数微秒即已分裂；当前 MySQL 因 `DATETIME` 秒级精度**碰巧**把每批归到一起，多进程并发时仍会混批。应加显式 `batch_id`，而不是靠时间戳隐式表示批次。

### 4.3 P2：选股打分无截面标准化（`pipeline.py:37-49`）

直接对原始因子加权求和：`mom20` 与 `mom60` 量纲不同，无去极值、无排序、无行业/市值中性化，`vol_ratio5` 加成上限 0.06 与真实动量价差同量级，高换手股可以压过动量更强的股。任一因子为 NaN 直接丢弃该股（`return None`），静默缩小池子。

### 4.4 P2：GET screener 全市场 N+1（`screener.py:59-66`）

取当日全部因子行后逐只 `load_bars_df`，全市场约 5000 次往返，每次扫约 150 个交易日。`structured_screen` 已改为单次批量 `IN`（`screener.py:454-460`），GET 路径未改。同函数 `screener.py:96-98` 先截断再算 `total`，前端显示"共 100 条"而实际匹配 500 条。

## 五、数据库结构

- **P2 价格/金额用 `Float`**（`models.py:54-60,104-106`）：MySQL `Float` 单精度约 7 位有效数字，`positions.py` 累加 `price*qty + fee` 再除法求均价，六位数持仓的精度损失超过展示用的 `round(4)`。应改 `DECIMAL` 或至少 `Double`。这也让 `ingest.py:112` 的 0.001 相对阈值接近 float32 噪声。
- **`quant_daily_bar` 聚簇错**（`models.py:51`）：代理 BigInteger 主键使行按插入顺序聚簇而非 `(code,date)`，每次区间扫描都是二级索引 + 随机回表；`ix_quant_daily_bar_code` 与唯一键前缀完全冗余，在千万行表上纯粹拖慢写入。改 `(code, date)` 自然复合主键更合适。
- **无版本化迁移**：`create_all` 从不 ALTER 既有表。`schema.py` 手写 ALTER 与 models 声明目前一致（索引名、约束名已逐一比对），但今后任何对既有表的模型改动都会被静默忽略。建议上 Alembic。
- **`user_id` 可空**：所有查询都正确按 `user_id` 过滤，无越权读取；但 `NULL = ?` 在 SQL 中为 unknown，遗留行对任何用户都不可见，需手工跑认领脚本且启动无提示。认领后应改 `NOT NULL`。
- 主键类型不一致（`models.py:115,170,194,228` 用 `Integer`，其余 `BigInteger`）；`ValuationSnapshot` 按全市场日频最终会超 21 亿。
- `JSON` 列存 `per_code` 指标（`evaluate.py:89`）单行可达 MB，读时才过滤（`evaluate.py:158`），仍要从磁盘取出。
- 连接池 `5+5`（`db.py:18-19`）偏小：调度器任务整段持有 session 数分钟，`/api/backtest/sweep`（`api/backtest.py:93`）允许任意客户端同步触发多达 200 次完整回测，会耗尽连接池。

## 六、API 与前端

这块整体最干净。核对完整路由表：除 `login`/`health` 外全部有 `require_user`/`require_client`/`require_admin`，**无未授权端点**；无 IDOR（backtest/trades/watchlist 均过滤 `user_id`，`tests/test_api_metadata.py:198-244` 覆盖跨用户场景）；无 SQL 注入（唯一裸 SQL 在 `auth.py:24-30`，参数绑定正确，screener 字段白名单）；无 `v-html`；JWT 校验签名与 exp，密钥缺失时 import 期即抛错，无不安全默认值。重活是同步 `def`，Starlette 丢线程池执行，admin 侧正确用 `run_in_executor`——不存在阻塞事件循环的问题。

真实缺陷：

1. **P3 密码超 72 字节返回 500 而非 401**（`auth.py:33`）：锁定的 bcrypt 5.0.0 对超长密码**抛 `ValueError`** 而非截断；Rust 侧 `bcrypt::verify(...).unwrap_or(false)`（`server/src/routes.rs:39`）会截断，两边行为不一致，且未认证请求即可刷出堆栈。
2. **P3 `limit` 只有 `le` 无 `ge`**（`selection.py:101`、`signals.py:21`），负值渲染成 `LIMIT -1`；`high_window` 无上界并直接进 `timedelta`。`/api/market/kline` 无 code 格式校验、无日期上界，直接倒全量历史。
3. **P3 前端静默降级**（`web/src/api.ts:488-521`）：404/405 时回退旧版 GET screener，**丢掉 OR 逻辑和所有基本面条件**，用户看到的结果与所设筛选不符且无提示。
4. JWT TTL 30 天且无刷新/吊销机制，token 存 localStorage；未知用户名提前返回构成时序枚举（`auth.py:31-32`）；无登录限流；`/api/auth/me` 直接 `claims["username"]`，缺该 claim 的 token 会 500。
5. `api/admin.py` 八处 `except Exception as e` 把原始驱动/网络异常文本回给客户端（仅管理员可达）。

## 七、生产库现状（只读核查）

- 约 **748 万条日线**，无空 OHLC、无非法高低价、无回测净值孤儿记录。
- 当前 **800 只有效股票池**与 800 条当日因子一致。
- **估值表与财务快照表均为空** —— 相关筛选字段目前无覆盖数据，基于基本面的结构化筛选实际筛不出东西。这一点尤其值得注意：`available_date` 的 point-in-time 关联逻辑写得正确且有测试，但还没有数据经过它。
- 3 条成交 + 3 条回测仍为 `user_id IS NULL`，受问题 1 阻塞无法认领。

## 建议修复顺序

1. **问题 1**（UUID 类型迁移）—— 用户隔离功能目前是坏的，且需要数据迁移，宜最先做。
2. **问题 2**（提前建仓）—— 污染所有回测与排行榜数字。
3. **问题 3、4**（排行榜混批、Session 中毒）—— 功能实际不工作。
4. **问题 5、6**（重锚绕过、跨进程互斥）—— 会静默腐蚀已入库的价格序列。
5. **问题 7、8、9、10**（账本一致性、回测可复现、指标口径、涨跌停）。
6. 其余性能与健壮性项。

问题 1 与 6 都涉及 schema 变更，建议合并为一次版本化迁移（引入 Alembic），同时把 `user_id` 改 `VARCHAR(36) NOT NULL`、价格列改 `DECIMAL`、`quant_daily_bar` 换自然主键。
