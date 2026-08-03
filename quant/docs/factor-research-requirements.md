# 因子研究能力补强 · 需求文档

| 属性 | 内容 |
|------|------|
| 文档 ID | `REQ-FACTOR-RESEARCH-2026-08` |
| 状态 | 待评审 |
| 范围 | quant 因子研究的表达力、正交性检验与草稿沉淀链路 |
| 产品边界 | A 股日频研究;不下单、不连券商、不输出交易指令 |
| 关联 | `docs/a2a-design.md`(§8.13 / §8.13a)、`app/factors/evaluation.py`、`app/strategy/operators.py`、`docs/research/verification-protocol.md` |

---

## 1. 背景

2026-08 已完成因子评估的方法学补强:`factor.evaluate` 支持行业/市值中性化、
多前瞻期 IC 衰减、Newey-West t 值与多重检验报告,并新增
`factor.evaluation_list` / `factor.evaluation_get` 两个只读 skill 作为多轮
提炼的记忆面(见 `docs/a2a-design.md` §8.13 / §8.13a)。

至此 agent 能对**单个**因子给出可信度合格的结论。但三个缺口仍然存在,它们
共同决定 agent 能否从「评估一个因子」走到「构建一组因子」:

1. **写不出横截面因子**。因子求值是逐股时序的,行业内排名、截面 z-score、
   对市值回归取残差这类因子研究基本操作无法表达。
2. **答不出「新因子有增量吗」**。跑出第二个因子时,没有任何工具能回答它与
   已有因子的相关性,也就无法判断是新信息还是换个写法的旧信息。
3. **草稿沉淀不下来**。`factor.save_draft` 仅 admin;即便存了草稿,也没有
   A2A 侧的回填能力,`FactorDaily` 里没有历史值,之后走 `factor_key` 路径
   评估会读空 —— 链路在这里断掉。

三者都不是「接口没暴露」,而是能力本身缺失或链路断裂。

## 2. 目标

| # | 目标 | 验收口径 |
|---|------|---------|
| G1 | agent 能表达并评估横截面因子 | 能用 DSL 写出「行业内 20 日动量排名」并跑完 `factor.evaluate`,`ic_decay` 与分层结果非空 |
| G2 | agent 能判断新因子相对已有因子的增量 | 新 skill 返回与指定因子集的 IC 相关性矩阵,含正交化后残差 IC |
| G3 | 因子草稿能被普通研究会话沉淀并复用 | 非 admin 会话保存草稿 → 回填 → 按 `factor_key` 评估,全链路在一个会话内跑通 |

## 3. 非目标

**产品边界(任何时候都不做):**

- 分钟级 / Level-2 / 日内高频因子
- 自动交易、下单、券商连接
- 让 agent 自动 `enable` 因子或直接进入选股池(始终需人工在看板确认)
- 无界因子搜索:禁止 agent 穷举表达式空间刷 IC

**本期不做(后续候选):**

- 因子合成与权重优化(多因子打分模型)
- 因子拥挤度 / 容量估计
- 完整 Reality Check / Deflated Sharpe(当前只有 Bonferroni 粗校正)
- PIT 历史行业(见 §6 风险 R3)

## 4. 需求详述

### 4.1 横截面因子表达力(G1)

**现状**:`app/factors/engine.py` 的 `evaluate_factor(expr, df)` 接收**单只
股票**的日线 `DataFrame`,把每列包成 `Series` 求值。而 `rank` / `top_n`
算子在 `app/strategy/operators.py` 明确要求 `DataFrame` 输入,否则抛
「只能用于组合横截面」。因子链路因此永远拿不到横截面。

**关键发现**:组合策略侧已经有完整的横截面求值路径 ——
`app/strategy/compiler.py:_portfolio_fields()` 把 `{code: 日线帧}` 转成
`{字段: date×code DataFrame}`,`evaluate_expression` 在该形状下正常工作,
`rank` / `top_n` 也正是为它设计的。**所以本需求不是新建横截面引擎,而是让
因子求值复用这条既有路径。**

需求:

