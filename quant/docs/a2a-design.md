# quant × Trace A2A 方案设计

| 属性 | 内容 |
|------|------|
| 文档 ID | `DESIGN-A2A-2026-07` |
| 状态 | 已拍板，待实现 |
| 范围 | quant 作为确定性 A2A Server；Trace（含微信）作为唯一 LLM 编排 Client |
| 产品边界 | A 股日频研究与模拟回测；不连接券商；不下单；不输出真实交易指令 |
| 关联 | `PRODUCT.md`、`AGENTS.md`、`docs/summry/00-framework-overview.md`、[A2A Protocol](https://a2a-protocol.org/latest/specification/) |

---

## 1. 背景与目标

### 1.1 要解决的问题

希望在 **Trace 对话** 与 **微信通道** 中，由智能体调用 quant 的研究能力（校验策略、回测、筛选、数据质量），并把结果以可复现的 `run_id` / artifact 形式回传用户。

quant 已具备完整 REST 与领域服务，但缺少对 **Agent 间互操作** 的标准入口；若在 Trace 与 quant 各跑一套开放式 LLM Agent，会出现双重推理成本、职责漂移与不可审计问题。

### 1.2 目标

1. quant 以 **官方 A2A** 暴露研究能力：Agent Card 对任意合规 A2A Client 可发现；**调用面向结构化 Client**（能按 §7 约定发送 data part，见 §7.2 规则 4）。
2. quant **不运行 LLM**，仅做确定性工具执行（Deterministic A2A Agent）。
3. Trace 是 **唯一会思考的编排方**；微信不直连 quant，统一经 Trace Orchestrator。
4. 长任务（回测）经 **SSE** 推送生命周期；鉴权采用现有 **用户 JWT 透传**。
5. 人机看板继续走现有 REST；A2A 是 Agent 主路径，不替换 Web API。

### 1.3 非目标（v1）

- quant 内嵌研究 Worker / 多步 LLM loop  
- 官方 A2A Push Notification（webhook）  
- service account + on-behalf-of 双身份体系  
- **仅有 inline `spec`、无已保存 `strategy_id` 的 ephemeral 回测**（须先落库策略再跑，对齐现网 REST）  
- **协作式中断已 `running` 的回测线程**（Cancel 仅覆盖 `pending`，见 §4.5）  
- 实验网格扫参、自然语言选股  
- 券商连接、下单、半自动交易  
- 用 A2A 替换 `web/` 看板 REST  

---

## 2. 已拍板决策

| # | 决策点 | 选择 | 理由 |
|---|--------|------|------|
| 1 | 第一调用方 | Trace 对话 + 微信 | 微信复用 `server/src/weixin/`，同一 Orchestrator |
| 2 | quant 智能程度 | **纯工具**（无 LLM） | 研究语义与边界留在 quant；编排与话术在 Trace |
| 3 | 协议 | **官方 A2A** | 发现、Task、异步、可扩展第三方 Client |
| 4 | 流式 | **上 SSE** | 对齐 A2A Streaming；回测进度可观测 |
| 5 | 鉴权 | **用户 JWT 透传** | 两端已共用 `jwt_secret` 与 claims，实现成本最低 |
| 6 | 回测规格来源 | **必须 `strategy_id`**（已保存策略） | 对齐 `BacktestIn` / `_prepare_backtest`；不做 v1 ephemeral spec 路径 |
| 7 | Cancel | **仅 `pending` 可取消** | 现网 `cancel_task` 无法安全中断 `running` 线程；协作取消属二期 |
| 8 | 成本字段 | **`costs` 对齐引擎** | `commission` / `stamp_tax` / `slippage`（价格比例）；无 `initial_cash`、无 `slippage_bps` |

概念澄清：

> 官方 A2A 的对端称为 Agent，但 quant **不跑模型**。  
> quant 实现完整 A2A **服务端语义**（Card / Task / SSE / Cancel），执行路径为：鉴权 → 解析 skill+payload → 调内部 service → 回 artifacts。  
> 对外仍是标准 A2A 节点；对内是确定性工具网关。

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
     │  skill: quant-research               │
     │  tools: quant_*（封装 A2A Client）   │
     │  确认闸门 / 话术 / 微信摘要格式化    │
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
     │  长任务 ↔ quant_task / backtest jobs │
     └────────────────────────────────────┘
                      │
                      ▼
              strategy / backtest / selection
              data.quality / catalog / …
```

### 3.1 职责边界

| 组件 | 负责 | 不负责 |
|------|------|--------|
| **Trace Orchestrator** | 理解用户意图、拼 StrategySpec、选 skill、确认写操作、对用户措辞、微信长度适配 | 回测撮合、前视规则、能力解析实现 |
| **quant A2A** | Card 发现、Task 生命周期、SSE、JWT 校验、skill 确定性执行、artifact | 闲聊、自由文本「想策略」、交易执行 |
| **quant REST / web** | 人机看板、现有 API | 不强制经 A2A |

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

### 4.1 对齐范围（v1）

以 [A2A Protocol Specification](https://a2a-protocol.org/latest/specification/) 为准，v1 实现最小可用子集：

| 抽象操作 | v1 | 说明 |
|----------|----|------|
| Get Agent Card / 发现 | 是 | `/.well-known/agent-card.json` 或 SDK 约定路径 |
| Send Message | 是 | 短任务可阻塞至终态 |
| Send Streaming Message | **是** | 主路径；SSE 推状态与 artifact |
| Get Task | 是 | 断线后查询 |
| Cancel Task | 是 | 映射 `cancel_task`；**仅 pending 成功**（§4.5） |
| Subscribe to Task | 是（建议） | 断线后续订长任务 |
| List Tasks | 可选 | 可二期 |
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
- **Trace 侧（Rust）**：**无成熟官方 A2A Rust SDK，手写最小 client**——v1 子集仅 4 个 JSON-RPC 方法 + SSE 消费，成本可控；落点定为新 crate `crates/hank-a2a-client`，`quant_*` tools 在 server 侧引用该 crate。

本文 schema 为**语义契约**，字段名以 proto/SDK 为准做映射，业务 payload 不变。

### 4.4 Task 状态映射（仅长任务）

`backtest.run` 持久化映射 `quant_task`（见 `app/models.py` Task / `app/tasks.py`）：

| A2A Task state | quant 内部（`quant_task.status`） |
|----------------|------------|
| submitted | `pending`（入队） |
| working | `running`（编译、模拟中） |
| completed | `done` + artifacts |
| failed | `failed` + 模型可读错误 |
| canceled | `cancelled`（双 l，幂等；**仅从 pending 可达**） |

终态：`completed` | `failed` | `canceled` | `rejected`（按 spec 命名对齐实现）。

**既有约束：`quant_task` 单任务互斥。** 现网每个用户同时只能有一个 `pending`/`running` 任务，提交冲突返回 **409**。A2A `backtest.run` 复用该约束：撞 409 时 Task 置 `failed`，message 写明「已有进行中的任务，可等待完成；若仍为排队中可先 Cancel」（模型可读，可操作），**不**排队。v1 的「每用户并发限额」即此互斥，不再新增并发配置。

> 为何拒绝也产出 Task 而非在 Send Message 层直接返回 JSON-RPC error：**有意为之**。拒绝以 Task `failed` 形式返回，错误文案统一走 SSE / Get Task 通道，Orchestrator 只需一条消费路径；实现者不要再拆「协议层拒绝」第二通道。

**区间上限**：复用现网 `validate_backtest_window`（**最长 10 年**，`MAX_BACKTEST_YEARS`），A2A **不**另起更松默认。  
**日次数**：新配置项，默认建议 **20 次/用户/自然日**（超限 → Task `failed`，文案含剩余额度与重置时间）；可在 `quant` config 覆盖。**计数口径**：以 `quant_a2a_audit` 中该用户当日 `skill = 'backtest.run'` 的记录数为准（含失败/被拒任务——防止用失败调用绕过配额；这也是审计表必须与回测流同在 Phase 2 落地的原因，见 §12 / §14）。

### 4.5 Cancel 语义（v1 拍板：仅 pending）

对齐现网 `cancel_task`（`app/tasks.py`）：

| `quant_task.status` | Cancel 结果 | A2A 表现 |
|---------------------|-------------|---------|
| `pending` | 成功 → `cancelled`；关联 `BacktestRun` 一并 cancelled | Task → `canceled`，关流 |
| `running` | **不可中断**（执行线程无协作取消点） | Cancel 请求失败或 Task 保持 `working`；返回模型可读文案：「任务已在执行、无法中断，请等待结束」；**不**伪装为 `canceled` |
| 已终态 | 幂等：保持原终态 | 返回当前状态 |

验收口径：pending 可取消且无僵尸 pending；**running 不承诺可取消**。协作式中断（`threading.Event` 检查点）列入 §1.3 非目标 / Phase 5 候选。

### 4.6 短任务的 A2A Task 存储

`strategy.validate` / `catalog.get` / `market.data_quality` / `selection.screen` / `backtest.get` **不**写入 `quant_task`。

| 项 | v1 约定 |
|----|---------|
| 存储 | **进程内 ephemeral**（内存 dict），生成 A2A `task_id` 供 SSE / Get Task |
| TTL | 完成后保留 **15 分钟**，超时 Get Task → not found |
| 重启 | 进程重启后短任务 Task 全部丢失（可接受；结果已在当次 SSE 推完） |
| 多实例 | **v1 假设 quant 单实例**（与现网 `hank-quant.service` 一致）；多副本需共享 Task 存储，不在 v1 |

仅 `backtest.run` 的 A2A Task ↔ `quant_task`（及 `quant_backtest_run`）持久映射，Get Task / Cancel 以 DB 为准。

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

### 5.3 不在 v1 做

- Trace service token + `X-On-Behalf-Of`  
- 匿名 A2A skill 调用（Card 可公开读，**调用必须鉴权**）  

### 5.4 产品级确认闸门（非第二套鉴权）

写类 / 高成本 skill（如 `backtest.run`、未来 `strategy.save_draft`）要求 payload：

```json
"confirmed": true
```

由 Trace 在用户确认后置位；quant 缺失则 `failed` / 校验错误，防止模型误触。

闸门实现要求（Trace 侧，写死）：

1. **`confirmed` 只能由 Trace 工具调用拦截层在用户真实确认后注入**，模型自身输出不得直接置位；模型传 `confirmed: true` 而未经用户确认时拦截层应剥离并先走确认流程。
2. **对话入口**：UI 确认按钮（复用现有工具确认机制）。
3. **微信入口**：Orchestrator 先发待确认摘要（含关键参数：策略 id、区间、费用覆盖）；用户回复肯定确认后置位执行；**5 分钟超时未确认自动作废**。  
   - **肯定白名单**（去空白、全半角不敏感，整句匹配其一即可）：`确认` / `好的` / `是` / `OK` / `ok` / `同意`  
   - 白名单外任意回复视为放弃（不执行）。**此为有意取舍**：用户回「费用是多少？」这类追问也会作废待确认单——宁可不执行，也不在语义模糊时执行；实现时**不要**「优化」成忽略非白名单回复继续等待。用户可重新发起。
   - 微信确认状态挂在会话上下文，不落新表

---

## 6. Agent Card（草案）

```json
{
  "name": "quant-research",
  "description": "A-share daily research tools: validate StrategySpec, run simulated backtests, screen universe, report data quality. No broker connectivity or order execution. Deterministic server (no LLM). Invocation requires structured data parts (skill+payload); text-only messages are rejected.",
  "version": "0.1.0",
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
      "description": "Fixed dictionaries: filter fields, labels, snippet metadata.",
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
      "id": "backtest.run",
      "name": "Run backtest",
      "description": "Simulated T+1 backtest for a saved strategy_id. Long-running. Requires confirmed=true. Returns summary artifact and run_id. Cancel only while pending.",
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
      "id": "selection.screen",
      "name": "Structured screener",
      "description": "AND/OR condition screen with per-field coverage warnings.",
      "tags": ["selection", "read"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    }
  ]
}
```

说明：

- Card 文案明确 **No trading / No LLM**  
- `protocolVersion` 实现时与所选 SDK/spec 版本对齐  
- skill 列表即 v1 范围；扩 skill 走 Card 版本升级，不破坏旧 payload  

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
  "sections": ["filter_fields", "snippets"]
}
```

