# code-agent 实现审查报告

> 审查对象：`crates/code-agent`
> 审查依据：`docs/book/super-agent-工程要点.md`、`docs/book/agent-fundamentals-工程要点.md`（下文分别简称【SA】【AF】，标注章节号）
> 审查日期：2026-07-21

## 总评

该 crate 覆盖了两书的大部分主题：agent loop、循环检测、retry、token 预算、三层压缩、工具截断、权限门控、Verifier、Orchestrator/Worker 子代理、分层 prompt、事件可观测性。整体骨架与书中架构一致，且有几处做得比书中教学代码更好（见文末"符合项"）。

但存在 **3 个必现 bug（P0）**、**5 个与书中明确规范相悖且实际会出问题的缺陷（P1）**，以及若干规范偏差（P2/P3）。P0/P1 中多个问题的共同根源是 **tool_use / tool_result 配对被破坏**——这是 Anthropic API 的硬约束，破坏后下一次请求直接 400。

修复顺序建议：P0 全部 → P1 的 #4、#6 → 其余按优先级。

---

## P0 — 必现 bug，功能失效

### #1 Verifier 收到第一个 Usage 事件就退出流，验证机制恒为空转

- **位置**：`crates/code-agent/src/agent/verifier.rs:136`
- **现象**：事件循环里 `Ok(_) | Err(_) => { break; }` 把所有未显式匹配的事件当作流结束。而 Anthropic provider 在 `message_start` 时**第一个**就发 `StreamEvent::Usage`（`crates/hank-provider/src/anthropic.rs:195`）。verify() 在收到任何文本前就 break，`final_text` 为空 → `parse_verification` 解析失败 → 默认返回 Approved（verifier.rs:230）。
- **后果**：接 Anthropic provider 时整个 FR-VERIFY 验证机制是恒通过的空操作。测试的 MockProvider 不发 Usage 事件，所以现有测试测不出来。
- **修复**：对未知事件 `Ok(_) => {}` 忽略、`Err(_)` 单独处理（参照 `worker.rs:186` 的写法）。同时给 MockProvider 的脚本加上 leading `Usage` 事件，让测试覆盖真实事件序列。
- **验收**：集成测试中 Verifier 脚本以 `Usage → TextDelta(JSON verdict) → MessageEnd` 顺序发事件，verify() 能正确解析出 NeedsRevision。

### #2 Orchestrator / Worker 的 loop nudge 消息插在 tool_use 与 tool_result 之间，破坏配对

- **位置**：`crates/code-agent/src/agent/orchestrator.rs:771-780`、`crates/code-agent/src/agent/worker.rs:281-290`
- **依据**：【AF 06/10 章】tool_result 必须出现在紧跟 assistant(tool_use) 的下一条 user 消息里。
- **现象**：循环检测告警时直接 `messages.push` 一条独立 user 文本消息，插在 assistant(tool_use) 和 tool_results 之间 → 下一轮请求 Anthropic API 直接 400。
- **对照**：`session.rs:723-729` 已经用 `pending_loop_nudge` 方案正确修复了同一问题（nudge 追加进 tool_results 同一条消息，注释写明原因）。三处实现不一致，两处是坏的。
- **修复**：把 session.rs 的 `pending_loop_nudge` 方案统一搬到 orchestrator.rs 和 worker.rs。

### #3 循环检测"终止"没有真正终止，且留下未配对的 tool_use

- **位置**：`crates/code-agent/src/session.rs:710-721`、`crates/code-agent/src/agent/orchestrator.rs:757-769`、`crates/code-agent/src/agent/worker.rs:266-279`
- **现象**（三处各有问题）：
  - session.rs：`should_terminate` 命中后只给**当前这一个** tool_use push 错误结果就 `break` 内层 for —— (a) 同一条 assistant 消息里其余 tool_use 块没有 tool_result → 配对破坏，下一轮 400；(b) `break` 只跳出块遍历，外层 `for iteration` 照常继续，日志写着 "Agent terminating" 但循环没停。
  - orchestrator.rs：同上，且 break 后 `delegate_tasks` 还照常执行，最终返回 `Continue` 继续循环。
  - worker.rs：确实 return 终止了，但同样丢了其余 tool_use 的配对（此时如果上层持久化 messages 再恢复会 400），且此前 nudge 已插坏消息序（见 #2）。
