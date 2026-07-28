# 需求文档：策略验证框架能力补强（四项）

| 属性 | 内容 |
|---|---|
| 文档 ID | `REQ-FW-CAP-2026-07` |
| 状态 | 待评审 |
| 范围 | Trace Quant 策略验证框架（前端配置 + Spec + 回测 + 实验） |
| 产品边界 | A 股日频研究与模拟回测；不连接券商；不下单；不输出真实仓位指令 |
| 关联文档 | `docs/summry/00-framework-overview.md`～`06-gap-and-roadmap.md`；`docs/research/strategy-framework.md` |
| 目标读者 | 产品/研发；可直接拆任务与验收 |

---

## 1. 背景与问题

策略验证框架主链已通：

```text
StrategySpec（DB）→ 能力解析 → 编译 → T+1 回测 → validation → Experiment trials → evidence_status
```

但实际使用中有四类摩擦：

| # | 问题 | 现象 |
|---|---|---|
| 1 | 配置成本高 | 常用「突破 / 均线交叉 / 通道退出」仍要从零搭 AST 树 |
| 2 | 实验难对比 | Experiments 页仅列表展示收益/回撤，难看参数平台、排序、相对最优 |
| 3 | 表达力缺口 | 缺高复用数值算子（如滚动标准差），波动状态类 Spec 写不出或只能硬凑 |
| 4 | 设计完成门槛虚 | `mark_design_complete` 几乎只要求规格可解析；假说/基线/否决「写了但无效」也能过 |

本需求只解决上述四点，**不**搬书中策略、不扩交易能力、不为每个策略写 Python 模板。

---

## 2. 目标与非目标

### 2.1 目标

1. 用户能在 1 分钟内从片段生成可校验的进场/离场表达式草稿，再微调参数。  
2. 同一实验内多 trial 可并排对比核心指标，支持排序、高亮最优、识别参数敏感尖点。  
3. 新增 2～3 个确定性、无前视默认的高复用算子，前后端与测试对齐。  
4. `design_complete` 必须通过可机器检查的硬清单；清单未满足时禁止状态推进并给出字段级原因。

### 2.2 非目标

- 不提供「一键导入本书全部策略」。  
- 不实现拖拽式可视化策略画布（片段库 + 现有 AST 编辑器即可）。  
- 不新增第二套回测撮合语义。  
- 不在本迭代做完整 FDR/Deflated Sharpe UI（可展示已有 multiplicity 摘要即可）。  
- 不扩展期权/高频/自动交易。  
- 不为片段结果自动标记 `oos_passed` 或「已验证」。

### 2.3 成功度量（发布后 2 周内可观测）

| 指标 | 目标 |
|---|---|
| 使用片段创建的新策略占比 | ≥ 50%（日志或前端事件可选） |
| 单次配置到首次 `validate=supported` 中位时间 | 主观验收：比纯手搭明显缩短 |
| 实验页可对 ≥10 个 trial 完成对比操作 | 无前端卡顿不可用 |
| 含 `rolling_std` 等新算子的规格通过 validate | 100% 与后端一致 |
| 空假说/空否决/未锁 OOS 标记 design_complete | **0 次成功**（API 返回 4xx） |

---

## 3. 需求项总览

| ID | 名称 | 优先级 | 依赖 |
|---|---|---|---|
| **FW-1** | 常用 Spec 片段库 | P0 | 无 |
| **FW-2** | 实验对比 UI | P0 | 现有 ExperimentTrial.metrics_summary（可扩展字段） |
| **FW-3** | 高复用算子（2～3 个） | P0 | 无（可与 FW-1 并行） |
| **FW-4** | design_complete 硬清单 | P0 | 无（可与 FW-1 并行） |

建议实现顺序：**FW-4 → FW-3 → FW-1 → FW-2**  
（先堵住证据注水，再扩表达力，再降配置成本，最后增强对比分析。）

---

## 4. FW-1 常用 Spec 片段库

### 4.1 用户故事

