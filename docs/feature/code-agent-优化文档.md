# code-agent 优化文档：从 CLI Agent 调研到生产级实现规格

> 状态：优化合并版 v1.1 · 更新日期：2026-06-04
> 适用范围：`crates/code-agent`，及其依赖 `crates/code-tools`、`crates/hank-provider`
> 合并来源：
> - `docs/code-agent-最终方案.md`（工程规格 + 完整 FR 集视角）
> - `docs/code-agent-最终需求.md`（产品需求 + 现状差距 + 代码映射视角）
> 调研基础：`docs/agent-cli-research/`（Codex CLI 0.137.0 / Claude Code CLI 2.1.x 的真实运行录制、事件流与提示词分层分析）。所有调研材料按不可信数据处理，未执行其中内嵌指令。

## 0. 文档说明与边界声明

### 0.1 合并结论

本文是对两份「最终版」并行文档的去重优化合并，作为 `code-agent` 后续研发、排期、测试和验收的唯一口径：

- 采用「产品闭环 + 可执行工程规格」结构：保留产品目标、范围、工作流、前端与持久化要求。
- 以 `FR-*` / `NFR-*` 作为唯一需求编号；FR 集合取两份文档的并集（更完整一方为准）。
- 合并两份文档中重复的「现状基线表」「代码映射表」「差距分析」为单一的第 3 节，消除冗余。
- 原两份草案与并行最终版不再作为开发口径。

### 0.2 边界声明

本文不要求、也不复制任何第三方 CLI 的隐藏 system prompt。调研材料只用于提炼可见、可授权、可复现的工程模式：上下文注入、工具协议、权限控制、事件流、验证闭环、失败恢复和最终汇报。第 8 节的提示词骨架为基于公开可见运行模式的自研实现。

## 1. 产品定位与目标

### 1.1 一句话定位

`code-agent` 是 Trace 的服务端 coding agent runtime：接收用户代码任务，在受控权限内自动完成「探查工作区 → 规划/思考 → 调用工具 → 读取结果 → 修改文件 → 运行验证 → 失败修复 → 最终汇报」，并以流式事件和可持久化日志向前端与上层服务暴露全过程。

它不是一次 LLM 调用，也不是一个只会生成文本的助手，而是一个带工具、权限、上下文预算、验证和保险丝的循环执行系统。

### 1.2 产品目标 / 成功标准

1. 用户提交明确 coding 任务后，Agent 能在一次会话内完成从探索到验证的闭环。
2. 提示词以 base/developer/environment/user 分层组装，可在日志中分层可见。
3. 所有模型输出、工具调用、文件变更、验证结果、token/耗时指标都能以稳定事件流呈现，每条带 `run_id`。
4. Agent 能识别沙箱、权限、测试缺失、命令失败、上下文超限和重复循环，并做出可解释的降级或修复动作。
5. 最终回答必须说明改了什么、验证了什么、还有什么风险。
6. 相同输入和相同工具结果下，事件日志足以复盘 Agent 的关键决策路径，但不保存内部 chain-of-thought。

### 1.3 目标用户

1. 使用 agent-web 的开发者：用自然语言完成代码修改、测试修复、项目骨架生成。
2. Agent-web 前端：需要稳定渲染 Agent 状态、工具输出、文件变更、权限请求和验证结果。
3. 后端调度层：需要控制模型、工具、权限、预算、取消、重试和会话恢复。
4. 研发/调试人员：需要通过事件日志复盘失败原因、性能瓶颈和 token 成本。

### 1.4 核心场景

1. 新建小型项目或模块，生成 README、配置、源码和测试。
2. 修复测试失败，自动运行测试并根据错误继续修改。
3. 在大仓库中查找相关文件，做小范围改动并避免影响无关用户改动。
4. 在权限受限环境中完成可做部分，并把需要用户授权的操作清楚暴露出来。
5. 长任务中自动压缩上下文，保留目标、关键决策、文件变更和待办事项。

## 2. 调研提炼出的工程原则

1. 分层提示词：稳定基础指令 + 运行时 developer 消息（权限+能力目录）+ 环境上下文（cwd/shell/date/sandbox）+ 项目记忆 + 用户任务，分段发送以利于缓存复用、日志审计和回放。
2. 最小工具面即可跑通：文件读、结构化编辑、搜索、目录、shell/test 已能支撑大多数 coding 任务（Codex 仅 shell + apply_patch 即可建项目）；编辑必须走结构化工具，便于审计、权限判断和 diff 生成。
3. 工具动作双段事件：每次工具调用必须产生 `tool.started` 和 `tool.completed`，含输入、输出摘要、耗时、错误标记和 tool_use_id；token usage 按步与按轮上报。
4. 运行边界清晰：一次完整任务用 `run_id` 框定，一轮 LLM 交互用 `turn_id` 框定；事件统一携带关联 ID。
5. 权限模型是一等能力：不是 `can_run_shell` 布尔开关，必须前置设计；非交互场景不能卡死等待审批，必须支持拒绝、降级、建议用户手动执行和最终汇报。
6. 显式验证闭环：写操作后优先运行项目自身验证手段，再用 read-only verifier 辅助复核；失败后读取错误并做针对性修复。
7. 失败恢复有界：对 command not found、测试依赖缺失、沙箱拒绝、DNS/registry 失败、不可写路径等分类处理，「尝试→观察失败→适配→验证」（如 python → python3 → compileall + 冒烟），避免重复同一失败动作。
8. 上下文有保险丝：工具输出截断、预算分级、三层压缩、最大迭代、LLM 超时、provider retry/fallback、循环检测必须同时存在。
9. 渐进式上下文：Skills、长文档、MCP 工具说明只在触发时加载；默认上下文只放 name、description、path 等索引信息。
10. 运行约定：优先 `rg`；并行只读工具；默认 ASCII；绝不回滚用户无关改动 / `git reset --hard`；非交互 git。
11. 可观测优先于隐藏提示词：复用的是 context 注入、工具协议、权限升级、事件日志、验证与失败恢复，而不是隐藏 system prompt 文本。

## 3. 现状基线、差距与代码映射