| 编号 | 需求 | 说明 |
|------|------|------|
| R1.1 | 因子求值支持横截面模式 | 新增 `evaluate_factor_cross_section(expr, pool_dfs) -> DataFrame`,复用 `_portfolio_fields` 的帧构造口径,不复制第二套实现 |
| R1.2 | 新增截面算子 | `cs_rank`(截面分位,0~1)、`cs_zscore`(截面标准化)、`cs_demean`(截面去均值);均需 `group_by` 可选参数支持组内计算 |
| R1.3 | 分组维度 | `group_by` 支持 `industry`;分组字段不进 `SUPPORTED_FIELDS`(不是价量数据),由求值上下文注入 |
| R1.4 | 表达式校验区分模式 | `validate_expression` 需能判定表达式是「时序」还是「横截面」;横截面表达式在单股路径(preview / `FactorDaily` 回填)上必须明确报错,而非静默算错 |
| R1.5 | `factor.evaluate` 支持横截面因子 | 截面因子在评估时直接按调仓日截面求值,不经 `FactorDaily` |
| R1.6 | `factor.preview` 明确拒绝 | 截面因子无法在 ≤5 标的上抽查(截面样本不足),需返回明确错误并提示改用 evaluate |

**边界**:`cs_*` 算子只在横截面上下文有意义。时序表达式里出现 `cs_rank`
必须校验失败 —— 这类错误静默通过会产出看似正常但完全错误的因子值。

### 4.2 因子相关性与正交性(G2)

**现状**:无任何相关性工具。agent 跑出第二个因子时只能靠「IC 都是正的」
这种无意义比较。

需求:

| 编号 | 需求 | 说明 |
|------|------|------|
| R2.1 | 新 skill `factor.correlation` | 输入待检因子(`expression` 或 `factor_key`)+ 对照因子集(`factor_keys[]`,缺省取全部 enabled) |
| R2.2 | 因子值相关性 | 逐调仓日截面算 Pearson 与 Spearman 相关,报时序均值与稳定性(标准差、|ρ|>0.7 的期数占比) |
| R2.3 | IC 相关性 | 两因子 IC 序列之间的相关性 —— 因子值低相关但 IC 高相关意味着捕捉同一收益来源 |
| R2.4 | 正交化增量 IC | 待检因子对对照因子集做截面回归取残差,报残差的 IC 与 t 值。这是 G2 的核心判据:残差 IC 不显著 = 无增量 |
| R2.5 | 成本定级 | 与 `factor.evaluate` 同级高成本:走确认闸门、互斥槽、日配额计 1 |
| R2.6 | 落库 | 结果落新表 `quant_factor_correlation`,可复现可复查;配套只读 skill 或并入 `factor.evaluation_list` |

**判读口径(写入 SKILL.md)**:残差 IC 不显著 → 明确说「相对已有因子无增量」,
不得因为裸 IC 好看就推荐;因子值相关性低但 IC 相关性高 → 提示可能是同一
收益来源的不同表达。

### 4.3 草稿沉淀链路(G3)

**现状三处断裂**:

1. `factor.save_draft` 要求 `can_admin`(`app/a2a/server.py` 授权检查 +
   skill 内二次校验),普通研究会话产出的因子存不下来。
2. A2A 侧无回填能力。REST 有 `POST /api/factors/backfill`,但需 admin 且
   不在 Agent Card 上。
3. 结果:存了草稿 → `FactorDaily` 无值 → 按 `factor_key` 评估读空
   (`_load_saved_factor_values` 返回空 dict)→ 「有效样本过少」失败。

需求:

| 编号 | 需求 | 说明 |
|------|------|------|
| R3.1 | `factor.save_draft` 放开给 `can_client` | 仍强制 `enabled=false`、`is_system=false`;key 冲突检查与保留字段校验不变 |
| R3.2 | 草稿归属 | `quant_factor_def` 加 `owner_id`(可空,系统因子为 NULL);非 admin 只能改/删自己的草稿。**这是放开权限的前提,不能省** |
| R3.3 | 谱系 | 加 `parent_factor_key`,与策略侧 `parent_strategy_id` 对齐,支持变体溯源 |
| R3.4 | 新 skill `factor.backfill` | 只允许回填**自己的 disabled 草稿**;区间上限与 `factor.evaluate` 一致(10 年);走 `factor_backfill` quant_task |
| R3.5 | 成本定级 | 高成本(全市场逐日计算),走确认闸门与配额 |
| R3.6 | 回填范围守卫 | 非 admin 不得回填系统因子或他人草稿;`factor_key` 为空(回填全部 enabled)仍限 admin |