> 作为研究者，我希望从「突破进场」「双均线」「通道离场」等片段一键插入表达式，  
> 只改窗口/阈值，而不是每次从 `gt`/`rolling_max` 搭树，以便快速做消融副本。

### 4.2 概念定义

| 术语 | 定义 |
|---|---|
| **片段 (Snippet)** | 预定义的、合法的 `StrategyAstNode` 子树，可带参数占位 |
| **片段参数** | 插入前可编辑的标量（window、threshold 等），写入 AST 叶子 |
| **应用目标** | 策略表单中的某一表达式槽：`entry` / `native_exit` / `add` / `reduce` / `score` / `risk_filter` |

片段 **不是** 完整 StrategySpec，也 **不是** 系统策略种子；插入后仍是用户可编辑草稿。

### 4.3 功能需求

#### FW-1.1 片段目录（首批内置）

系统内置只读片段，**版本化**，中英文 ID 稳定。首批最少包含：

| snippet_id | 名称 | 结果类型 | 适用槽 | 语义（简述） |
|---|---|---|---|---|
| `entry_breakout_n` | N 日新高突破 | bool | entry | `close > rolling_max(high, N, shift=1)` |
| `entry_breakout_vol` | 突破 + 量比 | bool | entry | 上式 AND `volume_ratio(volume,M,shift=1) > thr` |
| `entry_ma_cross_up` | 双均线上穿 | bool | entry | `cross_above(ma(close,fast), ma(close,slow))` |
| `entry_close_above_ma` | 收盘站上均线 | bool | entry | `close > ma(close, N)` |
| `exit_channel_low` | N 日低点通道离场 | bool | exit | `close < rolling_min(low, N, shift=1)` |
| `exit_ma_cross_down` | 双均线下穿离场 | bool | exit | `cross_below(ma(close,fast), ma(close,slow))` |
| `exit_close_below_ma` | 收盘跌破均线 | bool | exit | `close < ma(close, N)` |
| `score_momentum_n` | N 日动量评分 | number | score | `momentum(close, N)` 或等价 `return` |
| `filter_rsi_oversold_recover` | RSI 超卖后恢复 | bool | entry / risk_filter | RSI 自下上穿阈值（参数化） |

默认参数示例（可调，非真理）：

- 突破：`N=20`，量比窗 `M=20`，`thr=1.5`  
- 均线：`fast=10`，`slow=60`  
- 通道离场：`N=10`  
- RSI：`period=14`，`level=30`

> 片段注释必须标明：**未验证，仅降低配置成本**。

#### FW-1.2 片段数据结构

建议前端常量模块 + 可选后端只读 API（二选一；若仅前端，须与后端算子集同步测试）。

```ts
interface SpecSnippet {
  id: string
  name: string
  description: string
  resultType: 'bool' | 'number'
  /** 允许应用的槽位 */
  targets: Array<'entry' | 'exit' | 'add' | 'reduce' | 'score' | 'risk_filter'>
  /** 仅 single / portfolio / both */
  kind: 'single' | 'portfolio' | 'both'
  params: Array<{
    key: string
    label: string
    type: 'int' | 'float'
    default: number
    min?: number
    max?: number
  }>
  /** 根据 params 生成 AST */
  build(params: Record<string, number>): StrategyAstNode
  /** 插入后建议声明的 data_requirements 字段 */
  suggestedFields: string[]
}
```

#### FW-1.3 UI 行为

1. 在 `StrategySpecEditor` 中，每个表达式区域旁增加 **「插入片段」**。  
2. 弹层：按目标槽过滤列表 → 选片段 → 调参数 → 预览 JSON/结构化摘要 → 确认。  
3. 确认后 **整槽替换** 当前表达式（二次确认若当前非默认占位）。  
4. 自动合并 `suggestedFields` 到 `data_requirements`（required=true，不删已有项）。  
5. 不自动改 `overlays` / `validation`（避免静默改变验证设计）。  
6. 系统策略只读时隐藏插入按钮。