下表合并了两份文档中的「现状基线」「已识别差距」和「代码映射」，作为单一对照口径。

| 模块 | 文件 | 已有能力 | 主要差距 |
| --- | --- | --- | --- |
| 会话循环 | `src/session.rs` | `AgentMode::Simple` 扁平循环、streaming、工具执行、取消、metrics、run/turn 生命周期、权限门控、文件变更事件 | 缺完整持久化、artifact 索引和所有事件统一关联 ID |
| 编排器 | `src/agent/orchestrator.rs` | Think/Act 两阶段、run/turn 生命周期、直接工具权限门控、文件变更事件、读工具并发；非只读 Worker 委派暂时拒绝避免绕过权限 | 需要 plan 事件、Worker scoped permission 与写入跟踪、验证修订回流 |
| Worker | `src/agent/worker.rs` | 子任务 loop、工具过滤、独立上下文预算（100k/60k）、artifact summary；由 Orchestrator 主流程限制写 Worker | 需要 affected_paths 强约束、Worker 内部权限门控和文件变更回传 |
| Verifier | `src/agent/verifier.rs` | read-only verifier、JSON verdict；Simple 写操作后可调用并进入有限修订 loop；Orchestrator 写操作后可调用 verifier 并记录非通过风险 | Orchestrator 仍未接 verifier 修订回流 |
| 上下文 | `src/context/manager.rs`、`summary.rs` | 预算分级、microcompact、LLM summary、truncate 三层压缩；Simple 压缩后重置 usage | 需要 summary 质量验收、稳定前缀策略和 Orchestrator usage 校准 |
| 循环检测 | `src/agent/loop_detector.rs` | 滑动窗口指纹 + 重复率双策略 | 需要失败分类与终止事件更结构化 |
| 重试 | `src/retry.rs` | 指数退避、抖动、按错误文本判定可重试 | 需要与 provider fallback、事件和最终风险汇报闭合 |
| Prompt Pipe | `src/prompt_pipe.rs` | `PromptSegment`、`discover_project_context`（CLAUDE.md/AGENTS.md/.cursorrules） | 需要 runtime context、tool inventory、deferred skills 接入运行时 |
| 事件 | `src/types.rs` | `AgentEvent` 已包含并发出 run/turn lifecycle、file change、permission、verification、metrics、压缩、ask_user | 需要 plan events 覆盖、所有旧事件补 run_id/turn_id/timestamp、JSONL 持久日志 |
| 工具 | `crates/code-tools` | 统一 `Tool` trait、timeout、risk、streaming、常用工具集、直接编辑工具可触发文件变更事件 | 需要 diff artifact、shell/test 更结构化输出、完整输出 artifact |
| 权限 | `code-tools/src/permission.rs` | `PermissionGuard` 已接入 Simple 与 Orchestrator 直接工具路径；支持 sandbox path 校验、denial 事件和 run 汇总 | Worker 内部权限、交互式审批持久规则和恢复闭环不足 |

### 3.1 关键差距摘要（待补齐）

- **G1 分层提示词部分落地**：服务端 chat 路径已使用 layered prompt 和 context 摘要，Orchestrator phase 仍有局部硬编码提示。
- **G2 权限模型部分接线**：Simple 与 Orchestrator 直接工具已走 `PermissionGuard`；Worker 内部工具执行仍需 scoped permission。
- **G3 验证阶段部分启用**：Simple 写操作后可调用 `VerifierAgent` 并有限修订；Orchestrator 可调用 verifier，但仍缺修订回流。
- **G4 事件 schema 部分补齐**：已有 `run.started/run.completed`、`turn.started/turn.completed`、`file.changed`、permission 事件；旧事件还未统一携带 run/turn/timestamp。
- **G5 失败恢复策略仍偏观察型**：已有工具错误分类提示，缺主动替代路径、提权申请和恢复次数上限。
- **G6 Skills 渐进式披露部分落地**：工具 schema 支持 deferred stub，SKILL.md 扫描与引用文件按需加载仍未完成。
- **G7 持久化与恢复不完整**：仍缺会话恢复、artifact 索引、JSONL 事件重放和完整 terminal status 归档。

## 4. 范围

### 4.1 In Scope

1. Headless coding 任务执行。
2. Simple loop 与 Orchestrated loop 两种运行模式。
3. Prompt 分层组装与项目上下文发现。
4. 工具注册、权限检查、执行、流式输出和结果截断。
5. 文件读写、搜索、shell、测试、git 状态、用户提问等代码任务常用工具。
6. 上下文预算管理、microcompact、LLM 摘要压缩和兜底截断。
7. 循环检测、最大迭代数、超时、max tokens continuation、provider retry/fallback。
8. 验证阶段：测试运行、demo/冒烟验证、read-only verifier。
9. 事件流 schema、JSONL 持久日志、会话恢复、artifact 索引。
10. 权限模型：`read-only`、`workspace-write`、`escalated`、`unrestricted` 以及审批/预授权。
11. 前端集成所需事件：phase、工具状态、流式命令输出、文件变更、权限请求、测试结果和最终 summary。

### 4.2 Out of Scope

1. 还原或复制第三方 CLI 的隐藏 system prompt。
2. 让模型绕过沙箱、系统权限或用户审批。
3. 默认接管任意全盘文件系统。
4. 自动执行未授权破坏性命令，例如删除、reset、checkout、force push。
5. 把内部推理链明文写入持久日志。
6. 在 Agent loop 内写入 UI 展示逻辑；前端只消费事件与 artifact。

## 5. 标准运行工作流

1. `run.started`：创建 `run_id`，记录 cwd、模型、工具、权限、预算和 session 配置。
2. `turn.started`：创建 `turn_id`，记录用户请求、phase、message_count、可用工具摘要。
3. `context.assembled`：按 base/developer/environment/project/user 分层组装上下文，必要时输出 debug 摘要。
4. `workspace.inspected`：优先读取目录结构、关键配置、git 状态和相关文件。
5. `plan.updated` 或 progress message：输出用户可见的短状态。
6. `llm.requested`：调用模型，记录 phase、provider、model、max_tokens、tools。
7. `tool.started` / `tool.completed`：执行合法工具并回填 tool result；只读工具可并发，写工具串行。
8. `file.changed`：编辑类工具产生结构化文件变更事件和 diff artifact。
9. `verification.started` / `verification.completed`：运行项目测试、编译检查、demo/smoke test 或 read-only verifier。
10. `revision.loop`：验证失败时读取失败原因，窄范围修复并限制修订轮数。
11. `run.completed` / `run.failed` / `run.cancelled`：汇报变更、验证、残余风险、usage、permission_denials 和 artifacts。