- **修复**：terminate 命中时，给**本条 assistant 消息中所有剩余 tool_use** 各补一条错误 tool_result（内容如 "Loop detected, execution aborted"），push 完整的 tool_results 消息后，再通过标志位/返回值退出外层循环。
- **验收**：构造脚本让模型一次发 3 个 tool_use 且触发 terminate，断言 messages 中每个 tool_use 都有对应 tool_result，且外层循环不再进入下一轮 LLM 调用。

---

## P1 — 与书中明确规范相悖，实际会出问题

### #4 LoopDetector 缺"结果指纹"，且阈值远低于书中规定 → 大量误报

- **位置**：`crates/code-agent/src/agent/loop_detector.rs`
- **依据**：【SA 03 章】【AF 08 章】同一要点：**只有"相同调用 + 相同结果"才算无进展**，参数相同但结果不同属正常探索；resultHash 必须回填。阈值：warning 5（生产 10）/ critical 8（生产 20）/ 全局熔断 10（生产 30），窗口 30。
- **现象**：只对 `tool_name + args` 做指纹，完全没有 resultHash。叠加 `repeat_threshold: 2`、窗口 6 —— 窗口内同一调用出现第 2 次就报 LoopDetected 并注入 nudge。"读文件 A → 改 → 再读文件 A 确认"这种标准工作流必然误报。而误报在 orchestrator/worker 里又触发 #2 的消息序破坏，**误报直接升级为致命错误**。
- **修复**：
  1. 增加 `record_result(tool_name, args_hash, result)` 回填结果指纹（工具执行完成后调用，从窗口尾部找同 tool+argsHash 且未填 result 的记录）；
  2. "无进展 streak" 定义改为调用指纹 **且** 结果指纹连续相同；
  3. 阈值改为书中口径：warning ≥5 注入 nudge、critical ≥8 阻断、无进展 streak ≥10 全局熔断；窗口扩到 30；
  4. 每轮新 user query 进入 loop 时 reset（session.rs 目前每次 run 新建 detector，等效满足；orchestrator 的 detector 是成员变量，跨 run 复用时需显式 reset）。
- **验收**：单测覆盖"同参数不同结果不计 streak"、"同参数同结果 5 次触发 warning、8 次触发 critical"。

### #5 MaxTokens 截断时已完成的 tool_use 块 + 续写提示 → 配对破坏

- **位置**：`crates/code-agent/src/session.rs:632-648`、`orchestrator.rs:595-617`、`worker.rs:223-238`
- **依据**：【AF 07 章】流中断后按完整性区分：完整工具调用（JSON 闭合）保留**并执行**，不完整的丢弃。
- **现象**：MaxTokens 时 assistant_content 已 push 进 messages。注释只考虑了 in_tool_block 的不完整调用已被丢弃，但如果截断发生在**若干完整 tool_use 块之后**（多工具调用被截断的常见情形），assistant 消息里含 tool_use，下一条消息却是纯文本续写提示而非 tool_result → API 400。
- **修复**（二选一）：
  - 方案 A（推荐，符合书中规范）：MaxTokens 且 assistant_content 中含完整 tool_use → 正常执行这些工具、push tool_results，然后继续循环（不注入续写提示）；
  - 方案 B：push 前从 assistant_content 剥离所有 tool_use 块，只保留文本，再注入续写提示。
- **验收**：脚本模拟 `ToolUseStart/InputDelta/ToolUseEnd → MessageEnd(MaxTokens)`，断言后续消息序列合法（每个 tool_use 有配对 result 或 tool_use 被剥离）。

### #6 压缩切分点未对齐 user 消息边界 —— 书中点名的 API 报错来源