#### FW-1.4 与消融工作流

- 「复制策略」后，用户可对副本只换片段参数（如 N=20→40），形成消融。  
- 不强制创建 Experiment；可选后续迭代「从片段参数生成 parameter_scans」。

#### FW-1.5 非功能

- 片段 build 结果必须通过 `/api/strategies/validate`（单测批量校验）。  
- 禁止片段引入未注册算子。  
- 中文 UI；snippet_id 稳定英文。

### 4.4 验收标准（FW-1）

| # | 标准 |
|---|---|
| A1 | 首批 ≥ 8 个片段，覆盖 entry/exit/score 至少各 1 个 |
| A2 | 每个片段 build + 最小 Spec 壳通过后端 validate，`capability.supported` |
| A3 | 策略页可插入片段并保存；刷新后 AST 与参数一致 |
| A4 | 插入后缺失字段可自动补全；未声明 required 字段时 validate 仍按原规则失败 |
| A5 | 文档/UI 标明片段未验证 |
| A6 | 单元测试：参数边界（min/max）、快慢均线顺序（fast&lt;slow 校验或自动交换策略写明） |

---

## 5. FW-2 更强的实验对比 UI

### 5.1 用户故事

> 作为研究者，我在一个实验里跑了多组参数 trial，希望在一张表里对比收益、回撤、夏普、交易次数与参数补丁，  
> 按指标排序、标记最优，并一眼看出「只有尖点参数好看」的不稳定情况。

### 5.2 现状差距

当前 `Experiments.vue`：

- 仅展示 `trial_index / outcome / backtest_run_id / total_return / max_drawdown / error`  
- 单次手动填 `paramPath` + `paramValue`  
- 有 multiplicity 摘要但无表格级对比  

`metrics_summary` 已含：`total_return, annual_return, max_drawdown, sharpe, win_rate, trade_count, round_trips`。

### 5.3 功能需求

#### FW-2.1 对比表（主界面）

在实验详情中，将 trial 列表升级为 **对比表**：

| 列 | 来源 | 说明 |
|---|---|---|
| # | trial_index | 固定 |
| 结果 | outcome | 完成/无交易/失败/否决 |
| 参数摘要 | param_patch | 格式化为 `path=value` 多行或 chip |
| 总收益 | metrics_summary | 可排序 |
| 年化 | metrics_summary | 可排序 |
| 最大回撤 | metrics_summary | 可排序（默认越接近 0 越好，排序方向可切换） |
| 夏普 | metrics_summary | 可排序 |
| 胜率 | metrics_summary | 可选列 |
| 交易数 / 往返 | metrics_summary | 可选列 |
| 回测 ID | backtest_run_id | 可点击跳转回测详情（若路由支持） |
| 错误 | error | 截断+title |

要求：

1. 列显示开关（本地偏好即可，localStorage）。  
2. 单击表头排序；稳定排序（次键 trial_index）。  
3. `outcome !== ok` 的行视觉弱化，但 **不得从对比集删除**（失败也是证据）。  
4. 空指标显示 `—`，排序时沉底。

#### FW-2.2 最优高亮与基线行

1. 用户选择「优化目标」：默认 `sharpe`，可选 `annual_return` / `total_return` / `calmar`（若无 calmar 则用 annual/|max_dd| 前端计算，分母为 0 时跳过）。  
2. 在 `outcome=ok` 且目标指标非空的子集中高亮最优行。  
3. 若存在 `param_patch` 为空的 trial，标记为 **基准 trial**（可选自动在首次创建时跑一发空 patch——本需求不强制自动跑，仅 UI 识别）。

#### FW-2.3 参数维度展开

当多个 trial 的 `param_patch` 键集合为 1～2 个路径时：

1. 将路径拆成独立列（如 `window`、`threshold`），便于扫参数平台。  
2. 键 &gt; 2 时保持 JSON/摘要，避免表过宽。

#### FW-2.4 批量 trial（降低对比样本成本）