### 5.1 Simple Mode

对应当前 `AgentSession::run_simple`（`src/session.rs`），用于普通问答、小范围修改和无需多 Agent 协作的任务。必须满足：

1. 支持 `TextDelta`、`ToolStart`、`ToolResult`、`ToolOutputDelta`、`TurnComplete`。
2. 支持工具循环直到 `EndTurn`、取消、预算溢出、超时或最大迭代数。
3. 支持 `ask_user` 中断，让前端接管用户确认。
4. 支持 `MaxTokens` continuation，连续 3 次无有效进展后停止。
5. 每轮工具结果进入上下文前必须截断，完整输出作为 artifact 保存。
6. 写操作后应进入验证阶段，不能仅靠模型自述完成。

### 5.2 Orchestrated Mode

对应 `OrchestratorAgent`（`src/agent/orchestrator.rs`），用于复杂任务、需要 Worker 分工或显式 Think/Act/Observe 的任务。必须满足：

1. Think phase 不暴露写工具，输出用户可见的简短 `Thinking` 或 `PlanUpdated` 事件（可配置隐藏）。
2. Act phase 暴露工具并执行 tool result 回填。
3. `delegate_task` 只能把明确子任务交给 Worker，必须携带 description、context、tools_allowed、affected_paths。
4. Worker 默认只拿到必要工具和必要上下文，避免继承完整主会话。
5. 写操作 Worker 默认串行，读操作允许并发。
6. Worker 结果必须压缩为 summary、affected_files、artifacts，再返回 Orchestrator。
7. Orchestrator 在写操作后调用验证阶段，并把 `needs_revision` / `rejected` 回注到修订循环。

## 6. 功能需求

需求编号规则：`FR-<域>-<序号>`。优先级：P0 必须、P1 应当、P2 可选。状态：已实现、部分、新增。每节标注对应代码。

### 6.1 分层上下文与 Prompt Pipe（FR-CTX）

对应代码：`src/prompt_pipe.rs`、`src/context/manager.rs`、`src/context/summary.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-CTX-1 | P0 | 部分 | 系统提示词必须由 `PromptSegment` 分层组装：`base`（人格+工程规范）→ `developer/runtime`（权限+工具目录+技能目录）→ `environment`（cwd/shell/date/timezone/sandbox/可写根）→ `project context` → `user task`，不在业务代码里拼接大块硬编码文本。 |
| FR-CTX-2 | P0 | 部分 | 必须注入结构化环境上下文（参考 Codex `<environment_context>`）：cwd、shell、当前日期、时区、repo root、workspace roots、sandbox mode、permission mode、network policy。运行时从 session/config 生成，不硬编码。 |
| FR-CTX-3 | P1 | 已实现 | 必须扫描并注入项目记忆文件 `CLAUDE.md`、`AGENTS.md`、`.cursorrules`，默认单文件截断 4000 字符并标注，缺失时不报错；`discover_project_context` 已满足，需接入运行时上下文。 |
| FR-CTX-4 | P0 | 已实现 | 必须按字符类型估算 token：CJK 约 1.5 chars/token，ASCII 约 4 bytes/token，图片占位 1000；优先使用 provider 报告的真实 usage。 |
| FR-CTX-5 | P0 | 已实现 | 必须支持预算分级：Normal(<80%)、Warning80、Critical95、Overflow100，并触发对应事件与动作。 |
| FR-CTX-6 | P0 | 已实现 | 上下文超阈值时必须执行三层压缩：microcompact → LLM summary → truncate（fallback 永不失败）。摘要保留原始目标、关键决策、改动文件、验证结果、当前进度、待办。 |
| FR-CTX-7 | P1 | 新增 | base/developer/environment 应保持稳定前缀以利于 prompt cache 复用；压缩只作用于对话历史中段，不压缩 base、project rules 摘要和最新用户消息。 |
| FR-CTX-8 | P2 | 部分 | 支持 deferred loading：Skills、MCP 工具说明、长文档只在触发时加载；默认上下文只放 name、description、path。 |
| FR-CTX-9 | P1 | 部分 | Prompt 组装结果可在 debug 模式记录摘要和 segment 指纹，默认不持久化完整 system prompt 或敏感内容。 |

验收：

1. 给定相同 segments，prompt 组装输出稳定。
2. 缺失项目上下文文件不报错。
3. 超长项目规则不会撑爆上下文。
4. 压缩后 Agent 仍能说明当前任务目标、已修改文件和待办。

### 6.2 Agent Loop 与编排（FR-LOOP）

对应代码：`src/session.rs`、`src/agent/orchestrator.rs`、`src/agent/worker.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-LOOP-1 | P0 | 已实现 | 支持扁平工具循环：stream → 解析 tool_use → 执行 → 回填 tool_result → 续循环，直到 end_turn、取消、预算溢出、超时或迭代上限。 |
| FR-LOOP-2 | P1 | 已实现 | 支持 Orchestrated loop：Think/Act 两阶段和 `delegate_task` 派发 Worker，Worker 拥有较小独立上下文预算并回传压缩摘要。 |
| FR-LOOP-3 | P0 | 已实现 | 必须限制最大迭代次数：Simple/Worker 默认 25，Orchestrator 默认 50；达上限发 terminal event 并安全退出，汇报未完成风险。 |
| FR-LOOP-4 | P0 | 已实现 | 必须处理 `StopReason::MaxTokens`：注入 continuation prompt；连续 3 次无工具调用或无进展则停止。 |
| FR-LOOP-5 | P0 | 已实现 | 必须支持取消：通过 `CancellationToken` 在流式与工具执行边界响应中断，并发 terminal event，取消后不再启动新工具。 |
| FR-LOOP-6 | P0 | 已实现 | 只读工具可并发执行；任意写工具、shell/git/test、`delegate_task` 默认串行，避免冲突。 |
| FR-LOOP-7 | P0 | 部分 | 必须以 `run_id` 框定一次完整运行，以 `turn_id` 框定一轮 LLM 交互；所有事件必须携带关联 ID。 |
| FR-LOOP-8 | P1 | 部分 | 验证必须成为显式阶段：写操作后触发项目验证或 `VerifierAgent` 复核，不能仅靠模型自述完成。 |
| FR-LOOP-9 | P1 | 已实现 | 每次 LLM 请求必须记录 phase：`simple`、`think`、`act`、`worker`、`verify`。 |
| FR-LOOP-10 | P1 | 新增 | Agent 默认先观察工作区再写操作；探索动作包括目录、关键配置、git 状态和相关文件，具体工具由模型根据上下文选择。 |