- `sections` 省略 = 返回实现支持的全部目录段  

**artifact `catalog`**：与现有 `GET /api/catalog` 语义对齐的 JSON（可裁剪体积）。

---

### 8.2 `market.data_quality`

**payload**：v1 **无参数**（空对象 `{}`）。

- 对齐现有 `GET /api/market/data-quality`（`app/api/market.py`）与 `data_quality_public_summary`（`app/data/quality.py`）：全局只读快照，读旁路缓存，不触发采集。
- **不支持** `date` / `universe` 参数：现有 domain service 没有按日期/股票池切分的口径；`watchlist` 是每用户关系，与全市场质量快照语义不匹配。按维度切片的质量报告属 domain 扩展，列入二期候选，不进 v1 契约。

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
- `spec_hash`：`strategy_spec_hash(parsed)`（同 REST；若未来要忽略 `evidence_status` 的 identity 语义，另加字段 `identity_hash`，v1 不强制）  

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
  "confirmed": true
}
```

| 字段 | 说明 |
|------|------|
| `strategy_id` | **必填**。调用方必须先通过 REST（或未来 `strategy.save_draft` skill）保存策略；A2A 只跑已落库、当前用户可读的策略 |
| `start` / `end` | 必填；走 `validate_backtest_window`（最长 **10 年**，禁止未来日） |
| `codes` | 可选；空则按策略 universe / 池解析（同 REST） |
| `pool_id` | 可选；临时覆盖研究范围，与 `codes` 互斥规则同 `_prepare_backtest` |
| `costs` | 可选覆盖；键名对齐 `DEFAULT_COSTS`：`commission` / `stamp_tax` / **`slippage`（价格比例，不是 bps）**。省略则用引擎默认 |
| `confirmed` | 必须为 `true`（由 Trace 拦截层注入） |

**明确不在 v1 payload：**

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
| 限额 | 并发复用 `quant_task` 单任务互斥（409 → `failed`，见 §4.4）；日次数默认 20/用户/日；区间复用 10 年上限 |
| 归属 | `quant_backtest_run.user_id = JWT.sub` |
| 证据状态 | 成功落库回测后与 REST 一致调用 `advance_after_backtest`（有 `strategy_id` 路径），推进策略 `evidence_status`；失败/取消不推进 |

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

## 9. SSE 时序

### 9.1 长任务（`backtest.run`）

```text
Client  →  Send Streaming Message (skill=backtest.run)