在详情区增加「按声明扫描跑批」入口（二期可做；**本期最小集**）：

**本期必须：**

- 支持一次提交 **多个** param_patch（JSON 数组，最多 32 项，与后端 `MAX_PARAMETER_COMBINATIONS` 精神一致）。  
- API：扩展现有 create trial 或新增 `POST /api/experiments/{id}/trials/batch`。  
- 顺序执行或有限并发（建议顺序，避免压垮回测队列）；失败单项记 error，不中断整批。

**本期可选 / 下期：**

- 一键读取策略 `validation.parameter_scans` 笛卡尔积（需检查组合数上限）。  
- 导出 CSV。

#### FW-2.5 对比摘要卡

表上方固定摘要：

- trial 总数 / ok / no_trades / error / rejected  
- 最优目标指标值与对应 trial_index、param_patch  
- 若 ok≥3：目标指标的 min / median / max（仅 ok 子集）  
- 复用已有 `multiplicity` 文案（数据挖掘提示）

#### FW-2.6 后端补充（最小）

| 项 | 要求 |
|---|---|
| metrics_summary | 确保上述键稳定存在（null 允许） |
| validation 摘要 | **可选**：`metrics_summary` 增加 `oos_annual_return`、`rejection_verdict` 便于 OOS 对比；若成本高可二期 |
| 列表性能 | 单实验 256 trial 内详情一次返回可接受；超出需分页（当前上限可写死拒绝过多 batch） |

### 5.4 验收标准（FW-2）

| # | 标准 |
|---|---|
| B1 | ≥5 个 ok trial 时，可按夏普/年化/回撤排序且高亮最优 |
| B2 | param_patch 在表中人类可读；单键/双键时有独立列 |
| B3 | error / no_trades 行仍可见且不参与「最优」 |
| B4 | 摘要卡数字与表格一致 |
| B5 | batch 创建 10 个 trial：全部落库，含失败项 |
| B6 | 前端 Vitest：排序、最优选择、param 列展开纯函数 |

---

## 6. FW-3 高复用算子（2～3 个）

### 6.1 用户故事

> 作为研究者，我需要用滚动波动、滚动分位等通用数值变换写过滤器，  
> 且保证与 `rolling_mean` 一样无前视、可哈希、可回测。

### 6.2 选定算子（本需求锁定）

| op | 结果类型 | 形状 | 语义 | 典型用途 |
|---|---|---|---|---|
| **`rolling_std`** | number | `input, window, shift` | 对 `shift` 后序列做样本标准差（`ddof=0` 或 `1` 必须文档固定，建议 **ddof=0** 与波动率实务一致并写死） | 波动过滤、vol 状态 |
| **`rolling_rank`** | number | `input, window, shift` | 时序窗口内当前值（shift 后末值）的百分位排名，输出 ∈ (0,1]；窗口不足为 NaN | 相对历史高低位 |
| **`zscore`** | number | `input, window, shift` | `(x - rolling_mean) / rolling_std`，std=0 时 NaN | 偏离均值的标准化 |

说明：

- 三者均 **number**，可与 `gt`/`lt` 组合为 bool 条件。  
- **不**引入横截面 `rank` 语义冲突：`rolling_rank` 为时序；现有 `rank`/`top_n` 仍为组合横截面。  
- 若实现量必须砍到 2 个：保留 `rolling_std` + `zscore`（`zscore` 可内部复用 std/mean，少一个独立用户心智负担时可只暴露 std+mean 让用户手写；但需求要求「2～3 个」，推荐三个都做且 zscore 为语法糖）。

### 6.3 语义与防前视

与现有 `rolling_mean` 对齐：

```text
history = shift(input, shift)
rolling_std = history.rolling(window).std(ddof=DDOF)
```

- `window`：2～500  
- `shift`：0～500；**片段与文档默认 shift=1**（信号用收盘后可知历史）  
- `shift=0` 允许但 catalog 警告「可能含当日，仅当研究明确需要」  
- NaN 传播：与 pandas 滚动一致；比较算子对 NaN 为 False（与现网行为一致，回归测试锁定）