验收：

1. 任意 tool call 都有唯一 id，并能和 result 对齐。
2. 取消后不再启动新 LLM 请求或新工具。
3. 达到最大迭代数时停止并汇报未完成风险。
4. 写操作后的最终 summary 必须包含验证情况或无法验证原因。

### 6.3 工具系统（FR-TOOL）

对应代码：`crates/code-tools`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-TOOL-1 | P0 | 已实现 | 所有工具必须实现统一 `Tool` trait：name、description、input_schema、execute，并暴露 timeout、is_write、risk_level、supports_streaming。 |
| FR-TOOL-2 | P0 | 已实现 | 必须提供最小可用工具面：read_file、write_file、str_replace、search、list_directory、shell、test_runner、git、ask_user。 |
| FR-TOOL-3 | P0 | 已实现 | 工具结果进入上下文前必须截断，默认 40000 字符，保留 head 60% + tail 40% + 原始长度和截断提示；完整输出可作为 artifact 持久化。 |
| FR-TOOL-4 | P0 | 已实现 | 每个工具必须有超时，默认 30s，shell/test 等长任务默认 300s；超时返回 is_error tool result，不挂起，并继续 loop。 |
| FR-TOOL-5 | P1 | 已实现 | 支持流式工具输出：shell/test 以行或 chunk 为单位通过 `ToolOutputDelta` 实时回传 stdout/stderr。 |
| FR-TOOL-6 | P0 | 部分 | 编辑类工具必须记录结构化文件变更：path、kind(add/update/delete)、patch/diff 或 before/after 摘要，用于 `file.changed` 事件与 artifact（对齐 Codex apply_patch / Claude Write 可审计性）。 |
| FR-TOOL-7 | P2 | 部分 | 支持工具和 Skills 渐进式披露：上下文只放 name、description、path，命中时再加载 `SKILL.md` 及引用文件。 |
| FR-TOOL-8 | P1 | 已实现 | `ask_user` 工具必须中断循环并发 `AskUser` 事件，等待前端回传后续输入后作为新 turn 继续。 |
| FR-TOOL-9 | P0 | 已实现 | 未知工具必须返回结构化错误 tool result，不能 panic 或终止进程。 |
| FR-TOOL-10 | P1 | 部分 | shell/test 工具必须记录 command、cwd、exit_code、stdout/stderr 摘要、duration、is_error、是否使用 escalation。 |

验收：

1. 多个 read-only 工具可并行执行且事件结果不串 id。
2. 写工具按模型请求顺序执行。
3. 工具执行超时会返回 error result 并继续 loop。
4. 完整工具输出可作为 artifact 持久化，模型上下文只接收截断版本。

### 6.4 权限与沙箱（FR-PERM）

对应代码：`code-tools/src/permission.rs`。调研强调：权限模型必须前置设计，是一等公民，不是「能否跑 shell」的布尔开关。

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-PERM-1 | P0 | 部分 | 必须支持权限模式：`read-only`（仅探查）、`workspace-write`（可写根内编辑）、`escalated`（单次命令/工具经批准后执行）、`unrestricted`（仅受信自动化环境显式启用）。对齐 Codex sandbox 三档与 Claude permission-mode。 |
| FR-PERM-2 | P0 | 部分 | 所有 write/shell/network/destructive 工具执行前必须经过 `PermissionGuard::check`，返回 Allow、Deny(reason)、NeedApproval(reason)。接线点包括 `execute_single_tool`、Simple、Worker、Verifier 的工具执行入口。 |
| FR-PERM-3 | P0 | 已实现 | 必须维护危险命令黑名单，例如 `rm -rf /`、`mkfs`、fork bomb、`chmod -R 777 /`；shell 命中黑名单直接 Deny。 |
| FR-PERM-4 | P0 | 已实现 | 写路径必须做 sandbox 校验：解析为绝对路径后必须落在 `sandbox_paths` / writable roots 前缀内，否则 Deny。需将 work_dir/sandbox 配置从会话传入 Guard。 |
| FR-PERM-5 | P1 | 部分 | `NeedApproval` 在交互场景转为权限请求或 `AskUser`；非交互场景必须优雅降级，完成可做部分并在最终总结说明被拒动作和建议命令（对齐 Claude acceptEdits 下 Bash 被拒后让用户手动跑测试）。 |
| FR-PERM-6 | P1 | 已实现 | 被拒动作必须记录到 `permission_denials` 列表，并随 `PermissionDenied` 与 `run.completed` 上报。 |
| FR-PERM-7 | P0 | 已实现 | 工具必须声明风险等级：safe/read（只读，自动放行）、write（写，默认允许）、network/shell/destructive（需审批），或映射到 Safe/Moderate/Dangerous。 |
| FR-PERM-8 | P1 | 部分 | 命令审批支持 prefix rule，例如允许 `npm test`、`cargo test`、`pnpm install`；规则必须可审计、可持久化、可撤销。 |
| FR-PERM-9 | P1 | 部分 | 沙箱、DNS、registry、写路径失败应识别为可升级失败，并生成 scoped approval request（见 FR-ROBUST-4），而不是普通命令失败。 |

验收：