Server SSE:
  1) Task { id, status: working }
  2) statusUpdate { working, message: "validating" | "compiling" | "simulating …" }
  3) artifactUpdate { name: backtest_summary, parts: [data] }
  4) statusUpdate { state: completed, final: true }
  5) close stream
```

**心跳**：回测 SSE 可能持续数分钟；模拟阶段若无进度事件，Server 每 **~15 秒**发一帧心跳（SSE 注释行 `: ping` 或空 statusUpdate），防止中间代理掐断空闲连接。

失败：

```text
  statusUpdate { state: failed, message: "<actionable error>" }
  可选 artifact: validation_result / error_detail
  close
```

取消（见 §4.5）：

```text
Client Cancel Task
  ├─ quant_task 仍为 pending → cancelled + A2A canceled + 关流
  └─ 已是 running → 不可中断；返回可操作错误，流可继续至 completed/failed
```

### 9.2 短任务（validate / quality / catalog / get / screen）

统一走 Streaming 亦可，便于 Client 单路径：

```text
Task working → artifact* → completed → close
```

耗时通常毫秒～数秒。A2A Task 仅 ephemeral（§4.6），不占 `quant_task` 互斥槽。

### 9.3 Trace 侧聚合

`quant_run_backtest` 等 tool：

1. 打开 A2A SSE  
2. 可选：向用户对话发「回测进行中」类状态（微信可跳过中间帧）  
3. 终态将 **artifact JSON 字符串** 作为 `ToolOutput` 交给模型  
4. 模型生成用户可见摘要；**禁止**把 oos/回测说成交易建议  

---

## 10. Trace 集成

### 10.1 工具清单（对模型暴露细粒度 tool，底层统一 A2A）

| Tool 名 | A2A skill | 确认 |
|---------|-----------|------|
| `quant_catalog` | `catalog.get` | 否 |
| `quant_data_quality` | `market.data_quality` | 否 |
| `quant_validate_strategy` | `strategy.validate` | 否 |
| `quant_run_backtest` | `backtest.run` | **是** |
| `quant_get_backtest` | `backtest.get` | 否 |
| `quant_screen` | `selection.screen` | 否 |

细 tool 利于模型选型；网络层仍是官方 A2A，避免并行维护 MCP 通道。

### 10.2 Orchestrator Skill（`quant-research`）

行为约束（写入 Trace skill 文档）：

1. 回测前 **有则优先** 调用 `data_quality` / `validate`（Phase 3 仅暴露 validate+backtest 时，至少 validate；data_quality 在工具上线后强制优先）  
2. 仅 `capability.status == supported` 且 `valid == true` 才 `run_backtest`  
3. 回测前确保策略已保存并持有 **`strategy_id`**（v1 不接受裸 spec 回测）  
4. 高成本回测与保存策略类操作必须用户确认后再由拦截层注入 `confirmed: true`  
5. 结果必须带 `run_id` 或等价引用，便于复现  
6. 文案：研究模拟，非投资建议，系统不下单  
7. 区分「假说/参数问题」与「系统缺口」（`missing_data` / `missing_engine` / coverage warnings）  
8. 费用覆盖使用 `costs.slippage` 等比例字段，勿编造 `initial_cash` / `slippage_bps`  

### 10.3 微信

- 入口：`server/src/weixin/` → 同一 Orchestrator  
- 身份：Trace 内部代签用户 JWT（§5.2，新增实现任务）  
- 确认：写类操作走「待确认摘要 → 肯定白名单回复 → 5 分钟超时作废」流程（§5.4）  
- 输出：短摘要 + 关键指标 + 引导到 Trace/看板看详情  
- **禁止**微信 → quant A2A 旁路  


### 10.4 配置

Trace 侧配置项（示例）：

```toml
[quant_a2a]
base_url = "http://127.0.0.1:8100"
# Card 路径或完整 card URL；超时、最大回测并发等
```

---

## 11. quant 模块落点（建议）

```text
quant/app/a2a/
  __init__.py
  card.py          # Agent Card 生成
  server.py        # JSON-RPC / 路由挂载
  stream.py        # SSE 任务流
  auth.py          # 复用 JWT，薄封装
  tasks.py         # A2A Task：短任务 ephemeral；backtest ↔ quant_task 映射（§4.4–4.6）
  skills/
    catalog.py
    data_quality.py
    strategy_validate.py
    backtest_run.py
    backtest_get.py
    screen.py