### 6.4 改动面（必须同步）

| 层 | 文件/位置 | 改动 |
|---|---|---|
| Spec 白名单 | `app/strategy/spec.py` | `SUPPORTED_OPERATORS`、`_OP_FIELDS`、类型检查 |
| 实现 | `app/strategy/components.py` | `evaluate_expression` + `COMPONENT_VERSIONS` |
| 测试 | `tests/test_strategy_spec.py` 等 | 形状、类型、无前视、ddof、除零/零方差 |
| 前端注册表 | `web/src/specExpression.ts` | `EXPRESSION_OPS` 标签与 defaults |
| 编辑器 | 自动吃注册表则无需大改 | 回归 `SpecExpressionEditor.spec.ts` |
| 研究词典 | catalog | 算子中文名与说明 |
| 片段（可选） | FW-1 | 如 `filter_high_vol`：`rolling_std(close,20,1) > literal` |

**禁止** 为演示写 `strategies/vol_filter.py`。

### 6.5 版本

- 新算子写入 `COMPONENT_VERSIONS`，建议统一 `strategy-components-v1` 或 bump 为 `v2` 若担心旧语义；**新 op 不影响旧哈希**。  
- `COMPILER_VERSION` 仅在编译生命周期语义变时 bump（本需求通常不必）。

### 6.6 验收标准（FW-3）

| # | 标准 |
|---|---|
| C1 | 三算子（或批准的两个）出现在前后端白名单，非法字段被拒 |
| C2 | 合成序列单测：固定输入下输出与参考 pandas 一致 |
| C3 | `shift=1` 时第 t 日不依赖 t 的 input 实现细节有断言 |
| C4 | 含新算子的 Spec 可完整跑通 compile + 回测（内存 SQLite 测试） |
| C5 | 前端可选中该算子并保存策略 |
| C6 | catalog / 文档说明 ddof、NaN、默认 shift |

---

## 7. FW-4 design_complete 硬清单

### 7.1 用户故事

> 作为研究者，我只有在假说、对照基线、否决条件与样本外设计都写清楚后，  
> 才能把策略标成「验证设计完成」，避免空壳规格伪装成可验证研究。

### 7.2 现状

- `MetadataSpec.hypothesis`：`min_length=1`（过短即可过）  
- `ValidationSpec.baseline_ids`：`min_length=1`  
- `ValidationSpec.rejection_criteria`：`min_length=1`  
- `locked_oos`：bool，**可为 false**  
- `apply_manual_action(mark_design_complete)`：仅检查状态为 `unverified` + 规格可解析  

因此存在：**合法但无研究意义** 的规格（短假说、占位否决字符串、不锁 OOS）仍可标记 design_complete。

### 7.3 硬清单定义（机器可判定）

进入 `design_complete` 必须 **全部** 满足：

| 检查项 ID | 规则 | 失败 code |
|---|---|---|
| `HYP_LEN` | `hypothesis` 去空白后长度 ≥ 20，且 ≤ 1000 | `hypothesis_too_short` |
| `HYP_PLACEHOLDER` | 不得整句等于黑名单占位（如 `todo`/`测试`/`TBD`/`占位`，大小写不敏感） | `hypothesis_placeholder` |
| `BASELINE_KNOWN` | `baseline_ids` 非空，且每个 id ∈ 服务端已知基线集合（当前：`buy_and_hold`, `equal_weight`；未知则失败） | `baseline_unknown` |
| `BASELINE_MIN` | 至少 1 个基线（已有）；**推荐**至少与「买入持有」相关——不强制第二基线 | — |
| `REJECT_NONEMPTY` | `rejection_criteria` 去空白后每项非空，且 ∈ 已知遗留集合 **或** `rejection_rules` 长度 ≥ 1 且通过 RejectionRuleSpec | `rejection_missing` |
| `REJECT_KNOWN` | 字符串 criteria 必须属于 `_LEGACY_CRITERIA` 或文档注册表；禁止随意字符串凑数 | `rejection_unknown` |
| `LOCKED_OOS` | `validation.locked_oos === true` | `oos_not_locked` |
| `NATIVE_EXIT` | `kind=single` 时 `native_exit` 非 null；`portfolio` 时 positioning 完整（已有规格层） | `native_exit_missing` |
| `CAPABILITY` | `resolve_capabilities` 为 `supported`（缺数据不得标设计完成） | `capability_not_supported` |
| `SOURCE_OPTIONAL` | `sources` 已有 min_length=1；保持 | — |