1. 未授权写 workspace 外路径必须失败。
2. 未授权 shell 命令必须产生可见 permission event。
3. 审批失败后 Agent 能完成无需该权限的剩余工作，并在最终汇报说明。
4. 危险命令默认拒绝，即使模型反复请求也不能执行。

### 6.5 事件流与持久日志（FR-EVT）

对应代码：`src/types.rs`。调研建议：事件 schema 紧凑，每个工具动作双段（started/completed），内部 reasoning 不进持久日志。

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-EVT-1 | P0 | 已实现 | 每个工具动作必须发出 `ToolStart`/`tool.started`（含 id/name/input）和 `ToolResult`/`tool.completed`（含 id/output 摘要/is_error）。 |
| FR-EVT-2 | P0 | 已实现 | 必须发出运行边界事件：`run.started`（run_id/cwd/model/tools/permission_mode）、`run.completed`（status/usage/summary/permission_denials）、`run.failed`、`run.cancelled`。对齐 Codex `turn.completed` / Claude `result`。 |
| FR-EVT-3 | P0 | 已实现 | 必须发出 turn 边界事件：`turn.started`、`turn.completed`，便于回放一轮 LLM/tool 交互。 |
| FR-EVT-4 | P1 | 部分 | 必须发出结构化文件变更事件 `file.changed`：`changes:[{path, kind:add|update|delete}]`，由编辑类工具驱动。 |
| FR-EVT-5 | P0 | 已实现 | 必须上报 `Metrics`（provider/model/phase/input_tokens/output_tokens/latency_ms）与 `ToolMetrics`（tool_name/duration_ms/is_error）。 |
| FR-EVT-6 | P0 | 已实现 | 必须发出预算与压缩事件：`TokenWarning`、`CompressionTriggered`（含 before/after tokens 与策略）；循环检测发 `LoopDetected`；provider 降级发 `ProviderFallback`（from/to/reason）。 |
| FR-EVT-7 | P0 | 新增 | 每个事件至少包含 run_id、turn_id、timestamp；工具事件包含 tool_use_id；必要时包含 parent_tool_use_id。 |
| FR-EVT-8 | P1 | 新增 | 内部 reasoning/thinking 不得进入持久日志。持久层仅保留用户可见摘要、工具调用、工具结果、事件和 artifact。`Thinking` 仅用于实时流，不落库；`LlmRequest` 仅 debug 模式输出 system 摘要。 |
| FR-EVT-9 | P1 | 部分 | 补齐 `PlanUpdated`、`PermissionRequested`、`PermissionDenied`、`VerificationStarted`、`VerificationCompleted`、`ContextAssembled` 事件。 |
| FR-EVT-10 | P1 | 新增 | 持久日志采用 JSONL，一行一个事件，可重放出关键 UI 状态；debug 模式可记录 prompt segment 摘要，但默认不保存完整 system prompt。 |

推荐紧凑 JSONL schema：

```json
{"type":"run.started","run_id":"...","timestamp":"...","cwd":"...","model":"...","permission_mode":"workspace-write","tools":["read_file","str_replace","shell"]}
{"type":"turn.started","run_id":"...","turn_id":"...","timestamp":"...","phase":"simple","message_count":4}
{"type":"message","run_id":"...","turn_id":"...","role":"assistant","channel":"progress","text":"..."}
{"type":"tool.started","run_id":"...","turn_id":"...","tool_use_id":"...","tool":"shell","input":{"command":"npm test","cwd":"..."}}
{"type":"tool.completed","run_id":"...","turn_id":"...","tool_use_id":"...","output":{"exit_code":0,"is_error":false,"elapsed_ms":1234}}
{"type":"file.changed","run_id":"...","turn_id":"...","changes":[{"path":"src/lib.rs","kind":"update"}]}
{"type":"run.completed","run_id":"...","timestamp":"...","status":"success","usage":{"input_tokens":0,"output_tokens":0},"permission_denials":[],"summary":"..."}
```

`AgentEvent`（`src/types.rs`）到上述 schema 的映射需补齐 `run.started`/`run.completed`/`file.changed`/`permission.*`/`plan.updated`，并为现有事件附加 `run_id`/`turn_id`/`timestamp`。

验收：

1. 任意一次运行可以用 JSONL 重放关键 UI 状态。
2. 事件中不包含明文 chain-of-thought、API key、auth token 或隐藏系统提示词。
3. 工具 started/completed 数量可按 tool_use_id 对齐。

### 6.6 上下文预算与压缩（FR-BUDGET）