- **位置**：`crates/code-agent/src/context/manager.rs:229-235`（compress_async 的 recent 窗口）、`manager.rs:259-270`（truncate_oldest）
- **依据**：【SA 11 章】切分点必须**向前对齐到 user 消息边界**，保留段不能以 assistant/tool 开头，否则 API 报错。【AF 17 章】Snip 裁剪后必须做 tool_use/tool_result 配对修复，第一条必须 user 角色。
- **现象**：机械地取"最后 PRESERVE_RECENT(6) 条消息"，不做角色对齐、不做配对修复。若保留窗口第一条是 assistant，或是一条 tool_results user 消息而其对应 tool_use 被压缩掉 → 压缩本身制造 400。长对话中压缩触发越频繁越容易命中。
- **修复**：计算 recent 窗口起点后，向前扫描直到落在一条"纯文本 user 消息"（非 tool_results）上；或在切分后跑一个配对修复 pass（删孤儿 tool_result / 给孤儿 tool_use 补占位 result）。`compress()` 同名旧方法与 `truncate_oldest` 一并处理。
- **验收**：单测构造 `user → assistant(tool_use) → user(tool_result) → …` 长序列，压缩后断言：第一条非 summary 消息是 user 纯文本、无孤儿 tool_use/tool_result。

### #7 工具结果后的预算检查使用过期的 actual token，检查形同虚设

- **位置**：`crates/code-agent/src/session.rs:766-799`（及 orchestrator.rs:1034、worker.rs:329 同模式）、根因在 `crates/code-agent/src/context/manager.rs:117-119`
- **依据**：【SA 12 章】TokenTracker 规范："精确基准 + 粗估增量"——API 返回 usage 时校准基准并清零增量，之后新增消息按估算**增量累加**。
- **现象**：`check_budget` 优先用 `actual_input_tokens`，而它是上一次 LLM 响应时记录的值，**不包含刚 push 进去的工具结果**。巨大的工具输出要等到下一次 LLM 调用（可能已经 prompt_too_long 失败）才被察觉——"catch large tool outputs" 的注释与实际行为不符。
- **修复**：ContextManager 增加 `pending_estimated: usize`；`update_actual_tokens` 时清零；每次 push 新消息后调用 `add_pending(estimate_tokens(&new_msgs))`；`check_budget` 用 `actual + pending`。
- **验收**：单测：actual=100K（预算 200K），push 一条估算 150K 的工具结果后 check_budget 返回 Overflow100。

### #8 流消费阶段的错误/超时不重试，超时被静默当作正常结束

- **位置**：`crates/code-agent/src/retry.rs`（只覆盖建连）、`crates/code-agent/src/session.rs:482-494`（超时分支）、`session.rs:543-551`（消费中 Err 直接 return）
- **依据**：【SA 03 章】重试粒度是"步骤级"——try-catch 包住整个 stream 消费过程，重试前重置本步累积状态。【AF 08 章】退出必须带上下文：停了、为什么停、下一步能做什么。
- **现象**：
  1. `stream_with_retry` 只在 `provider.stream()` 建连失败时重试；流建立后消费中的 `Err(e)` 直接失败整个 run；
  2. 更隐蔽：`LLM_STREAM_TIMEOUT_SECS`(120s) 超时后 `event = None` → break，`stop_reason` 保持默认 `EndTurn` → 该轮被当作模型正常说完话，run 最终报 **Success**。超时静默伪装成成功是最坏的失败模式。
- **修复**：
  1. 超时分支设置显式标志，超时后终止 run 并发带原因的失败/告警事件（不能落入 EndTurn 正常收尾路径）；
  2. 步骤级重试：把"发请求 + 消费流"包成一个可重试单元，消费中可重试错误（连接 reset 等）时丢弃本步累积状态（assistant_content、current_*）重来，最多 MAX_RETRIES 次。orchestrator.rs / worker.rs 的三份流消费循环同步处理（顺带建议：三份几乎相同的消费循环抽成共享函数，本次 review 多个问题都是"三处实现不一致"导致的）。
- **验收**：Mock stream 中途发 Err(可重试)，断言重试后成功且消息里无半截内容；mock 超时（挂起的 stream），断言 run 以失败/超时事件收尾而非 RunCompleted(Success)。

---

## P2 — 规范偏差 / 缺失能力

### #9 Prompt cache 铺垫不足