**说明：**

- 硬清单用于 **`mark_design_complete` 与 `reset_rejected → design_complete`**，不在每次普通 `validate` 上强制 `locked_oos`（允许草稿规格 `locked_oos=false` 先回测探索）。  
- 可选开关：`POST /validate` 增加 `?mode=design_complete` 或 body `check_design_gate: true`，返回清单逐项结果，供 UI 勾选展示。

### 7.4 API 与状态机

1. `POST /api/strategies/{id}/evidence` action=`mark_design_complete`：  
   - 跑硬清单；失败 → **400**，body：

```json
{
  "error": "design_complete_checklist_failed",
  "checks": [
    {"id": "HYP_LEN", "ok": false, "code": "hypothesis_too_short", "message": "假说至少 20 字"},
    {"id": "LOCKED_OOS", "ok": true, "code": null, "message": "已锁定样本外"}
  ]
}
```

2. 全部 ok 才写入 `evidence_status=design_complete`。  
3. `reset_rejected` 复位到 design_complete 时 **同样** 跑清单（规格可能已被改坏）。  
4. 自动推进 `design_complete → backtested` 逻辑不变，但起点必须是真正通过清单的状态。

### 7.5 UI 需求

在策略页证据区域：

1. 展示 **验证设计清单** 列表（绿/红）。  
2. 「标记为验证设计完成」按钮：未全绿时 disabled，或点击后展示失败项。  
3. 点击失败项可 scroll 到对应表单字段（hypothesis / validation / exit）。  
4. 文案：通过清单 ≠ 策略有效，仅表示「可以开始严肃回测证据链」。

### 7.6 系统种子策略

六个系统预设：

- 若已满足清单，保持可只读展示 design 相关字段。  
- 若历史数据 `locked_oos=false` 或否决为弱占位，**迁移或 presets 更新** 使其满足清单，避免公共策略无法作为「设计完成」范例。  
- 不要求系统策略默认 `evidence_status=design_complete`（公共模板可为 unverified）。

### 7.7 验收标准（FW-4）

| # | 标准 |
|---|---|
| D1 | hypothesis 19 字 → mark 失败；20 字合法假说+其余满足 → 成功 |
| D2 | `locked_oos=false` → mark 失败 |
| D3 | `rejection_criteria=["foo"]` → mark 失败 |
| D4 | 未知 baseline_id → mark 失败 |
| D5 | capability=missing_data → mark 失败 |
| D6 | UI 清单与 API checks 一致 |
| D7 | 普通 validate 仍允许草稿（oos false）保存策略 |
| D8 | pytest 覆盖上述失败/成功矩阵 |

---

## 8. 跨需求交互

| 交互 | 处理 |
|---|---|
| FW-1 片段默认 validation | 不自动写入否决/基线；用户仍靠 FW-4 补全 |
| FW-1 + FW-3 | 片段可选用 `rolling_std`（算子合并后加可选片段） |
| FW-2 + FW-4 | design_complete 后创建的 experiment 更可信；不强制绑定 |
| FW-3 文档 | 研究词典与 `03-extension-points.md` 同步 |

---

## 9. 数据与安全

- 片段与算子不含用户代码执行；仍走白名单 AST。  
- 实验 batch 限制：单请求 ≤ 32 trials；单实验建议软上限 256 trials。  
- 不新增外网依赖。  
- 审计：evidence 状态变更可继续打日志（若已有）；清单失败可不落库。