**边界不变**:草稿始终 `enabled=false`,不自动进选股池;`enable` 只能人工
在看板操作。放开的是「存」和「算」,不是「用」。

## 5. 交付分层与优先级

| 层 | 内容 | 依赖 | 相对成本 |
|----|------|------|---------|
| **A** | §4.3 草稿沉淀链路(R3.1-R3.6) | 无 | 小 |
| **B** | §4.2 相关性与正交化(R2.1-R2.6) | 无(可与 A 并行) | 中 |
| **C** | §4.1 横截面表达力(R1.1-R1.6) | 无,但改动面最大 | 大 |

建议顺序 **A → B → C**:A 让现有能力闭环,B 直接提升结论质量且不动引擎,
C 改动因子求值核心与算子表,需要独立的回归验证。三层之间无硬依赖,可按
实际排期调整。

## 6. 风险与既有问题

| # | 风险 | 处置 |
|---|------|------|
| R1 | 横截面改动波及组合策略求值 | `_portfolio_fields` 与 `evaluate_expression` 是组合策略在用的生产代码。只允许**新增**调用方,不改既有签名与语义;`test_strategy_spec_regression.py` / `test_portfolio_strategies.py` 必须全绿 |
| R2 | `cs_*` 算子被误用于时序表达式 | R1.4 强制校验;并补「时序表达式含 cs_ 算子必须失败」的测试 |
| R3 | 行业数据非 PIT | `quant_stock.industry` 是**当前**行业(akshare `BOARD_NAME` 覆盖写入),按它做历史分组或中性化是轻微前视。本期沿用并在结论中声明;PIT 行业列为后续候选。**中性化已上线时即存在此问题,不是本期引入** |
| R4 | 全市场横截面内存占用 | 5000 只 × 2400 交易日 × N 字段的 date×code 帧不小。需实测并给出分块或按调仓日惰性求值方案 |
| R5 | 相关性 skill 放大配额消耗 | 与 evaluate 同池计费;对照因子集需设上限(建议 ≤20 个) |
| R6 | 单任务互斥 + 600s 超时 | 现有约束(`submit_task` 全局互斥、`QUANT_LONG_TIMEOUT=600`)会让「因子 × 多口径」对比只能串行,全市场评估也可能撞超时。本需求不解决,但 C 层实施前应实测确认是否需要先放宽 |

## 7. 数据前提

`factor.evaluate` 的「全市场」口径取决于日线覆盖。线上 `config.toml` 不随
部署同步,`bulk_daily_bars` / `full_market_daily` 的实际值需在实施前确认;
未开启全市场时评估只覆盖池内有日线的股票,IC 存在选择性偏差。任何正式
评估前先查 `market.data_quality`。

## 8. 验收标准

**A 层**:非 admin 会话内 `factor.save_draft` → `factor.backfill` →
`factor.evaluate`(按 `factor_key`)全链路跑通;非 admin 无法回填系统因子
或他人草稿(权限测试覆盖)。

**B 层**:对两个已知高相关因子(如 5 日与 10 日动量),`factor.correlation`
报出高相关且残差 IC 不显著;对一个已知低相关因子对,报出低相关且残差 IC
仍显著。

**C 层**:「行业内 20 日动量 `cs_rank`」能通过校验、跑完 evaluate 并产出
非空 `ic_decay`;同一表达式在 `factor.preview` 上返回明确的「截面因子不支持
抽查」错误;时序表达式误用 `cs_rank` 校验失败;组合策略既有回归测试全绿。

**全局**:Agent Card 与 `SKILL_IDS` 严格一致(现有测试已覆盖);
`docs/a2a-design.md` 同步新 skill 的 payload 与 artifact 契约;
`server/skills/quant-research/SKILL.md` 补对应判读口径与强制约束。