- **位置**：`crates/code-agent/src/prompt_pipe.rs:142-175`（分层顺序）、`crates/hank-provider/src/anthropic.rs`（无 cache_control）、`crates/hank-provider/src/types.rs:56`（Usage 无 cache 字段）
- **依据**：【SA 13 章】【AF 16/18 章】"先静后动"；system 里不放时间戳（放 user message 末尾或对齐到当天 00:00）；显式缓存打两个 breakpoint（system 末尾 + 最后一条 user 消息末尾）；usage 归一化含 cacheRead/cacheWrite 四类。
- **现象**：
  1. 分层顺序 base → developer → **environment（含 current_date）** → project：动态的日期放在静态的项目记忆前面，每天第一次运行就击穿 project 段缓存；
  2. provider 层完全没有 `cache_control` 断点；
  3. Usage 事件没有 cache_read/cache_write 字段，成本可观测性缺一角。
- **修复**：environment 段移到 project 之后（或 current_date 对齐到当天 00:00）；anthropic provider 在 system 末尾与最后一条 user 消息末尾加 `cache_control: {type:"ephemeral"}`；StreamEvent::Usage 扩展 `cache_read_tokens` / `cache_write_tokens`（Anthropic 单列不用减，OpenAI cached 已含在 input 里要减，见 SA 13 章 normalizeUsage）。

### #10 Microcompact 无白名单、且会修剪错误结果

- **位置**：`crates/code-agent/src/context/summary.rs:147-170`
- **依据**：【SA 11 章】只清"查询类"工具结果（CLEARABLE_TOOLS 白名单），返回 ID 后续要用的（如 create_issue）不清。【SA 12 章】匹配 `/error|失败|不存在|denied|timeout/i` 的失败结果**永不修剪**（防止模型重蹈死路）。保留策略是"最近 3 个**工具结果**"，非"最近 N 条消息"。
- **现象**：对所有 ToolResult 一视同仁截到 80 字符：错误结果被剪掉（summarize prompt 里"保留失败经验"的约束在 microcompact 层丢失）；无白名单；保留窗口按消息数而非工具结果数。
- **修复**：microcompact 增加 `is_error == true` 跳过；按 ToolResult 计数保留最近 N 个；（可选）传入 clearable 工具名集合。

### #11 MaxTokens 恢复缺"第一步"与递减回报检测

- **位置**：`crates/code-agent/src/session.rs:632-648` 等三处
- **依据**：【AF 08 章】三步递进：① 静默提高 max_output_tokens（8K→64K）重试；② 注入恢复消息，措辞四要素"不要道歉、不要回顾、从断点直接继续、拆成更小的块"，第二次起更强硬，最多 3 次；③ 认栽并标记"输出被截断"。另有递减回报检测：续写 ≥3 次且连续 2 次增量 <500 token → 直接停止。
- **现象**：直接跳到注入消息，措辞只有"Continue from where you left off"一要素；没有提额一步；没有递减回报检测；3 次后 break 但无"输出被截断"标记事件。
- **修复**：首次 MaxTokens 先把 max_tokens 提到上限重试；续写消息补齐四要素；跟踪每次续写的 output_tokens 增量做递减回报判停；最终放弃时发事件告知截断。

### #12 Worker 异常退出仍报 Success

- **位置**：`crates/code-agent/src/agent/worker.rs:365-369`
- **依据**：【SA 22 章】超时应标 `[部分结果]`；【AF 08 章】退出必须带原因。
- **现象**：最终 status 只看 `cancel.is_cancelled()`。预算 overflow break、达 WORKER_MAX_ITERATIONS break、流超时 break —— 全部报 `TaskStatus::Success`。Orchestrator 依赖 `last_worker_failed` 决定是否 re-Think，该信号因此失真。
- **修复**：用枚举记录退出原因（Completed / MaxIterations / BudgetOverflow / StreamTimeout / Cancelled），非 Completed 时 status 置 Failed 或加 `[partial result]` 前缀。

### #13 取消 / ask_user 路径丢弃已执行的工具结果，留下未配对 tool_use

- **位置**：`crates/code-agent/src/session.rs:660-663`（工具间取消）、`session.rs:750-754`（ask_user）
- **依据**：【AF 17 章】持久化的消息序列必须 tool_use/tool_result 成对；【AF 07 章】重试/恢复前已执行的工具调用要去重（不幂等风险）。
- **现象**：两条路径都直接 return，不 push 已收集的 tool_results。持久化的 messages 以未配对的 assistant(tool_use) 结尾：恢复会话时第一次请求就 400；已执行的写操作结果被丢弃，恢复后可能重复执行。
- **修复**：return 前把已收集的 tool_results push 进 messages，未执行的 tool_use 补占位错误 result（如 "cancelled before execution" / ask_user 的 tool_use 留给上层用用户答案回填——需与 server 端恢复逻辑核对协议）。