---

## 10. 测试计划

| 层级 | 内容 |
|---|---|
| 后端单元 | 新算子数值；design_complete checklist；batch trial |
| 后端 API | strategies evidence 400/200；experiments batch |
| 前端单元 | 片段 build；对比表排序/最优；清单展示映射 |
| 回归 | 六系统 presets hash（若 presets 为满足 FW-4 变更，更新金样并说明） |
| 手工 | 策略页走通：插片段 → 补清单 → mark design_complete → 建实验 → batch trial → 对比排序 |

---

## 11. 里程碑建议

| 里程碑 | 交付 | 预估 |
|---|---|---|
| M1 | FW-4 后端清单 + API + 策略页清单 UI | 2～3 天 |
| M2 | FW-3 三算子全链路 + 测试 | 2～3 天 |
| M3 | FW-1 片段库 ≥8 + 插入 UI + 测试 | 3～4 天 |
| M4 | FW-2 对比表 + 摘要 + batch API/UI | 3～5 天 |
| M5 | 文档更新（summry/catalog）+ 手工验收 | 1 天 |

合计约 **2～3 周**（单人兼职按日历拉长）。

---

## 12. 开放问题（评审时拍板）

| # | 问题 | 选项 | 建议 |
|---|---|---|---|
| Q1 | 片段是否需要后端 API？ | 仅前端常量 / 后端 catalog | 先前端常量 + 契约测试；catalog 二期 |
| Q2 | `rolling_std` 的 ddof？ | 0 / 1 | **0**，文档写死 |
| Q3 | design_complete 是否强制 `rejection_rules` 结构化？ | 仅 legacy 字符串 / 必须结构化 | **legacy 白名单或结构化二选一即可** |
| Q4 | batch trial 是否读 `parameter_scans`？ | 本期 / 下期 | **下期**；本期手写数组 |
| Q5 | 对比表是否必须 OOS 指标列？ | 本期 / 下期 | **下期**；有则加分 |

---

## 13. 验收总清单（发布门禁）

- [ ] FW-4：空壳规格无法 mark_design_complete  
- [ ] FW-3：新算子可配置、可回测、有单测  
- [ ] FW-1：≥8 片段可插入并 validate  
- [ ] FW-2：多 trial 排序、高亮、摘要、batch  
- [ ] 产品文案无「已验证可交易」暗示  
- [ ] `uv run pytest` 相关用例通过  
- [ ] `web` 相关 vitest / 类型检查通过  
- [ ] `docs/summry/06-gap-and-roadmap.md` 勾选对应缺口为「进行中/已完成」  

---

## 14. 附录：design_complete 检查伪代码

```python
def design_complete_checks(spec: StrategySpec) -> list[Check]:
    checks = []
    h = spec.metadata.hypothesis.strip()
    checks.append(Check("HYP_LEN", len(h) >= 20))
    checks.append(Check("HYP_PLACEHOLDER", h.lower() not in PLACEHOLDERS and h not in CN_PLACEHOLDERS))
    checks.append(Check("BASELINE_KNOWN", all(b in KNOWN_BASELINES for b in spec.validation.baseline_ids)))
    legacy_ok = bool(spec.validation.rejection_criteria) and all(
        c.strip() in LEGACY_CRITERIA for c in spec.validation.rejection_criteria
    )
    rules_ok = len(spec.validation.rejection_rules) >= 1
    checks.append(Check("REJECT_NONEMPTY", legacy_ok or rules_ok))
    checks.append(Check("LOCKED_OOS", spec.validation.locked_oos is True))
    if spec.kind == "single":
        checks.append(Check("NATIVE_EXIT", spec.native_exit is not None))
    cap = resolve_capabilities(spec)
    checks.append(Check("CAPABILITY", cap.status == CapabilityStatus.SUPPORTED))
    return checks
```

---

**文档结束。** 评审通过后可按 M1→M5 拆 issue 实现。