```

原则：

- skill handler **只调**现有 domain（`strategy` / `backtest` / `selection` / `data.quality` / `catalog`）  
- **禁止**复制撮合或 Spec 校验逻辑  
- 人机 REST 保持不动；A2A 为并行入口  

启动：`app/main.py` 挂载 A2A 路由；Card 与 `/a2a` 在 schema 校验通过后可用。

---

## 12. 安全、限额与审计

| 项 | 要求 |
|----|------|
| 鉴权 | 除 Card 发现外全部 Bearer；验签仅在请求入口（§5.1） |
| 授权 | 仅 `can_client` 或 `can_admin`；跨用户 run 不可读 |
| 限额 | `backtest.run`：并发复用 `quant_task` 单任务互斥（§4.4）；日配额默认 20/用户/日（计数口径见 §4.4）；区间复用现网 10 年。**读类 skill**（validate / quality / catalog / screen / get）：简单限速 **60 次/用户/分钟**（进程内计数即可），防模型重试循环打满全市场计算；超限返回模型可读错误 |
| 取消 | 用户停对话 → Trace 调 Cancel → **仅 pending 幂等取消**；running 返回不可中断文案（§4.5） |
| 部署 | v1 **单实例** quant；短任务 Task 进程内存储（§4.6） |
| 审计 | 落 **新建轻量表 `quant_a2a_audit`**（Phase 2 随 backtest 流一起落，不复用 `quant_job_run`——后者是调度任务日志，语义不同）：`user_id`, `a2a_task_id`, `skill`, `source`, `run_id`, `created_at`。Phase 1（审计表未建前）至少保证应用日志输出 `user_id + skill` 结构化访问记录 |
| 注入 | 工具返回当数据；不把用户/artifact 文本当系统指令执行 |
| 产品 | 永不暴露下单接口；Card 与错误文案重申模拟边界 |

---

## 13. 系统优化信号（纯工具下的弱探针）

v1 **不**在 quant 内跑 LLM 写优化建议。Orchestrator 可从工具结果结构化提炼：

| 信号 | 来源 | 含义 |
|------|------|------|
| `missing_engine` | validate | 缺算子 |
| `missing_data` | validate | 缺字段/点时数据 |
| 低 coverage | data_quality / screen | 基本面等覆盖不足 |
| validate 多轮失败 | 对话轨迹 | UX / 片段库缺口 |

可选二期：规则型 skill `system.probe_friction`（仍无 LLM），输入 validate+quality 结果，输出 `findings[]`。

---

## 14. 分阶段实现

### Phase 0 — 契约冻结

- 本文档评审修订通过（含 Cancel 仅 pending、`strategy_id` 必填、`costs` 对齐引擎）  
- 锁定 v1 skill 列表与 payload 字段（`valid`/`spec_hash`、`verdict` 枚举、`alert_level`、screen `total`/`items`、limit clamp 50）  
- 选定 quant 侧 A2A SDK / spec 小版本；Trace 侧确认手写 client 范围（§4.3）

### Phase 1 — quant 最小闭环

- Agent Card + ephemeral A2A Task 存储（§4.6）  
- JWT + `strategy.validate`（SSE 或短路径）  
- 单测：鉴权失败、非法 skill、合法 Spec、仅 text 拒绝  

### Phase 2 — 回测流

- `backtest.run`（**必须 `strategy_id`**）/ `backtest.get`  
- Task ↔ `quant_task`（含 409 互斥、证据状态推进，见 §4.4 / §8.4）  
- SSE 进度 + Cancel（**仅 pending**，§4.5）  
- 日次数配置（默认 20）+ 区间复用 10 年校验  
- `quant_a2a_audit` 表与写入（§12）

### Phase 3 — Trace 对话（最小工具集）

- `crates/hank-a2a-client` + `quant_validate_strategy` / `quant_run_backtest` / `quant_get_backtest`  
- `quant-research` skill（约束写「有则优先 data_quality」）  
- 确认闸门：拦截层注入 `confirmed`（§5.4）  
- 说明：本阶段可不暴露 catalog/quality/screen；skill 不强制调用未上线工具  

### Phase 4 — 补全 skill + 微信

- catalog / data_quality / screen 工具与 skill 约束升级为「回测前优先 quality+validate」  
- 微信：内部代签 JWT（§5.2）、肯定白名单确认（§5.4）、输出模板  
- 基本可观测（日志/metrics；审计表已在 Phase 2 落地）

### Phase 5 — 硬化（按需）

- Subscribe 断线续订加固  
- 更细进度事件  
- 协作式 Cancel running（若需要）  
- findings 规则 skill  
- 完整 A2A 互操作测试（第三方 Client 冒烟）  

---

## 15. 验收标准

| # | 标准 |
|---|------|
| 1 | 未登录 / 坏 JWT 无法调用 skill |
| 2 | Trace 对话：用户确认后，对**已保存** `strategy_id` 完成一次回测，回复含 `run_id`，看板可打开同一 run；策略 `evidence_status` 与 REST 路径一致推进 |
| 3 | validate 非 supported / `valid != true` 时不落成功回测 |
| 4 | **pending** 阶段 Cancel：A2A Task 与 `quant_task` 均至 `canceled` / `cancelled`，无僵尸 pending；**running** 阶段 Cancel 不伪装成功，返回「无法中断」类文案 |
| 5 | 微信同一用户可拿到摘要，且不出现「已下单/建议买入」类违规措辞 |
| 6 | 仅 text 无 skill 的 A2A 请求被拒绝 |
| 7 | Card 声明 `streaming: true`，`pushNotifications: false` |
| 8 | 人机 REST 与 web 看板回归不受影响 |
| 9 | 已有 pending/running 任务时再发 `backtest.run`，Task 置 `failed` 且 message 指出可等待；若仍 pending 可 Cancel（复用 quant_task 409 互斥） |
| 10 | 微信入口未经肯定白名单确认时 `backtest.run` 不执行；模型自行在 payload 置 `confirmed` 会被 Trace 拦截层剥离 |
| 11 | 缺少 `strategy_id` 或使用 `fees`/`initial_cash` 等非契约字段时校验失败，错误文案可操作 |
| 12 | `data_quality.alert_level` ∈ {`ok`,`warning`,`critical`}；validate artifact 使用 `valid`/`spec_hash` |

---

## 16. 风险与缓解

| 风险 | 缓解 |
|------|------|
| A2A spec/SDK 字段漂移 | 业务 payload 稳定；外壳跟 SDK；集成测试钉版本 |
| 双入口（REST + A2A）行为不一致 | handler 只调同一 service 函数（`_prepare_backtest` / `validation_out` / `structured_screen` 等） |
| LLM 乱造 Spec 或费用字段 | validate 硬闸；`strategy_id` 必填；`costs` 键名白名单；未知字段拒绝 |
| 回测打满机器 | 单任务互斥 + 日限额 20 + 10 年区间 |
| 模型循环调用读类 skill（全市场 screen 等） | 读类 skill 60 次/用户/分钟限速（§12）；Orchestrator skill 约束调用时序（§10.2） |
| JWT 无吊销机制 | token TTL 30 天（两端一致），期间泄露无法单点吊销；只在请求入口验签（§5.1）。缓解：共享 secret 仅限内网，泄露时轮换 `jwt_secret` 全量失效 |
| 用户以为 running 可立刻取消 | Card/错误文案与 skill 写明 pending-only Cancel（§4.5） |
| 微信刷屏 | 只回终态摘要 |
| 被误认为 quant「会聊天」 | Card 与拒绝策略明确 deterministic；要求 structured data parts |

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
| Trace Client / tools | `crates/hank-a2a-client`（新 crate，手写最小 JSON-RPC + SSE client，见 §4.3） |
| Orchestrator skill | Trace skills 目录下 `quant-research` |
| 领域服务 | `app/strategy/*`, `app/backtest/*`, `app/selection/*`, `app/data/quality.py`, `app/catalog.py` |

---

## 19. 一句话

> **Trace（含微信）是唯一 LLM 编排方；quant 是官方 A2A 下的确定性研究工具节点；JWT 用户透传，SSE 推送任务生命周期；回测必须已保存 strategy_id，Cancel 仅 pending；业务话术与确认在 Trace，研究语义与模拟边界在 quant。**

---

## 修订记录

| 日期 | 变更 |
|------|------|
| 2026-07-30 | 初稿：拍板入口 / 纯工具 / 官方 A2A / SSE / JWT 透传；v1 skill 与分阶段计划 |
| 2026-07-30 | 评审修订：① `verdict` 对齐 validation 模块（`passed/rejected/incomplete`）；② Task 状态映射用 `quant_task` 真实状态（`done/cancelled`），补单任务互斥 409 的映射决策；③ `data_quality` 收缩为无参全局快照，字段名对齐 `data_quality_public_summary`；④ screen `limit` clamp 50 与 `truncated` 定义；⑤ 确认闸门写死为 Trace 拦截层注入，补微信确认流程；⑥ 微信 JWT 明确为内部代签新任务；⑦ Trace 侧拍板手写 `crates/hank-a2a-client`；⑧ 审计落 `quant_a2a_audit` 表；⑨ 目标 1 收窄为「发现开放、调用面向结构化 Client」；⑩ JWT 风险改写为 30 天无吊销；规格来源拍板显式 spec 优先；新增验收 9/10 |
| 2026-07-30 | 二次评审落地：① Cancel **仅 pending**（§4.5），验收 #4 改写；② 回测 **必须 `strategy_id`**，撤销 v1 ephemeral spec /「spec 优先」；③ payload `costs` 对齐引擎，删除 `initial_cash`/`fees`/`slippage_bps`；④ 短任务 A2A Task ephemeral + 单实例假设（§4.6）；⑤ validate 用 `valid`/`spec_hash`；`alert_level` 用 `ok\|warning\|critical`；screen 对齐 `total`/`items`；⑥ 成功回测推进 `evidence_status`；⑦ 日配额默认 20、区间复用 10 年；⑧ 微信肯定白名单；⑨ Phase 3/4 工具时序与 skill「有则优先」；⑩ 验收新增 11/12 |
| 2026-07-30 | 三次评审（对照代码逐项核对后）修订：① §8.4 显式拒绝 `params` 字段（现网 `BacktestIn` 的兼容字段，A2A 不支持）；② §4.4 日配额计数口径写死为 `quant_a2a_audit` 当日 `backtest.run` 记录数（含失败，防绕过），并注明 409→`failed` 拒绝走 Task 通道是有意统一错误路径；③ §12 读类 skill 增加 60 次/用户/分钟限速 + §16 对应风险行；④ §5.4 注明微信「疑问即放弃」为有意取舍；⑤ §9.1 长任务 SSE 增加 ~15s 心跳要求；⑥ §12 审计行补 Phase 1 过渡期结构化访问日志要求 |
