# quant × Trace A2A 方案设计

| 属性 | 内容 |
|------|------|
| 文档 ID | `DESIGN-A2A-2026-07` |
| 状态 | 已拍板，待实现 |
| 范围 | quant 作为确定性 A2A Server；Trace（含微信）作为唯一 LLM 编排 Client |
| 产品边界 | A 股日频研究与模拟回测；不连接券商；不下单；不输出真实交易指令 |
| 关联 | `PRODUCT.md`、`AGENTS.md`、`docs/summry/00-framework-overview.md`、`docs/summry/06-gap-and-roadmap.md`、`docs/research/verification-protocol.md`、[A2A Protocol](https://a2a-protocol.org/latest/specification/) |

---

## 1. 背景与目标

### 1.1 要解决的问题

希望在 **Trace 对话** 与 **微信通道** 中，由智能体调用 quant 的研究能力，自动完成 **策略验证与参数/结构提炼**，并把结果以可复现的 `run_id` / `experiment_id` / artifact 形式回传用户；在过程中把「能力不够 / 数据不够 / 表达力缺口」沉淀为系统补强信号。

quant 已具备完整 REST、领域服务与 **Experiment / Trial 试验账本**，但缺少对 **Agent 间互操作** 的标准入口；若在 Trace 与 quant 各跑一套开放式 LLM Agent，会出现双重推理成本、职责漂移与不可审计问题。

### 1.2 目标

1. quant 以 **官方 A2A** 暴露研究能力：Agent Card 对任意合规 A2A Client 可发现；**调用面向结构化 Client**（能按 §7 约定发送 data part，见 §7.2 规则 4）。
2. quant **不运行 LLM**，仅做确定性工具执行（Deterministic A2A Agent）。
3. Trace 是 **唯一会思考的编排方**；微信不直连 quant，统一经 Trace Orchestrator。
4. 长任务（回测 / trial / 因子评估）经 **SSE** 推送生命周期；鉴权采用现有 **用户 JWT 透传**。
5. 人机看板继续走现有 REST；A2A 是 Agent 主路径，不替换 Web API。
6. **支撑 Agent 策略研究闭环（主叙事）**：提出可检验假设 → 读 catalog/质量 → 校验 Spec → 落库草稿（带谱系）→ **注册 experiment（冻结规格）** → 在闸门内跑 trial / 回测 → 查 trial 与 run 历史 → 对比迭代或停止。人工只在高成本执行（回测/trial）处按闸门介入（§5.4）。严肃验证 **对齐现网 experiment 主链**，不只「乱建草稿再 backtest」。
7. **支撑 Agent 因子提炼闭环（主叙事）**：表达式校验 → 小样本预览 → **全市场有效性评估（IC / RankIC / 分层收益，`factor.evaluate`，本期新建 domain 能力，§8.13）** → 草稿落库（admin）。因子评估属高成本 skill，与回测共用确认闸门、互斥与日配额。
8. **发现系统不足是一等结果**：校验 **与运行期** 失败写入 `failure_kind` / `missing_capability` 可聚合；Orchestrator 按停止条件输出结构化 findings 并经 `system.report_finding` **落表**（§8.15）；管理端可消费缺口排行（§13）。

### 1.3 产品分层（避免目标漂移）

| 层级 | 交付内容 | 说明 |
|------|---------|------|
| **A. 策略研究 Agent** | 端到端：假设 → experiment → trial → 对比 → 小结 | 普通 `can_client` 可用；默认路径 E（§2.1） |
| **B. 因子提炼工具链** | validate / preview / **evaluate（IC·RankIC·分层）** / save_draft | evaluate 为高成本 skill；save_draft 仍 admin |
| **C. 系统缺口闭环** | 审计缺口列（含运行期失败）+ findings 强制落表 + 管理端聚合 | 反哺 `06-gap-and-roadmap` 类补强优先级 |
| **D. 微信入口** | 同一 Orchestrator；确认更严、无批量授权 | 与其余三层同批交付，不互相阻塞 |

### 1.4 非目标

**永久产品边界（任何时候都不做）：**

- quant 内嵌研究 Worker / 多步 LLM loop（LLM 只在 Trace）
- 券商连接、下单、半自动交易；输出真实交易指令
- 用 A2A 替换 `web/` 看板 REST
- 自然语言选股（仅结构化筛选）
- 自动 `accept` 证据推进待办（promotion 仅可读提示；用户在看板确认，Agent 不代点同意）
- 无界网格扫参（trial batch 有硬上限；禁止 agent 穷举参数空间）
- **仅有 inline `spec`、无已保存 `strategy_id` 的 ephemeral 回测**（须先落库策略再跑，对齐现网 REST / experiment 落库要求）

**本期明确不做（后续增强候选，非产品边界）：**

- 官方 A2A Push Notification（webhook）
- service account + on-behalf-of 双身份体系
- quant 多实例共享 Task 存储（本期假设单实例，与现网 `hank-quant.service` 一致）
- 按日期/股票池切片的数据质量报告（domain 扩展，§8.2）  

---

## 2. 已拍板决策

| # | 决策点 | 选择 | 理由 |
|---|--------|------|------|
| 1 | 第一调用方 | Trace 对话 + 微信 | 微信复用 `server/src/weixin/`，同一 Orchestrator |
| 2 | quant 智能程度 | **纯工具**（无 LLM） | 研究语义与边界留在 quant；编排与话术在 Trace |
| 3 | 协议 | **官方 A2A** | 发现、Task、异步、可扩展第三方 Client |
| 4 | 流式 | **上 SSE** | 对齐 A2A Streaming；回测/trial 进度可观测 |
| 5 | 鉴权 | **用户 JWT 透传** | 两端已共用 `jwt_secret` 与 claims，实现成本最低 |
| 6 | 回测规格来源 | **`backtest.run` 必须 `strategy_id`** | 对齐 `BacktestIn` / `_prepare_backtest`；不做 ephemeral spec 路径 |
| 7 | Cancel | **pending 即取消 + running 协作中断** | 引擎主循环加 `threading.Event` 检查点（交易日/标的批次粒度），Cancel running 在下一检查点安全退出、状态一致；不追求即时取消（§4.5） |
| 8 | 成本字段 | **`costs` 对齐引擎** | `commission` / `stamp_tax` / `slippage`（价格比例）；无 `initial_cash`、无 `slippage_bps` |
| 9 | 策略草稿落库 | **`strategy.save_draft`，免确认** | `enabled=false` + 列 `research_status=unverified`；可选 `parent_strategy_id` 谱系；闸门留在高成本 skill；复用 `_check_quota` |
| 10 | **严肃验证主链** | **`experiment.*` 一等 skill** | 对齐 `app/api/experiments.py` / `app/experiment/service.py`：冻结规格 + trial 账本 + 失败不可抹；多轮参数迭代 **优先 experiment**，禁止只靠连建草稿冒充试验 |
| 11 | 因子范围 | **卫生 + 有效性评估全链** | `factor.validate` + `factor.preview`（少量多标的）+ **`factor.evaluate`（IC/RankIC/分层，高成本，§8.13）** + `factor.save_draft`；validate/preview/evaluate 开放 `can_client`（前两个 REST 同步放宽，属本期实现任务），`save_draft` 仍 `can_admin` |
| 12 | 迭代记忆 | **`experiment.get` / `experiment.list` + `backtest.list`** | trial 级记忆为主，run 列表为辅；不能只靠对话上下文 |
| 13 | 批量确认 | **会话级授权（仅对话入口）** | 「本会话最多 N 次高成本执行」（含 `backtest.run` 与 `experiment.trial` / `trial_batch` 内每次回测）；微信单次；共用日配额 |
| 14 | 证据推进 | **trial 不自动改 `evidence_status`** | 对齐现网：达标只生成 promotion 待办；`backtest.run` 成功仍走 `advance_after_backtest`（与 REST 单次回测路径一致） |
| 15 | 研究 Agent 规格 | **§10.5 状态机 + 停止条件 + 端到端验收** | 协议正确 ≠ 研究 Agent 可用；两者分开验收 |
| 16 | 日高成本配额 | **默认 50/用户/日，可配置** | 路径 E 多轮提炼 + 因子评估同池；20 对自动提炼过紧；互斥与 10 年区间上限仍是主防线 |
| 17 | findings 落地 | **`system.report_finding` 落表（§8.15）** | Conclude 强制 findings；只留对话会跨会话丢失，路径 B 形同虚设；落表后与审计列同被 `gap_summary` 聚合 |
| 18 | 运行期缺口 | **trial/run 失败也写缺口列** | 仅 validate 失败写列只反映语法层缺口；`outcome=error` 与能力拒绝同样是一等发现（§12） |

概念澄清：

> 官方 A2A 的对端称为 Agent，但 quant **不跑模型**。  
> quant 实现完整 A2A **服务端语义**（Card / Task / SSE / Cancel），执行路径为：鉴权 → 解析 skill+payload → 调内部 service → 回 artifacts。  
> 对外仍是标准 A2A 节点；对内是确定性工具网关。  
> **Trace 侧的 `quant-research` 才是「会思考的研究 Agent」**；本文 §10 是其行为规格，§8 是其工具契约。

### 2.1 两条验证路径（写死，避免 Agent 走歪）

```text
路径 S — 快速单次回测（探索）
  save_draft → validate → backtest.run → backtest.list
  用途：冒烟、看大致收益形状；不构成「试验账本」级证据

路径 E — 严肃试验（默认多轮提炼）
  save_draft（拿 strategy_id）→ experiment.create（冻结 spec + hypothesis）
    → experiment.trial | trial_batch（param_patch 变体）
    → experiment.get（trials + multiplicity + pending promotions）
  用途：可复现假说验证；失败 trial 保留；与看板 Experiments 同源
```

Orchestrator **默认走路径 E** 做「提炼/对比/是否值得继续」；路径 S 仅在用户明确要求「先随便跑一下」或 experiment 前置条件不满足时使用。

---

## 3. 总体架构

```text
用户
 ├─ Trace 桌面/Web 对话 ──► chat SSE
 └─ 微信 ────────────────► weixin router
              │
              ▼
     ┌────────────────────────────────────┐
     │ Trace server                         │
     │  Orchestrator Agent（唯一 LLM loop） │
     │  skill: quant-research（§10.5 循环） │
     │  tools: quant_*（封装 A2A Client）   │
     │  确认闸门 / 停止条件 / findings      │
     │  话术 / 微信摘要格式化               │
     └────────────────┬───────────────────┘
                      │ HTTPS
                      │ Authorization: Bearer <user_jwt>
                      │ A2A JSON-RPC + SSE
                      ▼
     ┌────────────────────────────────────┐
     │ quant (:8100)                        │
     │  A2A Server（无 LLM）                │
     │  Agent Card + tasks + stream         │
     │  skill handlers → domain services    │
     │  长任务 ↔ quant_task / trial 执行    │
     └────────────────────────────────────┘
                      │
                      ▼
         strategy / experiment / backtest
         factors / selection / data.quality / catalog
```

### 3.1 职责边界

| 组件 | 负责 | 不负责 |
|------|------|--------|
| **Trace Orchestrator** | 理解意图、拼 Spec、选路径 S/E、遵守停止条件、确认闸门、对用户措辞、会话 findings、微信适配 | 回测撮合、前视规则、能力解析、多重检验计算 |
| **quant A2A** | Card、Task/SSE、JWT、skill 确定性执行、artifact、审计缺口列 | 闲聊、自由「想策略」、交易执行、自动 accept promotion |
| **quant REST / web** | 人机看板、现有 API（含 promotion 确认） | 不强制经 A2A |

### 3.2 双层 SSE（勿混淆）

```text
Browser / 微信客户端
    │  产品层 SSE / 消息推送（Trace 已有）
    ▼
Trace Orchestrator
    │  A2A 协议层 SSE（server → quant）
    ▼
quant A2A Server
```

A2A SSE **不**直接透传给终端用户；Orchestrator 消费后折叠为对话状态或微信短文本。

---

## 4. 协议与传输

### 4.1 对齐范围

以 [A2A Protocol Specification](https://a2a-protocol.org/latest/specification/) 为准，本期实现子集：

| 抽象操作 | 本期 | 说明 |
|----------|----|------|
| Get Agent Card / 发现 | 是 | `/.well-known/agent-card.json` 或 SDK 约定路径 |
| Send Message | 是 | 短任务可阻塞至终态 |
| Send Streaming Message | **是** | 主路径；SSE 推状态与 artifact |
| Get Task | 是 | 断线后查询 |
| Cancel Task | 是 | 映射 `cancel_task`；pending 即取消，running 走检查点协作中断（§4.5） |
| Subscribe to Task | 是 | 断线后续订长任务（agent 长研究会话必需） |
| List Tasks | 是 | 长任务列表以 `quant_task` 为准；ephemeral 短任务不列入 |
| Push Notification Config | **否** | `pushNotifications: false` |

### 4.2 能力声明

```json
"capabilities": {
  "streaming": true,
  "pushNotifications": false,
  "extendedAgentCard": false
}
```

### 4.3 绑定

- **JSON-RPC 2.0 over HTTP** 为控制面（或官方 SDK 默认绑定）  
- **SSE** 为 Streaming Message / Subscribe 的事件通道  
- 请求头服务参数（按 spec）：`A2A-Version` 等  

SDK 选型（拍板）：

- **quant 侧（Python）**：优先官方/社区 A2A SDK，锁定当前稳定 minor 版本。  
- **Trace 侧（Rust）**：**无成熟官方 A2A Rust SDK，手写最小 client**——本期子集仅 4 个 JSON-RPC 方法 + SSE 消费，成本可控；落点定为新 crate `crates/hank-a2a-client`，`quant_*` tools 在 server 侧引用该 crate。

本文 schema 为**语义契约**，字段名以 proto/SDK 为准做映射，业务 payload 不变。

### 4.4 Task 状态映射（仅长任务）

`backtest.run` 持久化映射 `quant_task`（见 `app/models.py` Task / `app/tasks.py`）：

| A2A Task state | quant 内部（`quant_task.status`） |
|----------------|------------|
| submitted | `pending`（入队） |
| working | `running`（编译、模拟中） |
| completed | `done` + artifacts |
| failed | `failed` + 模型可读错误 |
| canceled | `cancelled`（双 l，幂等；pending 即达，running 经检查点协作中断后可达，§4.5） |

终态：`completed` | `failed` | `canceled` | `rejected`（按 spec 命名对齐实现）。

**既有约束：`quant_task` 单任务互斥。** 现网每个用户同时只能有一个 `pending`/`running` 任务，提交冲突返回 **409**。A2A `backtest.run` 复用该约束：撞 409 时 Task 置 `failed`，message 写明「已有进行中的任务，可等待完成；若仍为排队中可先 Cancel」（模型可读，可操作），**不**排队。本期的「每用户并发限额」即此互斥，不再新增并发配置。

> 为何拒绝也产出 Task 而非在 Send Message 层直接返回 JSON-RPC error：**有意为之**。拒绝以 Task `failed` 形式返回，错误文案统一走 SSE / Get Task 通道，Orchestrator 只需一条消费路径；实现者不要再拆「协议层拒绝」第二通道。

**区间上限**：复用现网 `validate_backtest_window`（**最长 10 年**，`MAX_BACKTEST_YEARS`），A2A **不**另起更松默认。  
**日次数**：新配置项，默认 **50 次/用户/自然日**（超限 → Task `failed`，文案含剩余额度与重置时间）；可在 `quant` config 覆盖。**计数口径**：以 `quant_a2a_audit` 中该用户当日 **高成本 skill** 记录数为准——`backtest.run`、`experiment.trial`、`factor.evaluate` 各计 1 次；`experiment.trial_batch` 按 **成功提交的 param_patch 条数**计（含 outcome=error 的已执行项，防止用失败绕过）。含失败/被拒——防刷；审计表与高成本流同批落地（§12 / §14）。**「自然日」写死为 quant 服务器本地时区**（A 股交易日口径），不随 client 时区变化。  
**幂等**：`backtest.run` / `experiment.trial` / `experiment.trial_batch` / `factor.evaluate` payload 支持可选 `client_request_id`（§8.4 / §8.11 / §8.13）；同一用户当日内重复提交相同 `client_request_id` → 不新建 run/trial/evaluation，直接返回首个对应 Task / 结果引用。SSE 断线后 client 重发不会造成重复落库与重复计配额。  
**validate 失败不进高成本配额**：仅在实际进入回测/trial 执行（或明确因 confirmed/配额/互斥拒绝的 `backtest.run`/`experiment.trial*` 请求）时计次；纯 `strategy.validate` / `factor.validate` 只计入读类限速。

### 4.5 Cancel 语义（拍板：pending 即取消 + running 协作中断）

| `quant_task.status` | Cancel 结果 | A2A 表现 |
|---------------------|-------------|---------|
| `pending` | 成功 → `cancelled`；关联 `BacktestRun` 一并 cancelled | Task → `canceled`，关流 |
| `running` | **协作中断**：引擎主循环每轮迭代检查 `threading.Event`；置位后在下一检查点安全退出，已落库部分保持一致（run/trial 标记 `cancelled`，**不**写半量 metrics） | Task → `canceled`（message 带「已在检查点中断，已完成部分不计入结果」），关流 |
| 已终态 | 幂等：保持原终态 | 返回当前状态 |

实现任务（quant domain 小改，非 A2A 层逻辑）：`run_backtest` 主循环注入取消事件检查点，粒度为**交易日**；`factor.evaluate` 全市场计算循环同样埋点，粒度为**标的批次**。现网执行线程无此机制，属本期新增。

验收口径：pending 取消无僵尸 pending；running 取消后 DB 无半写记录、状态一致为 `cancelled`；不伪装即时取消。

### 4.6 短任务 vs 长任务的 A2A Task 存储

**短任务**（不写 `quant_task`）：  
`strategy.validate` / `strategy.save_draft` / `catalog.get` / `market.data_quality` / `selection.screen` / `backtest.get` / `backtest.list` / `factor.validate` / `factor.preview` / `factor.save_draft` / `experiment.create` / `experiment.get` / `experiment.list` / `system.gap_summary` / `system.report_finding`

| 项 | 约定 |
|----|---------|
| 存储 | **进程内 ephemeral**（内存 dict），生成 A2A `task_id` 供 SSE / Get Task |
| TTL | 完成后保留 **15 分钟**，超时 Get Task → not found |
| 重启 | 进程重启后短任务 Task 全部丢失（可接受；结果已在当次 SSE 推完） |
| 断线 | 短任务 SSE 断开即视为结果可能丢失；TTL 内可 Get Task 碰运气，但 **client 的可靠路径是直接重发请求**（读类与 create 类幂等或可安全重试），不要实现断点续传 |
| 多实例 | **假设 quant 单实例**（与现网 `hank-quant.service` 一致）；多副本需共享 Task 存储，列入后续增强（§1.4） |

**长任务**（高成本，需确认）：

| skill | 执行模型 | 互斥 / Cancel |
|-------|----------|---------------|
| `backtest.run` | A2A Task ↔ `quant_task` + `quant_backtest_run`；Get/Cancel 以 DB 为准 | 复用单任务互斥；Cancel **仅 pending** |
| `experiment.trial` | REST 现网为 **同步** `create_trial_and_run`。A2A：**后台线程执行同一 service**，A2A Task 进程内（或可选映射 `quant_task` 若实现方便）推 SSE；**占用与 `backtest.run` 同一用户互斥槽**（同时只能跑一个高成本任务），避免双路径打满 CPU | Cancel：若尚未开始跑引擎可 canceled；已进入 `run_backtest` 则同 running 不可中断 |
| `experiment.trial_batch` | 顺序执行多个 trial（硬上限见 §8.11）；整批一个 A2A Task，进度事件带 `trial_index` | 同上互斥；pending 可整批取消，已开始的当前 trial 走检查点中断 |
| `factor.evaluate` | A2A Task ↔ `quant_task`（新 domain 长任务，§8.13）；全市场计算后台线程执行 `app/factors/evaluation.py` | 与回测共用互斥槽；Cancel 检查点按标的批次（§4.5） |

> 实现注意：不要在 A2A 层复制撮合逻辑；trial 必须调用 `create_trial_and_run`（或抽出的同函数），保证失败也落 trial、promotion 语义与 REST 一致。
>
> **进程重启兜底**：`experiment.trial` / `experiment.trial_batch` / `factor.evaluate` 的 A2A Task 若为进程内存储，重启后 Get Task 丢失但 DB 已有 trial/run/evaluation 记录。约定：Orchestrator 在 Get Task not found 时改用 `experiment.get` / `backtest.list` / `backtest.get` 恢复终态（写入 §10.2 行为约束 #15），不得盲目重发；确需重发必须复用同一 `client_request_id` 走幂等路径。

---

## 5. 鉴权

### 5.1 方案：用户 JWT 透传

```http
Authorization: Bearer <user_jwt>
```

- `jwt_secret` 与 claims 与现网一致：`sub`, `username`, `can_admin`, `can_client`  
- quant 复用 `app/auth.py` 的 `require_client` 语义  
- 所有写模拟结果（回测落库）绑定 `sub` → `user_id`  
- **验签时机**：仅在 A2A 请求入口（Send Message / Streaming / Get Task / Cancel 等）校验 JWT；任务一旦进入 `running`，执行本身**不受** token 在执行中途过期影响（与现网异步回测一致）

### 5.2 入口如何拿到 JWT

| 入口 | 方式 |
|------|------|
| Trace 对话 | 会话已登录，Orchestrator 使用当前用户 token |
| 微信 | 绑定到 Trace `user_id` 后，由 Trace **内部代签**用户 JWT（server 持有 `jwt_secret`，签名逻辑与登录签发同 claims）；**不在 quant 新建微信身份** |

> 注意：「内部代签」是 **Trace 侧新增实现任务**，不是既有逻辑——现有签发都绑在密码登录路径（`server/src/auth.rs` / `quant/app/auth.py`）。代签入口必须是 server 内部函数，不暴露 HTTP 端点，避免成为绕过密码的签发后门。

### 5.3 本期不做

- Trace service token + `X-On-Behalf-Of`  
- 匿名 A2A skill 调用（Card 可公开读，**调用必须鉴权**）  

### 5.4 产品级确认闸门（非第二套鉴权）

**高成本 skill**（须 `confirmed: true`）：

- `backtest.run`
- `experiment.trial`
- `experiment.trial_batch`（一次确认覆盖整批；批内条数计入会话授权与日配额）
- `factor.evaluate`（全市场计算，与回测同级成本）

```json
"confirmed": true
```

由 Trace 在用户确认后置位；quant 缺失则 `failed` / 校验错误，防止模型误触。

**免确认**（数据/注册，不耗回测算力或仅轻量写）：

| skill | 理由 |
|-------|------|
| `strategy.save_draft` | `enabled=false` 草稿；防刷靠策略配额 |
| `experiment.create` | 冻结注册，不跑回测；防刷靠读/写限速 + 用户 experiment 数量常识（超限可后续加配额） |
| `factor.save_draft` | admin 草稿因子 `enabled=false`；对齐 REST 创建但默认不启用 |
| 全部 read skill | — |

闸门实现要求（Trace 侧，写死）：

1. **`confirmed` 只能由 Trace 工具调用拦截层在用户真实确认后注入**，模型自身输出不得直接置位；模型传 `confirmed: true` 而未经用户确认时拦截层应剥离并先走确认流程。
2. **对话入口**：UI 确认按钮（复用现有工具确认机制）。
3. **对话入口·会话级批量授权**：用户可一次性确认「本会话内允许最多 N 次**高成本执行**」（N 由用户选择，硬上限 ≤ 当日剩余配额）。`backtest.run`、`experiment.trial`、`factor.evaluate` 各消耗 1；`trial_batch` 按 patch 条数消耗。额度内自动注入 `confirmed`，用尽后回到逐次确认。与日配额**同一池**（§4.4）；授权 UI 展示当日剩余。授权是会话态，不落表、重启即失效。
4. **微信入口**：Orchestrator 先发待确认摘要（策略/experiment id、区间、费用、trial 数量）；用户肯定白名单确认后置位；**5 分钟超时作废**。微信**无**批量授权。
   - **肯定白名单**（去空白、全半角不敏感，整句匹配其一即可）：`确认` / `好的` / `是` / `OK` / `ok` / `同意`  
   - 白名单外任意回复视为放弃（不执行）。**有意取舍**：追问也作废待确认单——宁可不执行；**不要**优化成忽略非白名单继续等待。
   - 微信确认状态挂在会话上下文，不落新表。**存储写死**：Orchestrator 进程内 map（key = 微信会话 id，value = 待确认摘要 + 创建时间），5 分钟 TTL 过期即清除；进程重启即全部作废（用户需重新发起），不持久化、不跨会话。

---

## 6. Agent Card（草案）

```json
{
  "name": "quant-research",
  "description": "A-share daily research tools: validate StrategySpec, register experiments/trials, run simulated backtests, factor validation/preview/market-wide evaluation (IC, layered returns), screen universe, report data quality and capability gaps, record research findings. No broker connectivity or order execution. Deterministic server (no LLM). Invocation requires structured data parts (skill+payload); text-only messages are rejected.",
  "version": "1.0.0",
  "protocolVersion": "0.3",
  "url": "https://<quant-host>/a2a",
  "capabilities": {
    "streaming": true,
    "pushNotifications": false,
    "extendedAgentCard": false
  },
  "defaultInputModes": ["application/json", "text/plain"],
  "defaultOutputModes": ["application/json", "text/plain"],
  "securitySchemes": {
    "bearer": {
      "type": "http",
      "scheme": "bearer",
      "bearerFormat": "JWT"
    }
  },
  "security": [{ "bearer": [] }],
  "skills": [
    {
      "id": "catalog.get",
      "name": "Get research catalog",
      "description": "Fixed dictionaries: filter fields, operators, labels, snippet metadata. Call before authoring new specs.",
      "tags": ["catalog", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "market.data_quality",
      "name": "Data quality snapshot",
      "description": "Coverage and trust metrics for bars, ST, valuation, fundamentals.",
      "tags": ["data", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "strategy.validate",
      "name": "Validate StrategySpec",
      "description": "Strict validation and capability report. Does not persist strategy.",
      "tags": ["strategy", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "strategy.save_draft",
      "name": "Save strategy draft",
      "description": "Persist StrategySpec as disabled unverified draft. Optional parent_strategy_id for lineage. Returns strategy_id for experiments/backtests.",
      "tags": ["strategy", "write-draft"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "experiment.create",
      "name": "Create experiment",
      "description": "Register frozen-spec experiment with hypothesis and permanent_candidate_id. Requires strategy_id for trial runs. No confirmation.",
      "tags": ["experiment", "write-registry"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "experiment.get",
      "name": "Get experiment",
      "description": "Experiment detail, trials, multiplicity hints, pending evidence promotions.",
      "tags": ["experiment", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "experiment.list",
      "name": "List experiments",
      "description": "List caller's experiments (summary). Avoid duplicate candidate registrations.",
      "tags": ["experiment", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "experiment.trial",
      "name": "Run experiment trial",
      "description": "Apply param_patch on frozen spec, run simulated backtest, append immutable trial. Long-running. Requires confirmed=true.",
      "tags": ["experiment", "write-sim"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "experiment.trial_batch",
      "name": "Run experiment trial batch",
      "description": "Sequential param_patch trials (hard cap). Requires confirmed=true. Counts as multiple high-cost units.",
      "tags": ["experiment", "write-sim"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "factor.validate",
      "name": "Validate factor expression",
      "description": "Validate factor expression. Does not by itself prove market-wide efficacy; use factor.evaluate for IC/layered evidence.",
      "tags": ["factor", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "factor.preview",
      "name": "Preview factor series",
      "description": "Compute factor on 1..N codes (small N). Spot-check only; use factor.evaluate for market-wide IC/layered returns.",
      "tags": ["factor", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "factor.evaluate",
      "name": "Evaluate factor efficacy",
      "description": "Market-wide (or pool-scoped) factor evaluation: IC / RankIC / ICIR and layered long-short returns over a date range. Long-running, high-cost. Requires confirmed=true.",
      "tags": ["factor", "write-sim"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "factor.save_draft",
      "name": "Save factor draft",
      "description": "Persist disabled factor definition. Admin only. No confirmation.",
      "tags": ["factor", "write-draft"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "backtest.run",
      "name": "Run backtest",
      "description": "Simulated T+1 backtest for saved strategy_id (path S). Long-running. Requires confirmed=true. Prefer experiment.trial for multi-round refinement.",
      "tags": ["backtest", "write-sim"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "backtest.get",
      "name": "Get backtest run",
      "description": "Fetch summary for an existing backtest run owned by the caller.",
      "tags": ["backtest", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "backtest.list",
      "name": "List backtest runs",
      "description": "List caller's recent backtest runs. Secondary memory; prefer experiment.get for trial lineage.",
      "tags": ["backtest", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "selection.screen",
      "name": "Structured screener",
      "description": "AND/OR condition screen with per-field coverage warnings.",
      "tags": ["selection", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "system.gap_summary",
      "name": "Capability gap summary",
      "description": "Aggregate missing_capability / failure_kind from A2A audit plus persisted research findings, for the caller or admin global view. Read-only.",
      "tags": ["system", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "system.report_finding",
      "name": "Report research finding",
      "description": "Persist structured research findings (missing capability, data gap, hypothesis outcome) from the Orchestrator's Conclude step. Aggregated by system.gap_summary.",
      "tags": ["system", "write-finding"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    }
  ]
}
```

说明：

- Card 文案明确 **No trading / No LLM**  
- `protocolVersion` 实现时与所选 SDK/spec 版本对齐  
- skill 列表即本期范围；扩 skill 走 Card 版本升级，不破坏旧 payload  

---

## 7. 消息与路由约定

### 7.1 请求 Message（Client → quant）

结构化优先，便于纯工具路由：

```json
{
  "role": "user",
  "messageId": "<uuid>",
  "parts": [
    {
      "kind": "data",
      "data": {
        "skill": "strategy.validate",
        "payload": {}
      }
    }
  ],
  "metadata": {
    "source": "trace_chat",
    "trace_session_id": "<optional>"
  }
}
```

`metadata.source`：`trace_chat` | `weixin`。

### 7.2 路由规则

1. 校验 Bearer JWT → `user_id`  
2. 解析 `parts` 中 data：`skill` + `payload`  
3. 未知 skill → 校验错误（模型可读：列出 Card 内 skill id）  
4. **仅有 text、无 skill/payload** → 拒绝（避免把 quant 当聊天后端）。推论：通用第三方 A2A Client 默认发 text，因此**只能发现、不能调用** quant；可调用的 Client 必须能构造结构化 data part（与 §1.2 目标 1 的口径一致）  
5. 执行 handler → Task + Artifacts  

### 7.3 Artifact 原则

- 对话/微信只消费 **summary** 级 JSON  
- 大对象（全量净值、全市场筛选行）**不**整包塞进 A2A；用 `run_id` / 上限截断 + `detail_ref`  
- 错误信息面向 **Orchestrator 模型**（可改什么），不是只给开发者错误码  

---

## 8. Skill 契约（payload / artifact）

以下为业务契约；A2A 外壳字段以 spec 为准。

### 8.1 `catalog.get`

**payload**

```json
{
  "sections": ["strategy_authoring", "product_boundary"]
}
```

- `sections` 是枚举；未知值返回模型可纠错错误并列出可用值，不得静默忽略。
- `strategy_authoring` 返回运行时真源生成的支持字段、操作符精确 JSON 形状、限制，以及完整合法系统 Spec 示例；Agent 应复制最近示例后修改，禁止猜 schema。
- `sections` 省略 = 返回实现支持的全部目录段。

**artifact `catalog`**：与现有 `GET /api/catalog` 语义对齐的 JSON（可裁剪体积）。

---

### 8.2 `market.data_quality`

**payload**：**无参数**（空对象 `{}`）。

- 对齐现有 `GET /api/market/data-quality`（`app/api/market.py`）与 `data_quality_public_summary`（`app/data/quality.py`）：全局只读快照，读旁路缓存，不触发采集。
- **不支持** `date` / `universe` 参数：现有 domain service 没有按日期/股票池切分的口径；`watchlist` 是每用户关系，与全市场质量快照语义不匹配。按维度切片的质量报告属 domain 扩展，列入后续增强候选（§1.4），不进本期契约。

**artifact `data_quality`**：字段名与 `data_quality_public_summary` 一致（示意）：

```json
{
  "as_of": "2026-07-29",
  "alert_level": "warning",
  "stock_count": 5400,
  "latest_bar_date": "2026-07-29",
  "st_stock_coverage_ratio": 0.98,
  "st_bar_coverage_ratio": 0.99,
  "valuation_coverage_ratio": 0.05,
  "fundamental_coverage_ratio": 0.02,
  "computed_at": "2026-07-29T18:05:00"
}
```

- `alert_level` 枚举与现网 `_alert_level` 一致：**`ok` | `warning` | `critical`**（不是 `warn`）。
- Orchestrator 侧解读规则：`valuation_coverage_ratio` / `fundamental_coverage_ratio` 远低于 1 时，使用相关字段的筛选/回测结论有偏，需在摘要中提示（对应 §10.2 行为约束）。
- 实现可对快照做字段裁剪（去掉 `st_window_*` 等运维口径字段），但比率与 `alert_level` 字段名不变。

---

### 8.3 `strategy.validate`

**payload**

```json
{
  "spec": {}
}
```

**artifact `validation_result`**（字段名与现网 `validation_out` / `POST /api/strategies/validate` 对齐）

```json
{
  "valid": true,
  "capability": {
    "status": "supported",
    "issues": []
  },
  "spec_hash": "<hash>",
  "errors": []
}
```

- `valid`：`parsed is not None and capability.status == supported`（同 REST，**不用** `ok`）  
- `spec_hash`：`strategy_spec_hash(parsed)`（同 REST；若未来要忽略 `evidence_status` 的 identity 语义，另加字段 `identity_hash`，本期不强制）  

`capability.status` 与现网一致：

- `supported`  
- `missing_data`  
- `missing_engine`  
- `subjective_only`  
- `boundary_denied`  

不落库、不改 `evidence_status`。

---

### 8.4 `backtest.run`

**payload**（对齐现网 `BacktestIn` + A2A 确认闸门；**不做 ephemeral spec 回测**）

```json
{
  "strategy_id": 42,
  "start": "2024-01-01",
  "end": "2025-12-31",
  "codes": [],
  "pool_id": null,
  "costs": {
    "commission": 0.00025,
    "stamp_tax": 0.0005,
    "slippage": 0.0001
  },
  "confirmed": true,
  "client_request_id": "<optional opaque string>"
}
```

| 字段 | 说明 |
|------|------|
| `strategy_id` | **必填**。调用方必须先通过 REST 或 `strategy.save_draft`（§8.7）保存策略；A2A 只跑已落库、当前用户可读的策略 |
| `start` / `end` | 必填；走 `validate_backtest_window`（最长 **10 年**，禁止未来日） |
| `codes` | 可选；空则按策略 universe / 池解析（同 REST） |
| `pool_id` | 可选；临时覆盖研究范围，与 `codes` 互斥规则同 `_prepare_backtest` |
| `costs` | 可选覆盖；键名对齐 `DEFAULT_COSTS`：`commission` / `stamp_tax` / **`slippage`（价格比例，不是 bps）**。省略则用引擎默认 |
| `confirmed` | 必须为 `true`（由 Trace 拦截层注入，逐次确认或会话级批量授权，见 §5.4） |
| `client_request_id` | 可选幂等键（≤128 字符，opaque）。同一用户当日重复提交同值 → 不新建 run，返回首个 Task/run 引用；由 Trace client 生成（建议每轮研究意图一个 uuid） |

**明确不支持的 payload 字段：**

- 裸 `spec` / 仅 inline StrategySpec 无 `strategy_id` → **校验错误**，文案引导先保存策略再回测  
- `initial_cash`：引擎净值归一 `init_cash=1.0`，相对收益指标；**出现则校验错误**（勿静默忽略）  
- `fees` / `slippage_bps`：**出现则校验错误**；模型应使用 `costs.slippage`（例：5 bps → `0.0005`）  
- `params`：现网 `BacktestIn` 有该字段（仅兼容旧客户端），A2A 契约**不支持**；**出现则校验错误**，文案提示参数应已固化在已保存策略中  
- 其它未知顶层字段：严格模式 **拒绝**（防模型乱造参数）

规则：

| 规则 | 说明 |
|------|------|
| 规格来源 | **仅**已保存 `strategy_id` → `_prepare_backtest` 冻结 `execution_spec`（与 REST 同路径） |
| 预检 | 内部能力校验；非 `supported` → Task `failed`，artifact 带 capability |
| 确认 | `confirmed !== true` → 拒绝（Trace 注入 + quant 二次校验，见 §5.4） |
| 限额 | 并发复用 `quant_task` 单任务互斥（409 → `failed`，见 §4.4）；日次数默认 50/用户/日（可配置）；区间复用 10 年上限 |
| 归属 | `quant_backtest_run.user_id = JWT.sub` |
| 证据状态 | 成功落库后与 REST 单次回测一致调用 `advance_after_backtest`；失败/取消不推进。**注意**：此为路径 S；多轮提炼应走路径 E（trial **不**自动改 `evidence_status`，见 §8.11）。Agent 不得把路径 S 的一次推进表述为「试验账本级验证通过」 |

**artifact `backtest_summary`**

```json
{
  "run_id": "<id>",
  "strategy_id": 42,
  "metrics": {
    "total_return": 0.12,
    "max_drawdown": -0.18,
    "sharpe": 0.9,
    "n_trades": 42
  },
  "validation": {
    "verdict": "passed",
    "reasons": []
  },
  "data_quality": {
    "st_history_incomplete": false,
    "notes": []
  },
  "detail_ref": {
    "run_id": "<id>"
  }
}
```

`verdict`：`passed` | `rejected` | `incomplete`——与 `app/backtest/validation.py` 的返回值逐字对齐（`incomplete` = 存在未评估的否决条件，既不算通过也不算否决）。  
**不**在 artifact 中返回完整 equity 序列。

---

### 8.5 `backtest.get`

**payload**

```json
{
  "run_id": "<id>"
}
```

- 仅允许读取 **当前用户** 的 run；否则按 not found（防探测）  

**artifact**：同 `backtest_summary`；可选附带 **采样** equity（点数上限，如 200）。

---

### 8.6 `selection.screen`

**payload**：对齐现有 `POST /api/selection/screener` / `StructuredScreenerRequest`（`date`, `pool_id`, `logic`, `groups`, `watchlist_only`, `limit` 等）。

**artifact `screen_result`**（字段与 `structured_screen` 返回对齐，A2A 仅裁剪体积）

```json
{
  "date": "2026-07-29",
  "total": 12,
  "items": [],
  "field_coverage": {},
  "truncated": false
}
```

- 现网 `structured_screen` **已返回** `total`（截断前命中数）与 `items`  
- A2A 路径：`limit = min(payload.limit or 100, 50)`（硬上限 **50**，clamp 不报错；REST 仍为默认 100 / 最大 500）  
- `truncated = total > len(items)`  
- 可将 `items` 在 artifact 中别名为模型友好名，但契约以 `total` / `items` / `field_coverage` 为准，避免与 REST 双名漂移

---

### 8.7 `strategy.save_draft`

**payload**（对齐 `StrategyCreateIn` 的 A2A 裁剪版）

```json
{
  "name": "ma_cross_v3_oos",
  "spec": {},
  "parent_strategy_id": 41
}
```

| 字段 | 说明 |
|------|------|
| `name` | 必填；与同 owner 现有策略重名 → 校验错误，文案建议改名后缀（对齐 REST 409 语义，走 A2A 校验错误通道） |
| `spec` | 必填；完整 StrategySpec，服务端 `parse_strategy_spec` 解析失败 → 校验错误（附 capability/issues） |
| `parent_strategy_id` | 可选；变体谱系。必须属于当前用户（或可读的 system 源策略）；写入 `quant_strategy` 扩展元数据或约定列（实现优先：`spec.metadata.parent_strategy_id`，避免未迁移前阻塞；若已有 DB 列则双写）。Orchestrator 迭代小改时应带上 parent，禁止无谱系连建孤儿草稿 |
| `confirmed` | **不需要**；出现亦不报错 |

**字段名澄清（避免与 evidence 混淆）**：

| 名称 | 位置 | 含义 |
|------|------|------|
| `research_status` | **策略表列** `quant_strategy.research_status` | 研究工作流粗状态（草稿默认 `unverified`） |
| `evidence_status` | **Spec 内** `spec.metadata.evidence_status` | 证据状态机（`advance_after_backtest` / promotion 路径） |

规则：

| 规则 | 说明 |
|------|------|
| 落库形态 | **`enabled=false` + 列 `research_status=unverified`**；`enabled=false` 跳过 `_require_supported`，非 supported 也可留档，但回测/experiment.create 仍会拦 |
| 归属 | `owner_id = JWT.sub`，`is_system=false` |
| 配额 | 复用 `_check_quota(..., adding=True, enabling=False)`；超限 → 提示停用/删除旧草稿或复用 parent 迭代 |
| 证据 | **不**推进 `evidence_status` |
| 严格模式 | 未知顶层字段拒绝 |

**artifact `strategy_draft`**

```json
{
  "strategy_id": 42,
  "name": "ma_cross_v3_oos",
  "spec_hash": "<hash>",
  "parent_strategy_id": 41,
  "enabled": false,
  "research_status": "unverified",
  "capability": { "status": "supported", "issues": [] }
}
```

---

### 8.8 `backtest.list`

**payload**

```json
{
  "strategy_id": 42,
  "limit": 20,
  "before_run_id": null
}
```

| 字段 | 说明 |
|------|------|
| `strategy_id` | 可选；过滤某策略的历史 run |
| `limit` | 可选，默认 20，硬上限 **50**（clamp 不报错） |
| `before_run_id` | 可选游标；只返回 id 小于该值的 run（倒序翻页） |

规则：

- 仅返回 **当前用户** 的 run，按 run id 倒序；不返回跨用户数据  
- **新增实现任务**：REST 无列表端点（仅 leaderboard / `GET /api/backtest/{run_id}`），需新增 domain 查询（`quant_backtest_run` 按 `user_id` 过滤 + 分页），summary 序列化复用 `backtest.get` 路径，禁止另造字段名

**artifact `backtest_list`**

```json
{
  "items": [
    {
      "run_id": "<id>",
      "strategy_id": 42,
      "status": "done",
      "start": "2024-01-01",
      "end": "2025-12-31",
      "metrics": { "total_return": 0.12, "max_drawdown": -0.18, "sharpe": 0.9, "n_trades": 42 },
      "validation": { "verdict": "passed", "reasons": [] },
      "created_at": "2026-07-29T18:05:00"
    }
  ],
  "has_more": false
}
```

---

### 8.9 `factor.validate`

**payload**（对齐 `FactorValidateIn`）

```json
{
  "expression": {}
}
```

**artifact `factor_validation`**：与 `ExpressionValidationResult` 逐字对齐（`valid` / `capability` / 规范化哈希、`min_bars` 等，字段名以现网 `validate_expression` 返回为准）。

- **授权：`can_client`**。**本期起 REST 与 A2A 同步由 admin 放宽至 client**（validate/preview 为只读轻量计算，属本期实现任务；写类 `save_draft` 仍 admin，§8.12）。
- 不落库、不触发全市场计算。
- 失败写入审计 `failure_kind` / `missing_capability`（§12）。

---

### 8.10 `factor.preview`

**payload**（在 REST `FactorPreviewIn` 上扩展多标的）

```json
{
  "expression": {},
  "factor_key": null,
  "code": "sh.600000",
  "codes": ["sh.600000", "sz.000001"],
  "days": 60
}
```

| 字段 | 说明 |
|------|------|
| `expression` / `factor_key` | 必须且只能其一（同 REST） |
| `code` | 单标的（与 REST 兼容） |
| `codes` | 可选多标的；与 `code` **至少提供其一**；若两者都有则合并去重 |
| 标的上限 | A2A 硬上限 **`min(len, 5)`**（clamp 截断并在 artifact 标 `truncated_codes`）；超出部分不报错丢弃——防 agent 扫全市场 |
| `days` | 默认 60，A2A 硬上限 **120**（clamp） |
| 授权 | **`can_client`**（REST 同步放宽，同 §8.9） |

**artifact `factor_preview`**

```json
{
  "items": [
    {
      "code": "sh.600000",
      "dates": [],
      "values": [],
      "reason_tree": {},
      "error": null
    }
  ],
  "truncated_codes": false,
  "note": "spot_check_only_not_market_efficacy"
}
```

- 单标的时 `items` 长度 1（也可兼容旧 REST 扁平字段，但 A2A 契约以 `items` 为准）。  
- 表达式非法 → 校验错误（附 capability）。  
- 某 code 无日线 → 该项 `error` 可读，不整包失败。  
- preview 仅抽查；**全市场有效性结论必须走 `factor.evaluate`**（§8.13），不得用 preview 冒充（§10.2 / §13）。

---

### 8.11 `experiment.create` / `get` / `list` / `trial` / `trial_batch`

对齐 `app/api/experiments.py` 与 `app/experiment/service.py`；handler **只调** domain，禁止复制 patch/回测逻辑。

#### `experiment.create`

**payload**

```json
{
  "title": "均线交叉是否在中证500有增量",
  "hypothesis": "20/60 均线金叉相对等权基线年化更高且否决条件不触发",
  "permanent_candidate_id": "ma-cross-csi500-v1",
  "strategy_id": 42,
  "spec": null,
  "family_id": "ma-cross",
  "universe_snapshot": null,
  "cost_snapshot": null
}
```

| 字段 | 说明 |
|------|------|
| `title` / `hypothesis` / `permanent_candidate_id` | 必填；candidate id 规则同 REST（2–64 字母数字等） |
| `strategy_id` | **A2A 强烈建议必填**；缺省则 create 成功但后续 trial 会失败（现网：`create_trial_and_run` 要求 strategy_id 才能落库回测）。契约：**缺 strategy_id → 校验错误**，文案引导先 `save_draft`（比 REST 更严，避免 agent 建出不可跑实验） |
| `spec` | 可选；省略则用 `strategy_id` 当前 spec 冻结。若提供则须 `parse` + **capability.supported**（同 `create_experiment`），否则校验错误 |
| `family_id` 等 | 可选，语义同 REST |

免确认。同 owner 重复 `permanent_candidate_id` → 校验错误（引导 `experiment.get` / 换 id）。

**artifact `experiment`**：对齐 `experiment_out`（`id`, `permanent_candidate_id`, `frozen_spec_hash`, `identity_hash`, `status`, `strategy_id`, …）。

#### `experiment.get`

**payload**：`{ "experiment_id": 1 }`  
仅 owner。  
**artifact**：对齐 REST get——experiment + `trials[]` + `multiplicity` + `pending_promotions`（**只读**；不提供 accept/dismiss skill）。

#### `experiment.list`

**payload**：`{ "include_archived": false, "limit": 50 }`（limit 硬上限 50）  
**artifact**：`{ "items": [ experiment_out 摘要… ], "count": n }`

#### `experiment.trial`（高成本）

**payload**

```json
{
  "experiment_id": 1,
  "codes": ["sh.600000"],
  "start": "2024-01-01",
  "end": "2025-12-31",
  "param_patch": { "$.entry.window": 20 },
  "costs": {},
  "pool_id": null,
  "dynamic_universe": false,
  "confirmed": true,
  "client_request_id": "<optional>"
}
```

| 规则 | 说明 |
|------|------|
| 执行 | `create_trial_and_run`；失败也落 trial（`outcome=error`） |
| 确认 / 配额 / 互斥 | §5.4 / §4.4 / §4.6 |
| `param_patch` | 路径须 `$.…`；非法路径 → trial error 或校验错误 |
| 证据 | **永不**自动改 `evidence_status`；artifact 可含 `promotion` 摘要（eligible / suggested_target / todo id），文案引导用户去看板确认 |
| 幂等 | `client_request_id` 当日同用户不重复创建 trial |

**artifact `trial_result`**

```json
{
  "experiment_id": 1,
  "trial": { "id": 9, "trial_index": 2, "param_patch": {}, "outcome": "ok", "backtest_run_id": 100, "metrics_summary": {} },
  "backtest": { "run_id": 100, "metrics": {}, "validation": {} },
  "promotion": { "eligible": false, "todo": null },
  "detail_ref": { "experiment_id": 1, "run_id": 100 }
}
```

#### `experiment.trial_batch`（高成本）

**payload**：同 trial，但 `param_patches: [{}]` 替代单一 patch。  
- A2A 硬上限 **`min(len, 8)`**（REST 32；Agent 批更严，防一次烧光配额）  
- clamp：超过 8 的尾部丢弃并在 artifact 标 `truncated_batch: true`（或校验错误二选一——**拍板：校验错误**，要求 agent 自行拆批，避免静默少跑）  
- 顺序执行；单项失败 `outcome=error` 不中断  
- 配额按 **实际执行条数**计  
- 须 `confirmed: true` + 可选 `client_request_id`（整批幂等）

**artifact `trial_batch_result`**：`{ "items": [ trial_result… ], "executed": n }`

---

### 8.12 `factor.save_draft`

**payload**（对齐 `FactorCreateIn` 裁剪）

```json
{
  "key": "draft_mom_20",
  "name": "20日动量草稿",
  "expression": {},
  "description": "",
  "category": "momentum"
}
```

| 规则 | 说明 |
|------|------|
| 授权 | **仅 `can_admin`** |
| 落库 | `enabled=false`（A2A 固定；payload 出现 `enabled:true` → 校验错误，防 agent 启用未验证因子进夜间管道） |
| 校验 | 先走表达式 validate；失败不落库 |
| 冲突 | key 已存在 → 校验错误 |
| 确认 | 不需要 |

**artifact `factor_draft`**：对齐 REST factor out（`key`, `expression_hash`, `min_bars`, `enabled: false`, …）。

---

### 8.13 `factor.evaluate`（高成本，新 domain 能力）

全市场（或指定池）因子有效性评估。quant 侧新增 `app/factors/evaluation.py`：saved `factor_key` 复用盘后已计算的全市场因子值；ad-hoc `expression` 现算（先过 `validate_expression`）。**禁止**在 A2A 层实现统计逻辑。

**payload**

```json
{
  "expression": null,
  "factor_key": "mom_20",
  "start": "2024-01-01",
  "end": "2025-12-31",
  "pool_id": null,
  "codes": [],
  "layers": 10,
  "rebalance": "weekly",
  "confirmed": true,
  "client_request_id": "<optional>"
}
```

| 字段 | 说明 |
|------|------|
| `expression` / `factor_key` | 必须且只能其一（同 preview）；ad-hoc expression 校验失败即校验错误（附 capability） |
| `start` / `end` | 必填；复用 `validate_backtest_window`（最长 10 年，禁止未来日） |
| `pool_id` / `codes` | 可选限定股票池（互斥规则同 `_prepare_backtest`）；缺省 = 全 A 可交易域（剔除 ST / 停牌 / 上市不足 60 日，口径在 artifact `universe.filters` 回显） |
| `layers` | 分层数，默认 10，硬上限 **10**（clamp） |
| `rebalance` | `weekly`（默认）/ `monthly` |
| `confirmed` | 必须 `true`（§5.4） |
| `client_request_id` | 可选幂等键，语义同 §8.4 |

**artifact `factor_evaluation`**

```json
{
  "evaluation_id": "<id>",
  "factor_key": "mom_20",
  "universe": { "size": 5100, "filters": ["st", "suspended", "lt_60d"] },
  "window": { "start": "2024-01-01", "end": "2025-12-31", "rebalance": "weekly" },
  "ic": { "ic_mean": 0.031, "icir": 0.42, "rank_ic_mean": 0.045, "positive_ratio": 0.57, "n_periods": 96 },
  "layers": [
    { "layer": 1, "annual_return": 0.21, "excess": 0.09 },
    { "layer": 10, "annual_return": -0.04, "excess": -0.16 }
  ],
  "long_short": { "annual_return": 0.25, "max_drawdown": -0.11, "turnover": 1.8 },
  "coverage": { "factor_value_ratio": 0.93, "notes": [] },
  "detail_ref": { "evaluation_id": "<id>" }
}
```

规则：

| 规则 | 说明 |
|------|------|
| 执行模型 | 长任务：A2A Task ↔ `quant_task`；与回测共用互斥槽、日配额（计 1 次）、Cancel 检查点（§4.5 / §4.6） |
| 授权 | `can_client` |
| 落库 | 评估结果落 **`quant_factor_evaluation`** 表（可复现、可复查；REST 读端点同期提供，看板可展示） |
| 话术边界 | 结果为**历史样本内统计**；artifact 强制带 `n_periods` / 覆盖率；Orchestrator 摘要须带样本期与多重检验提示（§10.2 #12），禁止表述为「未来持续有效」 |
| 多重检验 | 同一 factor 反复 evaluate 计入 multiplicity 提示（与 experiment multiplicity 同源口径）；agent 不得靠穷举表达式「刷」出高 IC 后只报喜 |

---

### 8.14 `system.gap_summary`

**payload**

```json
{
  "scope": "me",
  "limit": 20,
  "since_days": 30
}
```

| 字段 | 说明 |
|------|------|
| `scope` | `me`（默认，当前用户审计）或 `global`（**仅 can_admin**，全站聚合） |
| `limit` | 默认 20，硬上限 50 |
| `since_days` | 默认 30，硬上限 90 |

**artifact `gap_summary`**

```json
{
  "items": [
    {
      "missing_capability": "rolling_foo",
      "failure_kind": "missing_engine",
      "count": 12,
      "last_seen": "2026-07-29T18:00:00"
    }
  ],
  "note": "aggregate_of_a2a_audit_not_llm_advice"
}
```

依赖 `quant_a2a_audit` **与 `quant_research_finding`**（§8.15）双源：审计缺口列计数与 findings 计数**分列展示、合并排行**。两表为空时返回空列表。也可由 REST `GET /api/admin/a2a-gaps` 同源实现（管理页用 REST，Agent 用本 skill）。

---

### 8.15 `system.report_finding`

Orchestrator Conclude 的 findings（§10.5）落表通道。免确认，计入读/create 类限速。

**payload**

```json
{
  "findings": [
    {
      "kind": "missing_engine",
      "detail": "需要 rolling_foo 算子",
      "evidence": "validate capability.issues[0]",
      "suggested_system_work": "optional",
      "experiment_id": 1,
      "run_id": 100
    }
  ],
  "session_ref": "<trace session id, optional>"
}
```

| 规则 | 说明 |
|------|------|
| `kind` 枚举 | 与 §10.5 一致：`missing_engine` / `missing_data` / `low_coverage` / `product_gap` / `ux_friction` / `hypothesis_rejected`；未知 kind → 校验错误 |
| 空数组 | 单次结果或证据不足、假说未被明确拒绝且无系统缺口时仍调用本 skill，但传 `findings: []`；不得为满足落表步骤伪造 `hypothesis_rejected` |
| 落库 | **`quant_research_finding`** 表：`user_id`, `kind`, `detail`, `evidence`, `suggested_system_work`, `experiment_id`, `run_id`, `session_ref`, `source`, `created_at` |
| 聚合 | `system.gap_summary` 与 admin REST 把 findings 与审计缺口列合并排行（§8.14） |
| 授权 | `can_client`；仅写本人 findings |
| 防刷 | 单条 `detail` ≤ 512 字符，单批 findings ≤ **20** 条；并入读/create 限速（60/分） |
| 幂等 | 同 `session_ref` + 同 `kind` + 同 `detail` 当日内去重（重复 Conclude 不产生重复行） |

---

## 9. SSE 时序

### 9.1 长任务（`backtest.run` / `experiment.trial` / `experiment.trial_batch` / `factor.evaluate`）

```text
Client  →  Send Streaming Message (skill=backtest.run | experiment.trial | …)

Server SSE:
  1) Task { id, status: working }
  2) statusUpdate { working, message: "validating" | "compiling" | "simulating …" | "trial i/n …" }
  3) artifactUpdate { name: backtest_summary | trial_result | trial_batch_result, parts: [data] }
  4) statusUpdate { state: completed, final: true }
  5) close stream
```

**心跳**：长任务 SSE 可能持续数分钟；无进度时 Server 每 **~15 秒**心跳（`: ping` 或空 statusUpdate）。

失败：

```text
  statusUpdate { state: failed, message: "<actionable error>" }
  可选 artifact: validation_result / error_detail
  close
```

取消（见 §4.5 / §4.6）：

```text
Client Cancel Task
  ├─ 仍为 pending（未进引擎）→ canceled + 关流
  └─ 已在 run_backtest → 不可中断；可操作错误，流可继续至终态
```

### 9.2 短任务

`validate` / `save_draft` / `quality` / `catalog` / `get` / `list` / `screen` / `factor.validate` / `factor.preview` / `factor.save_draft` / `experiment.create|get|list` / `system.gap_summary` / `system.report_finding`：

```text
Task working → artifact* → completed → close
```

毫秒～数秒；ephemeral Task（§4.6）；**不**占高成本互斥槽。

### 9.3 Trace 侧聚合

高成本 tool：

1. 打开 A2A SSE  
2. 可选对话「回测/trial 进行中」（微信可跳过中间帧）  
3. 终态 artifact JSON 作为 `ToolOutput`  
4. 用户摘要：**禁止**交易建议措辞；**禁止**把 promotion.eligible 说成「已验证可交易」  

---

## 10. Trace 集成

### 10.1 工具清单（对模型暴露细粒度 tool，底层统一 A2A）

| Tool 名 | A2A skill | 确认 |
|---------|-----------|------|
| `quant_catalog` | `catalog.get` | 否 |
| `quant_data_quality` | `market.data_quality` | 否 |
| `quant_validate_strategy` | `strategy.validate` | 否 |
| `quant_save_strategy_draft` | `strategy.save_draft` | 否 |
| `quant_create_experiment` | `experiment.create` | 否 |
| `quant_get_experiment` | `experiment.get` | 否 |
| `quant_list_experiments` | `experiment.list` | 否 |
| `quant_run_trial` | `experiment.trial` | **是** |
| `quant_run_trial_batch` | `experiment.trial_batch` | **是** |
| `quant_run_backtest` | `backtest.run` | **是** |
| `quant_get_backtest` | `backtest.get` | 否 |
| `quant_list_backtests` | `backtest.list` | 否 |
| `quant_screen` | `selection.screen` | 否 |
| `quant_validate_factor` | `factor.validate` | 否 |
| `quant_preview_factor` | `factor.preview` | 否 |
| `quant_evaluate_factor` | `factor.evaluate` | **是** |
| `quant_save_factor_draft` | `factor.save_draft` | 否（admin） |
| `quant_gap_summary` | `system.gap_summary` | 否 |
| `quant_report_finding` | `system.report_finding` | 否 |

细 tool 利于模型选型；网络层仍是官方 A2A，避免并行维护 MCP 通道。

### 10.2 Orchestrator Skill（`quant-research`）行为约束

写入 Trace skill 文档，**与 §10.5 状态机一起生效**：

1. **新会话写 Spec 前必须 `catalog.get`**（或本会话未过期缓存）。禁止凭记忆编造字段/算子。  
2. 高成本前 **有则优先** `data_quality` + `strategy.validate`（工具未齐时至少 validate）。  
3. 仅 `capability.status == supported` 且 `valid == true` 才进高成本 skill。  
4. **多轮提炼默认路径 E**（`experiment.*`）；路径 S（`backtest.run`）仅冒烟或用户明确要求。  
5. 新策略：`save_strategy_draft`（带 `parent_strategy_id`）→ `create_experiment`（必填 `strategy_id`）；小参数迭代用 `param_patch`，**不要**每轮新建 strategy 行。  
6. 高成本须拦截层注入 `confirmed`（§5.4）；模型在前置条件满足后直接调用目标高成本工具，由 runtime 自动暂停确认，不得先调用通用 `ask_user` 或只用文字索取确认；用户确认后重调同一工具。优先会话级批量授权。
7. 迭代前先 `list_experiments` / `get_experiment`（必要时 `list_backtests`），对比「本轮假设 vs 已有 trial」。  
8. 结果必须带 `experiment_id` / trial id / `run_id` 等可复现引用。  
9. 文案：研究模拟，非投资建议，不下单；不把 promotion 说成已采纳证据。  
10. 区分 **假说失败**、**证据不足** 与 **系统缺口**；只有工具明确给出 rejected/verdict 拒绝时才写 `hypothesis_rejected`。证据不足但未拒绝且无系统缺口时调用 `system.report_finding` 传空数组（§10.5 / §13）。
11. 费用只用 `costs.*`；勿编造 `initial_cash` / `slippage_bps`。  
12. **因子提炼**：validate → preview（≤5 标的抽查）→ **evaluate（IC/分层，高成本，须授权）** → 可选 save_draft（admin）；evaluate 摘要必须带样本期、`n_periods` 与多重检验提示，禁止表述为「未来持续有效」。  
13. 遵守 §10.5 **停止条件**，禁止无限烧配额。  
14. 遵守 `verification-protocol.md` 精神：可检验假设、失败保留、一次漂亮回测 ≠ 科学验证通过。  
15. **重启兜底**：Get Task not found（quant 进程重启）时用 `experiment.get` / `backtest.get` / `backtest.list` 恢复终态（§4.6），不得盲目重发；确需重发必须复用同一 `client_request_id`。
16. **日频产品边界**：分钟级 K 线、Level-2 盘口、日内/高频撮合属于明确不做范围。触发 S6 并落 `product_gap`，但 `suggested_system_work` 只能说明保持边界或改用外部专用系统，不得建议 quant 建设这些能力。

### 10.3 微信

- 入口：`server/src/weixin/` → 同一 Orchestrator  
- 身份：Trace 内部代签用户 JWT（§5.2）  
- 确认：高成本「待确认摘要 → 白名单 → 5 分钟超时」；**无批量授权**  
- 输出：短摘要 + 指标 + 引导看板 experiment  
- **禁止**微信 → quant A2A 旁路  

### 10.4 配置

```toml
[quant_a2a]
base_url = "http://127.0.0.1:8100"
```

### 10.5 研究 Agent 循环（产品规格，本期必交付）

> Trace `quant-research` 行为规格，**不是** quant 内 LLM。验收 §15.3 #22–#25 与 #29、#30。

#### 状态机

```text
[Init] catalog.get（强制）+ data_quality（建议）
   ▼
[Hypothesize]  可检验假设；用户可改
   ▼
[AuthorSpec]  StrategySpec / param_patch → strategy.validate
   ├─ missing_* → findings → 调整或停止
   └─ supported
        ▼
[Register]  save_draft（parent?）→ experiment.create
   ▼
[Authorize]  直接调用高成本工具 → runtime 自动确认（§5.4；禁止手工 ask_user）
   ▼
[Execute]  experiment.trial | trial_batch  （或路径 S: backtest.run）
   │ 互斥：串行等待；勿改参狂重试
   ▼
[Evaluate]  experiment.get 对比 trials / multiplicity / validation
   ├─ 假说失败且未停 → 回 AuthorSpec（优先 param_patch）
   ├─ 系统缺口 → findings，必要时停止
   ├─ 达停止条件 → Conclude
   └─ 用户要求继续 → Authorize…
   ▼
[Conclude]  摘要 + 可复现 id + findings[] + 是否去看板看 promotion
```

#### 停止条件（任一触发 → Conclude，禁止继续高成本）

| # | 条件 |
|---|------|
| S1 | 用户停止 / 会话结束 |
| S2 | 日配额或会话批量授权用尽且用户拒绝再授权 |
| S3 | 连续 **3** 次 validate 失败且错误同类（同一 missing_capability/字段） |
| S4 | 同一 experiment 连续 **2** 个 trial `rejected` 且无新消融维度 |
| S5 | 达本会话最大高成本执行数（默认 **10**，软刹车：用户一句话可续；批量授权 N 可抬高但 ≤ 日剩余） |
| S6 | 所需能力明确不在 catalog / Agent Card（如分钟级回测、实时盘口）→ 记缺口并落表后停止 |

#### 会话 findings（Conclude 强制）

```json
{
  "findings": [
    {
      "kind": "missing_engine",
      "detail": "需要 rolling_foo 算子",
      "evidence": "validate capability.issues[0]",
      "suggested_system_work": "optional"
    }
  ],
  "reproducible_refs": {
    "experiment_id": 1,
    "run_ids": [100],
    "strategy_id": 42
  }
}
```

`kind` 枚举：`missing_engine` | `missing_data` | `low_coverage` | `product_gap` | `ux_friction` | `hypothesis_rejected`。  
**落表为强制步骤**：Conclude 在对话展示 findings 的同时调用 `system.report_finding`（§8.15）写入 `quant_research_finding`；`gap_summary` 跨会话可聚合（§13）。串行等待 UX：互斥时告知并等 SSE / Get Task / Subscribe 续订，批量授权下顺序执行并展示 i/N。

单次结果或证据不足、假说未被明确拒绝且无系统缺口时，`findings` 必须为空数组；仍调用 `system.report_finding` 记录本次 Conclude 已完成，但不得为满足落表步骤伪造 `hypothesis_rejected`。

---

## 11. quant 模块落点（建议）

```text
quant/app/a2a/
  __init__.py
  card.py
  server.py
  stream.py
  auth.py
  tasks.py
  skills/
    catalog.py
    data_quality.py
    strategy_validate.py
    strategy_save_draft.py
    experiment_create.py
    experiment_get.py
    experiment_list.py
    experiment_trial.py
    experiment_trial_batch.py
    backtest_run.py
    backtest_get.py
    backtest_list.py
    factor_validate.py
    factor_preview.py
    factor_evaluate.py
    factor_save_draft.py
    screen.py
    gap_summary.py
    report_finding.py
```

原则：

- handler **只调** domain（`strategy` / `experiment` / `backtest` / `factors` / `selection` / `data.quality` / `catalog`）  
- **禁止**复制撮合、Spec 校验、param_patch、trial 账本  
- REST 不动；promotion accept **仅**看板/REST  

启动：`app/main.py` 挂载 A2A；Card 与 `/a2a` 在 schema 通过后可用。

---

## 12. 安全、限额与审计

| 项 | 要求 |
|----|------|
| 鉴权 | 除 Card 外全部 Bearer；仅请求入口验签（§5.1） |
| 授权 | `can_client`：策略/实验/回测/因子 validate·preview·evaluate；`factor.save_draft` 与 `gap_summary.scope=global`：**can_admin**；跨用户不可读 |
| 限额 | 高成本：互斥 + 日 50（可配置）+ 10 年区间；trial_batch ≤8；evaluate layers ≤10。读/create/draft/finding：默认 60 次/用户/分钟（可配置） |
| 取消 | pending 即取消；running 经引擎检查点协作中断，状态一致无半写（§4.5） |
| 部署 | 单实例 quant |
| 审计 | `quant_a2a_audit`：`user_id`, `a2a_task_id`, `skill`, `source`, `run_id`, `experiment_id`, `trial_id`, `created_at`, `failure_kind`, `missing_capability`。**validate 失败与运行期失败（trial/run `outcome=error`、能力拒绝）均写后两列**；findings 落 `quant_research_finding`（§8.15） |
| 缺口消费 | `system.gap_summary`（审计列 + findings 双源）+ REST `GET /api/admin/a2a-gaps` 同源 |
| 注入 | 工具返回当数据 |
| 产品 | 无下单；模拟边界；不自动 accept promotion |

---

## 13. 系统优化信号（发现不足闭环）

quant 不跑 LLM 写优化建议。闭环三层：

### 13.1 路径 A — 审计聚合（quant）

`failure_kind` / `missing_capability`（validate **与运行期**失败，§12）+ `quant_research_finding` → `system.gap_summary` / admin API。

| 信号 | 含义 |
|------|------|
| `missing_engine` | 缺算子 |
| `missing_data` | 缺字段/时点 |
| `low_coverage` | 数据覆盖不足（含 evaluate 覆盖率过低） |
| validate 反复失败 | UX/片段库/catalog 教育 |
| `product_gap` | 要的能力明确不在 Agent Card（如分钟级回测、实时盘口） |

对照 `docs/summry/06-gap-and-roadmap.md`：已在路线图标「已知」，否则进候选补强。

### 13.2 路径 B — Orchestrator findings（§10.5）

Conclude **强制** `findings[]` 且经 `system.report_finding` 落表（§8.15）；跨会话可聚合，不再只留在易逝对话里。

### 13.3 路径 C — 人读后立项

管理端 gap 排行 + 抽查 findings → 人工进 roadmap。不自动改引擎。

---

## 14. 交付分解（无阶段，一次做完）

> 系统处于开发设计阶段：**不分期、不设里程碑门槛**，全部功能一次交付、一次性按 §15 验收。以下为按模块的工作分解；实现次序由依赖关系决定，不代表交付批次。

### W1 — 契约与 SDK

- 本文档评审通过，锁定 skill 列表与 payload（含 `experiment.*`、`factor.evaluate`、`system.report_finding`、`parent_strategy_id`、`client_request_id`、审计列、trial_batch cap 8）
- 选定 quant A2A SDK minor；Trace 手写 client 范围（§4.3）

### W2 — quant A2A Server

- Agent Card + ephemeral 短任务 Task + List Tasks（§4.1 / §4.6）
- JWT 鉴权 + 全量 skill handler（§8）：catalog / data_quality / strategy.* / experiment.* / backtest.* / factor.* / selection.screen / system.*
- SSE + Subscribe 续订 + Cancel（pending 即取消 + running 检查点协作中断，§4.5）
- 高成本互斥 + 日配额 50（可配置）+ `client_request_id` 幂等（§4.4）
- `quant_a2a_audit` + `quant_research_finding` 两张表（§12 / §8.15）

### W3 — quant domain 新增/调整（A2A 依赖项）

- **`app/factors/evaluation.py`**：IC / RankIC / ICIR / 分层多空（`factor.evaluate` 的引擎）；`quant_factor_evaluation` 落库表；REST 读端点同期提供（看板可展示）
- `run_backtest` 主循环 + 因子评估循环的 `threading.Event` 取消检查点（§4.5）
- `backtest.list` domain 查询（§8.8，REST 本无列表端点）
- factor validate / preview 授权 admin → client（REST 与 A2A 同步，§8.9）
- 引擎内部失败标准化为 `failure_kind` / `missing_capability` 写入审计（validate 与运行期，§12）

### W4 — Trace A2A Client 与工具

- `crates/hank-a2a-client`（JSON-RPC + SSE + Subscribe 续订 + 幂等重发）
- `quant_*` 工具全量（§10.1）+ `confirmed` 拦截层（§5.4，模型自填剥离）
- 确认闸门 UI + 会话级批量授权（与日配额同池，授权 UI 展示当日剩余）

### W5 — Trace 研究 Agent

- `quant-research` skill 完整实现 §10.2 + §10.5：状态机、停止条件、findings 强制落表、默认路径 E、因子提炼流程（validate → preview → evaluate → save_draft）
- 端到端验收 §15.3

### W6 — 微信入口

- 内部代签 JWT（server 内部函数，**不暴露 HTTP 端点**，§5.2）
- 白名单确认 + 5 分钟超时 + 无批量授权 + 短摘要模板（§5.4 / §10.3）
- 待确认单存储：Orchestrator 进程内 map（key=微信会话 id，5min TTL，不落表，重启即作废）

### W7 — 缺口消费与管理端

- `system.gap_summary` + REST `GET /api/admin/a2a-gaps` 同源聚合（审计缺口列 + findings 合并排行）
- 管理页 gap 排行展示

**实现次序**：W1 →（W2 ∥ W3）→ W4 → W5 →（W6 ∥ W7）。W2 的各 skill handler 依赖 W3 对应 domain 项，逐个对齐推进即可。

---

## 15. 验收标准

### 15.1 协议与安全

| # | 标准 |
|---|------|
| 1 | 未登录 / 坏 JWT 无法调用 skill |
| 2 | 路径 S：确认后对已保存 `strategy_id` 完成回测，回复含 `run_id`，看板可打开；`evidence_status` 与 REST 单次回测路径一致推进 |
| 3 | validate 非 supported / `valid != true` 时不落成功回测/trial |
| 4 | pending Cancel 成功且无僵尸；running Cancel 经检查点协作中断，run/trial 状态一致为 `cancelled`、无半写 metrics |
| 5 | 微信摘要无「已下单/建议买入」类违规措辞 |
| 6 | 仅 text 无 skill 的请求被拒绝 |
| 7 | Card：`streaming: true`，`pushNotifications: false` |
| 8 | 人机 REST 与 web 看板回归不受影响 |
| 9 | 高成本互斥：已有进行中任务再提交 → failed/可等待文案，不双开引擎 |
| 10 | 模型自填 `confirmed` 被 Trace 拦截层剥离；微信无白名单确认不执行 |
| 11 | 缺 `strategy_id` 或非法费用字段 → 校验失败，文案可操作 |
| 12 | `alert_level` ∈ {ok,warning,critical}；validate 用 `valid`/`spec_hash` |
| 13 | `strategy.save_draft` 免确认；`enabled=false` + 列 `research_status=unverified`；可带 `parent_strategy_id` |
| 14 | 相同 `client_request_id` 当日重发：不新建 run/trial、不重复计配额 |
| 15 | `backtest.list` 仅本人、可过滤翻页，字段与 get summary 一致 |
| 16 | 因子 validate/preview/evaluate 对 `can_client` 开放（REST 同步放宽）；preview ≤5 codes；`save_draft` 非 admin 拒绝且固定 `enabled=false` |
| 17 | validate 失败写 `failure_kind` / `missing_capability`，可聚合 |
| 27 | 微信代签为 server 内部函数，**无 HTTP 路由暴露**（代码审查项） |
| 28 | List Tasks 返回本人长任务；Subscribe 可在断线后续订进行中的长任务 |

### 15.2 Experiment 主链

| # | 标准 |
|---|------|
| 18 | `experiment.create` 无 `strategy_id` 被拒；有 id 时冻结 spec，看板可见同一 experiment |
| 19 | 确认后 `experiment.trial` 调用与 REST 同源逻辑：失败也落 trial；**不**自动改 `evidence_status`；artifact 可含 promotion 只读摘要 |
| 20 | `experiment.trial_batch`：>8 patch 校验错误；≤8 顺序执行；配额按条数计；与 `backtest.run` 共用日配额与互斥 |
| 21 | `experiment.get` 返回 trials + multiplicity 提示 + pending_promotions；无 accept skill |

### 15.3 研究 Agent（Trace）

| # | 标准 |
|---|------|
| 22 | 端到端：用户一句话研究意图 → catalog → validate → save_draft → create_experiment →（批量）授权 → ≥1 trial → get 对比 → Conclude 含 `findings[]` 与可复现 id |
| 23 | 默认走路径 E；仅用户要求冒烟时用 `backtest.run` 且话术不称为「试验结论」 |
| 24 | 触发停止条件 S3/S5 时不再发高成本 skill，并输出 findings |
| 25 | 用户要求 Agent Card 明确不具备的能力（如分钟级回测、实时盘口）时：不伪造不调用，findings 含 `product_gap` 并落表 |
| 29 | 因子提炼端到端：validate → preview →（授权）evaluate → 复查 → Conclude；摘要带样本期 / `n_periods` / 多重检验提示，不称「未来持续有效」 |
| 30 | Conclude 的 findings 经 `system.report_finding` 落表，对话展示与表内记录一致 |

### 15.4 缺口闭环

| # | 标准 |
|---|------|
| 26 | `system.gap_summary`（或 admin REST）能按 `missing_capability` 排出 top-N（有审计数据时） |
| 31 | 运行期失败（trial `outcome=error`、能力拒绝）写入 `failure_kind` / `missing_capability` 并可聚合 |
| 32 | `gap_summary` 合并排行审计缺口列与 findings 计数（双源分列展示） |

---

## 16. 风险与缓解

| 风险 | 缓解 |
|------|------|
| A2A spec/SDK 字段漂移 | 业务 payload 稳定；外壳跟 SDK；集成测试钉版本 |
| 双入口行为不一致 | handler 只调同一 service（`_prepare_backtest` / `create_trial_and_run` / `validation_out` / …） |
| Agent 绕开 experiment 只狂建草稿 | §2.1 / §10.2 默认路径 E；验收 #23；parent 谱系 + 策略配额 |
| LLM 乱造 Spec 或费用字段 | catalog 强制 + validate 硬闸；`costs` 白名单；未知字段拒绝 |
| 回测/trial/因子评估打满机器 | 高成本互斥 + 日 50（可配置）+ trial_batch≤8 + 10 年区间；批量授权共用日配额；evaluate 复用盘后已算因子值 |
| 批量授权变「无限自动跑」 | 会话态授权 + 停止条件 S2/S5 + 日配额硬顶；微信无批量 |
| 草稿/实验刷屏 | 策略 `_check_quota`；create 限速；停止条件 |
| SSE 重发重复计费 | `client_request_id` 幂等 |
| 读类死循环 | 60/分钟限速 + Orchestrator 时序 |
| trial 同步过久占请求 | A2A 后台线程 + SSE；与 backtest 互斥 |
| JWT 无吊销 | TTL 30 天；secret 仅内网；轮换全量失效 |
| running 协作中断非即时 | Card/文案写明检查点粒度（交易日/标的批次）；中断后状态一致、无半写（验收 #4） |
| 因子评估结果被说成「未来有效」 | artifact 强制样本期/`n_periods`/覆盖率；话术约束（§10.2 #12）+ 验收 #29 |
| evaluate 被用来穷举表达式刷 IC | multiplicity 提示（§8.13）+ 日配额 + 停止条件；findings 记 `ux_friction`/`product_gap` 可聚合 |
| promotion 被当成已验证 | 无 accept skill；话术禁止 |
| 微信刷屏 | 只回终态摘要 |
| quant「会聊天」误解 | deterministic Card + 拒绝纯 text |

---

## 17. 配置与部署示意

**quant**（已有端口 8100）：

- 暴露 Card 与 `/a2a`  
- 生产与 `hank-quant.service` 同进程即可，无需第二服务  

**Trace**：

- 配置 `quant_a2a.base_url`  
- 与 quant 网络互通（同机或内网）  
- 共用 JWT secret 已存在则无需新密钥  

**dev**：

- Trace `env` 与 quant `dev` 可只开 API；A2A 不依赖 APScheduler  

---

## 18. 文档与代码索引（实现后回填）

| 项 | 预期路径 |
|----|----------|
| 本方案 | `quant/docs/a2a-design.md` |
| quant A2A 包 | `quant/app/a2a/` |
| 因子评估引擎 | `quant/app/factors/evaluation.py`（W3 新增） |
| 审计 / findings 表 | `quant_a2a_audit` / `quant_research_finding` / `quant_factor_evaluation` |
| Trace Client / tools | `crates/hank-a2a-client`（§4.3） |
| Orchestrator skill | Trace skills 下 `quant-research`（§10.2 / §10.5） |
| 领域服务 | `app/strategy/*`, `app/experiment/*`, `app/backtest/*`, `app/api/factors.py`, `app/selection/*`, `app/data/quality.py`, `app/catalog.py` |
| 验证纪律 | `docs/research/verification-protocol.md`, `docs/summry/06-gap-and-roadmap.md` |

---

## 19. 一句话

> **Trace 是唯一 LLM 研究 Agent（§10.5 循环 + 停止条件 + findings 强制落表）；quant 是官方 A2A 确定性工具节点。严肃验证默认走 experiment/trial 主链（路径 E），草稿+单次回测仅冒烟（路径 S）；因子提炼走 validate → preview → evaluate（IC/RankIC/分层）→ save_draft 全链；高成本 skill（回测 / trial / 因子评估）共用确认闸门、日配额与互斥；Cancel 覆盖 pending 即取消与 running 检查点协作中断；系统缺口经审计列 + findings 落表双源聚合闭环。业务话术与确认在 Trace，研究语义与模拟边界在 quant。全部功能一次交付，不分期。**

---

## 修订记录

| 日期 | 变更 |
|------|------|
| 2026-07-30 | 初稿：拍板入口 / 纯工具 / 官方 A2A / SSE / JWT 透传；v1 skill 与分阶段计划 |
| 2026-07-30 | 评审修订：① `verdict` 对齐 validation；② Task↔`quant_task`；③ data_quality 无参快照；④ screen limit；⑤ 确认闸门；⑥ 微信代签；⑦ 手写 a2a-client；⑧ 审计表；⑨ 结构化 Client；⑩ JWT 风险 |
| 2026-07-30 | 二次评审：Cancel 仅 pending；回测必须 strategy_id；costs 对齐；短任务 ephemeral；valid/spec_hash；evidence 推进；日配额 20；微信白名单 |
| 2026-07-30 | 三次评审：拒绝 params；配额计 audit；读限速；微信疑问即放弃；SSE 心跳 |
| 2026-07-30 | 四次评审：save_draft / backtest.list / factor 最小对；批量授权；client_request_id；审计缺口列 |
| 2026-07-30 | **五次修订（选项 A）**：① 目标分层——策略 Agent 主叙事 / 因子辅叙事 / 缺口闭环；② **experiment.\* 一等 skill** 对齐现网试验账本，路径 E 默认、路径 S 冒烟；③ §10.5 研究 Agent 状态机 + 停止条件 + findings 结构 + 端到端验收 #22–#25；④ `parent_strategy_id` 谱系；⑤ `factor.save_draft` + preview 最多 5 标的；⑥ `system.gap_summary` + 审计 experiment/trial 列；⑦ 高成本统一配额（trial 计入）；⑧ Phase 2 含 experiment、Phase 3=策略 MVP、微信 4b；⑨ 关联 verification-protocol 与 gap-roadmap；⑩ 明确 trial 不自动 evidence、无 promotion accept skill |
| 2026-07-30 | **六次修订（一次做完，去阶段化）**：① §14 分阶段计划改为无阶段交付分解 W1–W7，全文扫净 Phase/v1/二期语义；② **因子提炼升主叙事**——新增 `factor.evaluate`（IC/RankIC/ICIR/分层多空，W3 新建 `app/factors/evaluation.py` + `quant_factor_evaluation` 表），validate/preview 授权放宽至 `can_client`（REST 同步）；③ **findings 强制落表**——新增 `system.report_finding` + `quant_research_finding`，`gap_summary` 双源聚合；④ Cancel 升级——running 经 `threading.Event` 检查点协作中断（交易日/标的批次粒度）；⑤ 配额松绑——日 20→50（可配置）、S5 默认 5→10 软刹车；⑥ 审计覆盖运行期失败（trial/run `outcome=error` 写缺口列）；⑦ List Tasks / Subscribe 续订纳入交付；⑧ 修正 §10.5 验收引用错位（#18–#21 → #22–#25）；⑨ 新增验收 #27–#32；⑩ 微信待确认单存储写死（进程内 map + 5min TTL） |