对应代码：`src/context/manager.rs`、`src/context/summary.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-BUDGET-1 | P0 | 已实现 | 上下文预算优先使用 provider usage，缺失时使用本地估算。 |
| FR-BUDGET-2 | P0 | 已实现 | 80% 触发 warning，可尝试压缩；95% 触发 critical，强制压缩；100% 触发 overflow，停止 loop 或要求用户缩小任务。 |
| FR-BUDGET-3 | P0 | 已实现 | 压缩管线分三层：microcompact 压缩旧 tool result；LLM summary 总结中间消息；truncate 兜底移除最老内容。 |
| FR-BUDGET-4 | P0 | 已实现 | 压缩失败不能导致 run 崩溃，必须有兜底策略。 |
| FR-BUDGET-5 | P1 | 部分 | 压缩后必须发送 `CompressionTriggered` 事件，记录 before_tokens、after_tokens、策略、压缩摘要 id。 |
| FR-BUDGET-6 | P1 | 部分 | 压缩后应重置或校准 usage，避免旧 usage 导致连续误判。 |
| FR-BUDGET-7 | P1 | 新增 | 完整工具输出、测试报告和 diff 应作为 artifact 保存，压缩只影响模型上下文，不影响审计记录。 |

验收：

1. 大型 tool output 不会直接撑爆下一次 LLM 请求。
2. 压缩失败时 run 仍可安全停止或继续。
3. 压缩后 Agent 仍能说明原始目标、最近工具结果、已修改文件和待办。

### 6.7 验证闭环（FR-VERIFY）

对应代码：`src/agent/verifier.rs`、`code-tools/src/test_runner.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-VERIFY-1 | P1 | 部分 | `VerifierAgent` 必须可被 Simple/Orchestrator 在写操作后调用，仅持有 read-only 工具，输出 `{verdict, issues}`：approved、needs_revision、rejected。当前 Simple 和 Orchestrator 已有调用方，但 Orchestrator 还缺修订回流。 |
| FR-VERIFY-2 | P1 | 部分 | 验证应产出 `VerificationStarted` / `VerificationCompleted` 事件；needs_revision/rejected 时将 issues 回注到模型修订，并限制修订轮数防循环。 |
| FR-VERIFY-3 | P1 | 新增 | 优先使用项目自身验证手段：`cargo test`、`npm test`、`pnpm test`、`python -m pytest`、`go test ./...`；框架缺失时降级为编译检查、lint、CLI demo 或 smoke test。测试输出作为 artifact 保存。 |
| FR-VERIFY-4 | P1 | 新增 | 验证失败必须读取失败输出并做针对性修复，而不是盲目重跑同一命令。 |
| FR-VERIFY-5 | P1 | 新增 | 测试输出和验证报告应作为 artifact 保存，最终 summary 引用验证命令和结果。 |
| FR-VERIFY-6 | P2 | 新增 | 对会修改样本数据的 demo/验证（对齐 Claude 录制中跑 demo 后恢复 requirements.json），完成后应恢复样本数据；无法恢复时必须在 summary 中说明。 |
| FR-VERIFY-7 | P1 | 部分 | Verifier JSON 解析失败必须有安全默认（当前默认 Approved 以防无限修订循环），并在 issues 中标注解析失败自动放行或降级原因。 |
| FR-VERIFY-8 | P1 | 新增 | 验证不可运行时（权限不足/框架缺失），最终回答必须说明原因和建议用户执行的命令。 |

验收：

1. 成功运行的验证命令出现在最终 summary。
2. 失败验证至少触发一次针对性修复尝试，除非权限不足、用户取消或风险过高。
3. Verifier parse 失败不能导致无限循环，应降级并记录风险。

### 6.8 失败恢复与保险丝（FR-ROBUST）

对应代码：`src/retry.rs`、`src/agent/loop_detector.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-ROBUST-1 | P0 | 已实现 | Provider 瞬态错误自动重试：429、5xx、网络、overloaded；指数退避 + 50% 抖动，上限 3 次，单次最长 30s；不可重试 4xx/认证错误立即失败。 |
| FR-ROBUST-2 | P0 | 已实现 | 必须检测执行死循环：滑动窗口（size=6）指纹 + 单指纹重复≥2 或唯一率<30% 双策略；连续命中阈值（3）则终止，否则注入 nudge 提示变更策略。 |
| FR-ROBUST-3 | P0 | 已实现 | LLM stream 必须有超时，默认 120s；超时按当前已累积内容收尾并结束当前 phase，不无限等待。 |
| FR-ROBUST-4 | P1 | 部分 | 必须实现「尝试→观察失败→适配→验证」恢复：工具/命令因缺失、沙箱、DNS、不可写路径失败时，记录失败并尝试有界替代路径或申请提权（策略允许时），而非反复重试同一命令。 |
| FR-ROBUST-5 | P1 | 部分 | 对 `command not found`、测试依赖缺失、权限拒绝、网络失败分别给出不同恢复策略，不能反复重试同一命令。 |
| FR-ROBUST-6 | P0 | 已实现 | 任何阶段预算 Overflow100 必须强制安全终止，发 terminal event，不得继续调用 LLM。 |
| FR-ROBUST-7 | P1 | 部分 | 工具失败进入 observe 阶段，由模型根据结构化错误选择修复、降级、申请权限或询问用户。 |
| FR-ROBUST-8 | P1 | 新增 | 每类失败都要进入事件日志，最终 summary 必须保留未解决风险。 |

验收：

1. 同一工具同一输入重复调用不会无限执行。
2. Provider fallback 会发送 `ProviderFallback`。
3. 权限失败不会被包装成普通命令失败而丢失语义。
4. 工具或命令缺失时，Agent 尝试替代路径的次数有明确上限。

### 6.9 会话持久化与恢复（FR-SESSION）

对应代码：`src/session.rs`

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-SESSION-1 | P0 | 新增 | Session 必须保存 messages、events、artifacts、model、provider、permission_mode、cwd、created_at、updated_at、status。 |
| FR-SESSION-2 | P1 | 新增 | 支持从历史 messages 和压缩摘要恢复继续执行；进程重启后可加载 session 并继续未完成任务。 |
| FR-SESSION-3 | P1 | 新增 | 支持导出用户可见 transcript，不包含隐藏 prompt、内部 reasoning、secret。 |
| FR-SESSION-4 | P1 | 新增 | 支持 artifact 索引：文件 diff、完整工具输出、测试报告、最终 summary。 |
| FR-SESSION-5 | P1 | 部分 | 支持取消后保留 partial state，可查看已完成工具调用、文件变更和失败原因。 |
| FR-SESSION-6 | P1 | 新增 | session 恢复时必须重新校验 cwd、权限模式、writable roots 和可用工具，不能盲目信任旧环境。 |

验收：

1. 进程重启后可以加载 session 并继续一个未完成任务。
2. 已完成任务可以查看最终 summary、变更列表和验证输出。
3. 持久化数据不包含 API key、auth token、隐藏系统提示词或 chain-of-thought。

### 6.10 前端集成体验（FR-UI）

| 编号 | 优先级 | 状态 | 需求 |
| --- | --- | --- | --- |
| FR-UI-1 | P1 | 新增 | 前端应能基于事件流展示：当前 phase、正在执行的工具、流式命令输出、文件变更列表、权限请求、测试结果、最终 summary。 |
| FR-UI-2 | P1 | 新增 | 工具输出支持折叠与展开，默认显示摘要；完整输出从 artifact 读取。 |
| FR-UI-3 | P1 | 新增 | 权限请求必须显示命令/工具、cwd、风险、原因、可选持久规则和允许/拒绝动作。 |
| FR-UI-4 | P1 | 部分 | `AskUser` 事件应暂停 Agent loop，直到用户回复；恢复后作为新 turn 继续。 |
| FR-UI-5 | P0 | 部分 | 取消按钮触发 cancellation token，并保证后端停止新工具和新 LLM 请求。 |
| FR-UI-6 | P1 | 新增 | 最终页面必须能看到「改动文件 + 验证命令 + 风险/未完成事项」。 |

验收：

1. 长命令运行时前端能持续收到 `ToolOutputDelta`。
2. 用户拒绝权限后 Agent 能继续或结束，并给出可理解说明。
3. 最终页面可从事件和 artifacts 还原关键执行过程。

## 7. 非功能需求（NFR）

| 编号 | 分类 | 需求 |
| --- | --- | --- |
| NFR-1 | 性能 | 只读工具并行执行；流式首字延迟由 provider 决定，Agent 层不得引入额外阻塞；压缩与 artifact 写入不得长期阻塞事件流。 |
| NFR-2 | 可移植 | 仅依赖 `hank-provider` 抽象，不绑定具体 LLM 协议；支持 Anthropic / OpenAI 兼容 provider。 |
| NFR-3 | 安全 | 默认最小权限；shell 黑名单 + sandbox path 校验；不执行未授权破坏性命令；不持久化 secret、auth 文件内容、隐藏 prompt、内部推理链；破坏性命令必须显式识别并二次确认。 |
| NFR-4 | 可靠性 | 每个外部调用有 timeout；每个 loop 有最大迭代数；provider 错误有重试边界；工具错误必须返回给模型和事件流。 |
| NFR-5 | 可观测 | 每次 run 可追踪完整事件；每次 LLM 请求可统计 token、耗时、provider、model、phase；每次工具调用可统计耗时、错误率、输出大小；每次压缩可统计节省 token。 |
| NFR-6 | 资源边界 | 工具输出、单消息、总上下文均有上限；Worker 上下文预算小于 Orchestrator；artifact 有大小和保留策略。 |
| NFR-7 | 可测试 | 循环检测、重试判定、预算分级、token 估算、prompt 组装、权限拒绝、工具超时、verifier parse 必须有测试覆盖。 |
| NFR-8 | 可维护性 | 优先复用 `AgentSession`、`OrchestratorAgent`、`ContextManager`、`Tool` trait；新能力通过小接口扩展，不把 UI 逻辑写入 Agent loop。 |
| NFR-9 | Git 安全 | 绝不执行 `git reset --hard`、`git checkout --` 等回滚用户改动的操作，除非用户显式要求并通过权限流程；git 操作采用非交互命令。 |
| NFR-10 | 语言约定 | 代码注释与 commit message 遵循项目约定（中文）；面向模型的提示词可用英文以贴合训练分布；用户可见总结按用户语言输出。 |

## 8. 运行时 Prompt 骨架（参考实现，非复制隐藏提示词）

运行时按以下结构组装，使用 `PromptSegment` 管理各段。该骨架是基于公开可见运行模式的自研参考，不复制隐藏提示词。

```text
[base]
You are a coding agent working in the user's repository.
Operate pragmatically:
- inspect before editing
- prefer existing project patterns
- keep changes scoped
- use structured file edits (str_replace/write_file), not shell redirection
- run relevant tests when permitted
- recover from failures by reading errors and making targeted fixes
- never revert unrelated user changes; no git reset --hard unless asked
- summarize changed files, verification, and remaining risk