### #14 Deferred tool 首调时机错位

- **位置**：`crates/code-agent/src/session.rs:665-675`、`orchestrator.rs:730-744`
- **依据**：【AF 11 章 / 09 章】deferred loading 的意义是让模型在拿到完整 schema 后再构造参数。
- **现象**：schema 注入发生在模型**已经**用空 schema 猜完参数之后，且这一次盲猜调用被直接放行执行。
- **修复**：首次调用 deferred 工具时注入 schema 并返回错误 tool_result："Tool schema now loaded, please retry with correct parameters"，不执行盲猜参数。

### #15 达到 MAX_ITERATIONS 静默结束

- **位置**：`crates/code-agent/src/session.rs:865-867`、`orchestrator.rs:313-316`、`worker.rs:345-347`
- **依据**：【AF 08 章】max_turns 是七种退出路径之一，退出必须告诉用户：停了、为什么停、下一步能做什么。
- **现象**：只有 `warn!` 日志，无面向用户的事件，run 照常报 Success。
- **修复**：达上限时发一个明确事件（可复用 Error 或新增 RunAborted{reason}），summary 中注明 "reached max iterations"。

---

## P3 — 参数口径 / 小改进

| # | 位置 | 问题 | 建议 |
|---|------|------|------|
| 16 | `retry.rs:9-13,63-69` | BASE_DELAY_MS=1000（书 500）、MAX_RETRIES=3（Claude Code 10）、jitter 加性 0-50%（书 Equal Jitter ±25%） | 教学口径差异可接受，注释说明取舍即可 |
| 17 | `session.rs:576-577` | `run_state.input_tokens` 取 `max()`（峰值上下文），RunCompleted 语义像累计用量，口径混用易误读 | 字段改名 `peak_input_tokens` 或改为累计，二者取一并注释 |
| 18 | `session.rs:591-624` | Simple 模式 check_budget 无 Warning80 分支（orchestrator 有），80-95% 区间不告警不压缩 | 与 orchestrator 对齐补 Warning80 分支 |
| 19 | `worker.rs:310-319` | 每个成功工具输出（截断后仍可达 40K 字符）clone 进 artifacts，orchestrator 只用 summary | artifacts 只存路径/摘要引用，或干脆移除 |

---

## 符合项（无需改动，防止误伤）

- 工具截断 60/40 head/tail + 显式 `[truncated x of y chars]` 标记，全程按字符数防 CJK 提前截断（`summary.rs:19-42`）——符合【SA 04/12 章】。
- CJK 感知 token 估算 1.5 chars/token（`summary.rs:71-98`）——比书中 "×1.2 安全系数" 更精细。
- Retry-After 优先于自算退避、HTTP 状态码独立匹配防 5000 误命中（`retry.rs:44-88`）——符合【AF 07 章】。
- 工具错误分类注入 `[error_type: ...]` 纠正上下文（`tool_runtime.rs:341-373`）——符合【AF 09 章】。
- 权限 NeedApproval 非交互场景优雅降级为 Denied 并双发事件；Worker 继承父权限边界 FR-PERM-6（`tool_runtime.rs:90-148`、`worker.rs:41-63`）。
- 只读工具并行执行且 join_all 保持调用顺序回填（`orchestrator.rs:1116-1237`）——符合【AF 06 章】"按原始调用顺序返回"。
- Verifier 解析失败默认 Approved 防无限修订循环、MAX_REVISIONS=2 封顶（`verifier.rs:229-235`）。
- 事件面完整（run/turn 生命周期、Metrics、ToolMetrics、LlmRequest、CompressionTriggered）——符合【AF 28 章】可观测性要求。

## 通用修复建议

session.rs / orchestrator.rs / worker.rs 存在**三份近乎相同的流消费循环与工具执行循环**，本次 P0/P1 中 #2、#3、#5、#8 均属"一处修了、另两处没修"或"三处行为不一致"。建议在修复 P0 时顺带把流消费循环抽成共享函数（输入 stream + cancel + timeout，输出 assistant_content + stop_reason + usage + 终止原因枚举），从结构上消除这类不一致。
