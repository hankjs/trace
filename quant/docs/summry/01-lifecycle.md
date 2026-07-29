# 策略验证生命周期

## 1. 状态机（证据）

```text
unverified
    │  人工：验证设计完成（假说/基线/否决条件写清）
    ▼
design_complete
    │  回测跑完（样本内有结果）
    ▼
backtested
    │  validation.locked_oos + 否决规则全部通过
    ▼
oos_passed  ──►  仍只是研究结论，≠ 交易建议
    │
    └── 任一否决命中 / 人工否决 ──► rejected
```

规则：

- 改 Spec 身份字段 → 证据状态按 `evidence.py` 复位策略回退  
- 回测保存 **规格快照**，改策略不改历史回测  
- `oos_passed` 不是「可上线交易」

## 2. 用户工作流（推荐）

### 步骤 A：配置策略

1. 打开 **策略** 页 → 新建自定义策略  
2. 选 `single` 或 `portfolio`  
3. 填：假说、来源（书/候选 ID 可选）、股票池、数据依赖  
4. 用表达式编辑器写 **进场**、**原生离场**（组合则写 score + 调仓）  
5. 可选：加减仓、风险/止盈覆盖层、参数扫描路径、否决规则  
6. 点 **校验** → 看 `capability.supported`  
7. 保存  

### 步骤 B：设计验证

在 Spec 的 `validation` 中声明：

- `baseline_ids`：如 `buy_and_hold`、`equal_weight`  
- `locked_oos: true`  
- `rejection_criteria` 或结构化 `rejection_rules`  
- `parameter_scans`：参数网格（有上限）

标记证据 **验证设计完成**。

### 步骤 C：回测

1. **回测** 页选中该策略、区间、费用  
2. 运行 → 查看净值、交易、退出原因、validation 段  
3. 系统写回测快照 + 可能推进 `evidence_status`  

### 步骤 D：实验族（参数搜索）

1. **实验** 页用冻结 Spec 创建 experiment  
2. 提交多个 trial（param_patch）  
3. 全结果入库（含 no_trades / error）  
4. 需要时做多重检验（`multiple_testing`）  
5. 淘汰归档，不删除  

**证据推进（试验路径）**：

- 试验**从不**自动改 `evidence_status`（与回测页自动推进不同）。  
- 系统对**空 patch 基准 trial** 跑质量闸门（设计完成、身份匹配、样本量、数据质量等）。  
- **未达标**：不生成待办，直接拦截。  
- **达标**：写 `quant_evidence_promotion` pending 待办，用户「采纳为证据」才调用状态机；「忽略」仅关闭待办。  


### 步骤 E：日常研究（可选）

- 启用策略参与信号/研究计划  
- 与 `quant_trade` 手工记账对照（不自动下单）

## 3. 单标的运行时状态（编译层）

```text
FLAT → ENTRY → HOLDING → EXIT → FLAT
         ↘ BLOCKED（涨停/停牌）
HOLDING 可加/减仓（若 allow_add/reduce）
覆盖离场后 cooldown / native_reset
```

每次转移应可解释：**哪条表达式、何日、何值**（原因树）。

## 4. 回测撮合固定语义

| 项目 | 规则 |
|---|---|
| 信号时钟 | T 日收盘后可知数据 |
| 成交 | 最早 T+1 开盘可成交价 |
| 买不到 | 涨停/停牌/缺 bar → 失败归因 |
| 卖不出 | 跌停/停牌 → 延迟，计入路径损失 |
| 成本 | 佣金、印花税、滑点（可扩展冲击） |

**不在框架内提供第二套成交语义。** 新策略只改目标仓位生成，不改撮合。

## 5. 验证段（`validation.py`）要点

| 能力 | 行为 |
|---|---|
| 锁定 OOS | 窗口最后 20% 交易日（不足 MIN_OOS_BARS 则 unevaluated） |
| 基线 | `buy_and_hold`、`equal_weight` 等；未知 id → unavailable |
| 否决 | 命中 → rejected；有未评估 → incomplete；全过 → passed |
| 参数不稳 | 扫描中大量更优组合 → 否决（可配置） |

## 6. 与「搬策略」的区别

| 搬策略 | 验证框架 |
|---|---|
| 固定代码路径 | 任意合法规格 |
| 难对比消融 | 同一撮合下改 Spec 字段做消融 |
| 难归档失败 | ExperimentTrial 全记录 |
| 书中参数当真理 | 参数扫描 + 否决 |

**消融实验 = 复制策略 → 改一个条件 → 同区间回测 → 对比 validation。**  
这是框架的一等公民用法。