[developer/runtime]
- permission mode: <read-only|workspace-write|escalated|unrestricted>
- approval policy: <auto|ask|never>
- writable roots: <paths>
- network policy: <enabled|restricted|disabled>
- available tools: <name + description + schema summary + risk>
- skills index: <name + description + path>

[environment]
<environment_context>
  <cwd>...</cwd>
  <shell>...</shell>
  <current_date>...</current_date>
  <timezone>...</timezone>
  <repo_root>...</repo_root>
  <sandbox_mode>...</sandbox_mode>
</environment_context>

[project context]
- CLAUDE.md / AGENTS.md / .cursorrules contents, truncated with marker

[user]
<用户任务原文>
```

约束：

1. base 段保持稳定，便于 prompt cache 复用。
2. runtime/environment 从 session/config 动态生成，不硬编码。
3. user task 与 runtime context 分离，便于日志、回放和安全审计；隐藏实现指令不得与用户可见任务混写。
4. debug 模式最多记录 segment 名称、hash、长度和摘要，不默认保存完整 system prompt。

## 9. 事件 Schema 映射

持久化与流式回放采用紧凑 JSONL 事件，内部 reasoning 不落库（schema 示例见 6.5）。

`AgentEvent`（`src/types.rs`）需补齐以下映射：

- 新增 `run.started`/`run.completed`/`run.failed`/`run.cancelled`、`turn.started`/`turn.completed`。
- 新增 `file.changed`、`permission.requested`/`permission.denied`、`plan.updated`、`verification.started`/`verification.completed`、`context.assembled`。
- 为现有事件附加 `run_id`/`turn_id`/`timestamp`，工具事件附加 `tool_use_id`。

## 10. 实施优先级路线

### 10.1 MVP / P0

目标：打通生产可用的 Simple Mode coding 闭环。

1. 权限接线：所有 write/shell/network/destructive 工具执行前接入 `PermissionGuard::check`（`execute_single_tool`/Worker/Verifier），补 sandbox path 校验。
2. Prompt 接线：PromptPipe 支持 runtime context、tool inventory、project context，环境上下文块注入，并接入 AgentSession。
3. 事件边界：补齐 `run.started`/`turn.started`/`turn.completed`/`run.completed`/`run.failed`/`run.cancelled` + `file.changed`，所有事件携带 run_id/turn_id，JSONL 持久化。
4. 文件变更：编辑工具产生 `file.changed` 和 diff artifact，shell/test 输出结构化 metadata。
5. 验证闭环：Simple Mode 完成「探索 → 修改 → 验证 → 修复 → 汇报」，写操作后运行项目验证或说明无法验证原因。
6. 上下文压缩事件与 artifact 持久化；最终回答标准化（改动/验证/风险/权限拒绝）。

覆盖需求：FR-CTX-1/2/3，FR-LOOP-7/9/10，FR-TOOL-6/9/10，FR-PERM-2/4/5/6，FR-EVT-2/3/4/7/10，FR-BUDGET-5/7，FR-VERIFY-3/4/8，FR-SESSION-1/4。

### 10.2 P1

目标：提升复杂任务、可恢复性和前端体验。

1. 补齐 Orchestrator 的 needs_revision/rejected 回流 + 客观测试验证。
2. 失败自适应与提权降级路径；被拒降级总结与 `permission_denials`；失败分类（command not found、依赖缺失、沙箱、DNS、registry、路径越界）。
3. Orchestrated Mode 接入真实复杂任务入口；Worker affected_paths 冲突检测和权限隔离。
4. Provider fallback 策略配置化并补齐事件；Capability handshake / context.assembled / plan.updated 等可观测事件。
5. 会话恢复：从历史 messages、压缩摘要和 artifacts 恢复；文件变更 diff artifact 与前端预览。
6. 前端权限请求、工具输出折叠、验证结果和文件变更预览。

覆盖需求：FR-LOOP-8，FR-PERM-8/9，FR-EVT-8/9，FR-BUDGET-6，FR-VERIFY-1/2/5/7，FR-ROBUST-4/5/7/8，FR-SESSION-2/3/5/6，FR-UI 全部。

### 10.3 P2

目标：扩展上下文效率、并发能力和高级控制。

1. Deferred skills / tool search / MCP 工具说明按需加载。
2. mutating demo 样本恢复策略自动化；checkpoint 与用户批准后的回滚能力。
3. 多 Worker 并发调度。
4. 成本预算、时间预算、自动降级模型。
5. 跨 session 记忆和项目级长期知识库。

覆盖需求：FR-CTX-8，FR-TOOL-7，FR-VERIFY-6。

## 11. 验收用例

### 用例 1：零依赖 CLI 项目生成

输入：创建一个小型本地 requirements tracker，包含 package.json、README、JSON 样例数据、CLI、node:test 测试，并运行测试。

期望：

1. Agent 先检查目录和 git 状态。
2. 创建所需文件（结构化编辑），并产生 `file.changed`。
3. 运行 `npm test`。
4. 运行 demo 或 CLI smoke test。
5. 如果 demo 修改样例数据，应恢复或说明。
6. 最终 summary 说明测试通过、文件列表和风险。

### 用例 2：权限受限的测试命令

输入：修改代码并运行测试，但当前 permission mode 不允许 shell。

期望：

1. Agent 完成允许范围内的读写文件改动。
2. 测试命令触发 `PermissionRequested` 或 `PermissionDenied`。
3. 非交互模式不阻塞。
4. 最终 summary 说明未运行测试的原因和建议命令，被拒动作出现在 `permission_denials`。

### 用例 3：测试失败后修复

输入：修复一个已有 failing test。

期望：

1. Agent 运行测试并捕获失败输出。
2. 根据失败定位相关文件。
3. 做窄范围修改。
4. 重跑相关测试。
5. 最终 summary 包含失败原因和修复结果。

### 用例 4：上下文接近上限

输入：长会话中继续执行新任务。

期望：

1. 80% 预算触发 warning。
2. 95% 预算触发压缩并发 `CompressionTriggered`（记录 before/after tokens 和策略）。
3. 压缩后仍保留原始目标、最近工具结果和待办。
4. Overflow100 安全终止。

### 用例 5：重复工具循环

输入：模型反复调用同一搜索工具同一参数。

期望：

1. LoopDetector 触发 `LoopDetected`。
2. Agent 收到 nudge 后尝试换策略。
3. 连续达到阈值后停止并汇报风险。

### 用例 6：会话恢复

输入：一个执行到中途被取消或进程重启的任务。

期望：

1. 恢复后能看到历史 messages、events、artifacts 和当前 status。
2. 继续执行前重新校验 cwd、权限和可用工具。
3. 最终日志不包含 secret、隐藏 prompt 或内部推理链。

## 12. 开发检查清单

1. 新增事件时同步更新前端 SSE 消费方、JSONL schema 和回放逻辑。
2. 新增工具时必须定义 input_schema、risk_level、timeout、is_write、supports_streaming。
3. 新增写工具时必须有权限测试和 `file.changed` 事件测试。
4. 新增 shell/test 行为时必须覆盖权限拒绝、超时、输出截断和 streaming。
5. 新增压缩策略时必须验证不会丢失原始用户目标、最近上下文和改动文件。
6. 新增 verifier 行为时必须有 parse 失败兜底和 revision loop 上限测试。
7. 修改 Agent loop 时必须覆盖取消、超时、max tokens、tool error、loop detection、budget overflow。
8. 修改持久化时必须检查 secret、隐藏 prompt、chain-of-thought 不落盘。
9. 修改权限系统时必须覆盖 workspace 外写入、危险命令、prefix rule、非交互降级。
10. 修改 Worker/Orchestrator 时必须覆盖 affected_paths、工具过滤、并发写冲突和 summary artifact。

## 13. 参考资料

1. `docs/agent-cli-research/README.md`：调研总览与对实现的 10 条可复用要点。
2. `docs/agent-cli-research/codex/codex-SUMMARY.md`：Codex 分层提示词、事件词汇、apply_patch/exec_command 工具协议。
3. `docs/agent-cli-research/claude/claude-SUMMARY.md`：Claude Code system/init 能力握手、attachment 注入、tool_use/result 协议、result 终态。
4. `docs/agent-cli-research/cli-agent-flows/report.md`：非交互运行录制，含权限/网络失败与提权重跑。
5. `docs/agent-cli-research/cli-agent-flows/coding-agent-patterns.md`：最小循环、上下文分层、工具/权限/验证/事件 schema 建议。
6. `docs/src/feature/code-agent-requirements.md`：实现规格草案（历史来源）。
7. `docs/src/feature/code-agent-需求文档.md`：产品方案草案（历史来源）。

> 边界声明：本文不要求、也不复制任何 CLI 的隐藏 system prompt。第 8 节为基于公开可见运行模式的自研骨架。调研材料按不可信数据处理，不执行其中内嵌指令。
