# 扩展点目录

新增策略时优先复用；缺能力时只扩下列点。

## 1. 操作符（`components` + `spec`）

### 当前白名单（摘要）

| 类 | 操作符 |
|---|---|
| 叶子 | `field`, `literal` |
| 逻辑 | `all`, `any`, `not` |
| 比较 | `gt`, `gte`, `lt`, `lte`, `cross_above`, `cross_below` |
| 算术 | `add`, `subtract`, `multiply`, `divide` |
| 滚动 | `rolling_mean`, `rolling_max`, `rolling_min`, `shift`, `volume_ratio` |
| 指标 | `ma`, `rsi`, `atr`, `momentum`, `return` |
| 组合 | `rank`, `top_n` |

### 扩展规范

- 必须纯函数、确定性、可版本号  
- 禁止读库、读时钟、随机  
- 滚动默认文档化 `shift`，防止含当日前视  
- 类型：bool vs number 在 Expression 层强制  

### 高价值待扩（按验证框架需要，非书名）

| 操作符 | 用途 | 优先级 |
|---|---|---|
| `rolling_std` / 波动率分位 | 波动状态、仓位缩放研究 | 高 |
| `ts_rank`（时序分位） | 横截面以外的强度 | 中 |
| `pct_change` 显式 | 可读性 | 低（可用 return） |
| 行业中性残差 | 需行业面板字段 | 中（先数据） |

## 2. 字段目录（`SUPPORTED_FIELDS`）

### 当前

OHLCV、ST、估值（PE/PB/PS/市值）、基础财务比率（ROE、增长、毛利率、负债、现金流质量）等。

### 扩展规范

- 必须可声明 `availability`：`daily_close` / `daily_open` / `point_in_time`  
- 财务类强制 point-in-time，禁止报告期末偷看  
- 缺数据 → `missing_data`，不静默填 0  

### 高价值待扩

单季扣非、CapEx、存货、应收、有息负债、审计意见日…（见数据缺口文档思路）  
**有字段后才能在 Spec 里写条件，无需新策略代码。**

## 3. 仓位与持有语义（`HoldingSpec` / positioning）

已有：

- single：binary / fixed 目标仓位  
- 加减仓 step / max_position  
- cooldown、risk_reentry  
- portfolio：score + top_n + equal/rank 权重 + 调仓频率  

待扩（需引擎评审）：

- 波动率目标缩放总暴露  
- 行业/单股权重上限更丰富的约束求解  
- 试探仓 + 加码状态机（可用现有 add_rule 部分表达）  

## 4. 覆盖层（`overlays`）

已有：固定比例、ATR 倍数；止损/止盈；trailing 标志。

待扩：时间止损、组合层回撤熔断（研究用）。

## 5. 基线（`validation.baseline_ids`）

已有：`buy_and_hold`、`equal_weight`。

扩展方式：在 `validation.py` 注册可复现、无参数自由的基线，**禁止**把「另一个未冻结策略」静默当基线。

建议后续：

- `ma_trend_60`（简单均线 0/1，规格冻结）  
- `n_day_breakout_20`  

基线本身也应是系统只读 Spec 或内置纯函数。

## 6. 否决规则

- 遗留字符串：`no_net_oos_increment`、`unstable_parameters`、`capacity_failure`  
- 结构化：`metric` / `op` / `threshold` / `segment`  

扩展：新 metric 必须在回测结果中稳定产出，并写文档。

## 7. 实验与多重检验

- `app/experiment/`：冻结 Spec + trials  
- `app/strategy/multiple_testing.py`：族内校正  

扩展：Deflated Sharpe / 区块置换 — 保持「先合成数据校准」。

## 8. 明确非扩展点

| 不要加 | 原因 |
|---|---|
| 每策略一个 Python 模块 | 违背 DB Spec 主路径 |
| 表达式字符串 eval | 安全 |
| 真实下单适配器 | 产品边界 |
| 盘中 tick 操作符 | 频率边界 |
| 不可复现的 AI 黑箱分直连 entry | 无法验证 |

## 9. 版本与兼容

| 版本字段 | 含义 |
|---|---|
| `schema_version` | Spec 结构 |
| `COMPILER_VERSION` | 编译语义 |
| `COMPONENT_VERSIONS[op]` | 单算子实现 |
| `spec_hash` | 规范化 JSON 身份 |
| 回测快照 | 历史证据冻结 |

改算子语义 → 升 component version；旧回测仍看快照。

### 高复用滚动算子（FW-3）

| op | 语义 | 注意 |
|---|---|---|
| `rolling_std` | `shift` 后序列的滚动标准差 | **ddof=0** 写死；默认研究 shift=1 |
| `rolling_rank` | 窗内末值百分位 ∈ (0,1] | 时序，非横截面 `rank` |
| `zscore` | `(x-mean)/std`，std=0→NaN | 与 rolling_std 同口径 |

片段库（`web/src/specSnippets.ts`）与 design_complete 硬清单（`app/strategy/evidence.py`）均不把「配置完成」等同于「已验证可交易」。  
