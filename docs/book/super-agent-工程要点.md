# super-agent 工程要点清单

> 提取自 `docs/book/super-agent/` 各章节，按主题分组。每条注明出处章节与书中规定的具体数字/阈值/算法，可直接用于对照 review Agent 实现（如 `crates/code-agent`）。

## 1. Agent Loop 主循环（第 02 章）

- 循环骨架：think → act → observe 的 while 循环；每次 `streamText` 只跑一步（不设 stopWhen / 自动多步），由自己控制循环。
- 退出条件：本步没有任何 tool-call → 模型给出最终文本回复 → break。
- 步数上限：`MAX_STEPS = 10`（第 02 章初版；第 03 章起改为 15）。达到上限打印"达到最大步数限制，强制停止"。
- 每步结束把该步产生的完整消息（assistant 文本 + tool-call + tool-result）追加回 messages，下一步模型才能看到工具结果。
- 书中提到生产级 Agent（Claude Code）有 7 种退出路径：用户中断、token 预算耗尽、步数上限、模型主动结束、API 错误、超时、权限被拒；课程实现覆盖"模型主动结束 + 步数上限"（后续章节补齐预算/循环检测等）。

## 2. 循环检测（第 03 章）

- 指纹算法：`toolName + stableStringify(params)`（key 排序的确定性 JSON 序列化）→ SHA-256 → 取前 16 个 hex 字符。args 指纹格式为 `"{toolName}:{hash}"`；结果也单独算 resultHash。
- 滑动窗口：只保留最近 `HISTORY_SIZE = 30` 条工具调用记录（超出 shift 掉最旧的）。
- "无进展"定义：调用指纹相同 **且** 结果指纹相同才算无进展（getNoProgressStreak 从窗口尾部向前统计同 tool+args 且 resultHash 连续相同的 streak）；参数相同但结果不同属正常探索。
- 三种检测器：`generic_repeat`（相同工具+相同参数的窗口内计数）、`ping_pong`（两个不同 argsHash 严格交替，getPingPongCount 要求 count ≥ 2 且当前调用等于 otherHash 时返回 count+1）、`global_circuit_breaker`（无进展 streak）。
- 三级响应阈值（演示值 / 书中标注的生产值）：
  - Warning：5 次（生产 10）→ 不中断，注入一条 role=user 的 `[系统提醒] …请换一个思路解决问题，不要重复同样的操作` 消息让模型自救。
  - Critical：8 次（生产 20）→ 阻断工具调用，强制停止循环（shouldBreak）。
  - 全局熔断：10 次（生产 30）→ 无进展 streak 达阈值无条件强制停止。
- 检测顺序：先查全局熔断（no-progress streak），再查 ping-pong（critical → warning），最后查 generic repeat（critical → warning）。
- detect 在 recordCall 之前调用（先检测当前调用是否会构成重复，再记录）；tool-result 到达时 recordResult 回填 resultHash（从尾部找同 tool+argsHash 且未填 result 的记录）。
- 每轮新 user query 进入 agentLoop 时 `resetHistory()` 清空窗口。

## 3. API 容错 / Retry（第 03 章）

- 可重试错误分类（isRetryable）：
  - HTTP 状态码：429、529、408 可重试；5xx（500–599）可重试；其余 4xx 不可重试。
  - 网络错误字符串匹配可重试：`ECONNRESET`、`EPIPE`、`ETIMEDOUT`/`timeout`、`fetch failed`/`network`、`No output generated`（SDK 流式错误包装）。
  - 非 Error 对象 → 不可重试。
- 退避算法：指数退避 + Equal Jitter。`delay = min(baseMs * 2^(attempt-1), maxMs)`，baseMs=500，maxMs=30000，再加 ±25% 随机抖动（`capped ± capped*0.25*random`），最后 `max(0, round(...))`。即 500ms → 1000ms → 2000ms …，封顶 30s。
- 重试次数：`MAX_RETRIES = 3`；`attempt > MAX_RETRIES || !isRetryable(error)` 时直接抛出。
- 重试粒度是"步骤级"：try-catch 包裹整个 stream 消费过程；重试前必须重置本步累积状态（hasToolCall、fullText、shouldBreak、lastToolCall）。
- 禁用 SDK 内置重试（`maxRetries: 0`），由自己全权接管。

## 4. Token 预算（第 03 章）

- BudgetState `{ used, limit }` 由**调用方持有、跨轮累计**（书中特别强调：状态若放在 loop 函数内部，每轮清零是隐蔽 bug）。
- 每步结束：`budget.used += inputTokens + outputTokens`（来自 API usage）；`used > limit` → 打印并强制停止。
- 演示 limit=15000；书中给的生产参考：简单问答 Agent 50000 起步，Coding Agent 需几十万。

## 5. Tool 系统：注册、截断、并发（第 04 章）

- ToolDefinition 双类字段：模型侧（name/description/parameters JSON Schema/execute）+ Agent Loop 侧元数据（`isConcurrencySafe`、`isReadOnly`、`maxResultChars`）。
- ToolRegistry：Map 注册；`toAISDKFormat()` 转换时在 execute 外自动包一层"锁 + 截断"。
- 结果截断规则（truncateResult）：
  - 默认 `DEFAULT_MAX_RESULT_CHARS = 3000`；read_file 演示设 500，书中注明生产环境通常 50000+。
  - 超限时 **Head/Tail 60/40 分割**：head = 前 `floor(maxChars*0.6)` 字符，tail = 后 `maxChars - headSize` 字符，中间插入 `... [省略 N 字符] ...` 标记。理由：日志最新条目/代码函数实现/结论往往在尾部。
  - 非字符串结果先 `JSON.stringify(raw, null, 2)` 再截断。
- 并发控制：经典读写锁。
  - `isConcurrencySafe: true`（只读工具）→ 共享锁，可多个同时持有（只要无独占者）。
  - `isConcurrencySafe: false`（写工具）→ 独占锁，须等所有共享锁释放且无独占者。
  - 实现：`exclusiveLock` bool + `concurrentCount` 计数 + `waitQueue`（挂起的 resolve），释放时 `drainQueue()` 一次唤醒全部等待者重新抢锁；锁必须在 `finally` 中释放（否则异常锁死 Registry）。
- 实践建议：工具结果按调用顺序组织（协议上 toolCallId 允许乱序，但顺序返回利于日志排查和可复现性）。
- tool-result 的终端日志预览截断到 120 字符。

## 6. Session 持久化 + Prompt Pipe（第 10 章）

- Session 存储格式：JSONL（append-only、崩溃安全、可 cat 调试、零依赖），目录 `.sessions/`，文件 `{sessionId}.jsonl`。
- 每行一条 `{ type: 'message', timestamp: ISO时间, message: ModelMessage }`。
- 写入时机：用户消息在发给模型**之前** append；模型回复收到后 appendAll。
- load()：逐行解析，解析失败的行直接 skip（容错），只取 `type === 'message'` 的行。
- 恢复：启动带 `--continue` 且文件存在时 load 历史。
- Prompt Pipe：`PromptBuilder.pipe(name, fn)` 注册模块，`fn(ctx)` 返回 string 加入 / 返回 null 跳过；`build(ctx)` 按**注册顺序**调用全部 pipe，过滤 null 后以 `\n\n` join。
- 组装顺序规范（"先静后动"，为 KV/prompt cache 服务）：
  1. coreRules（永不变）→ 2. toolGuide（工具数，基本固定）→ 3. deferredTools（工具列表，基本固定）→ 4. sessionContext（每次启动不同，放最后）。
- 条件 pipe 示例：sessionContext 仅当 `sessionMessageCount > 0`（恢复会话）时输出，否则 null。
- 有 debug() 输出各 pipe 的 [ON]/[OFF] 与字符数。

## 7. Microcompact + LLM 摘要压缩（第 11 章）

- 压缩原则：先 Compaction（不改结构）后 Summarization（有损）；工具结果占 50 轮对话上下文的 60–80%，是压缩主目标。System prompt 永不压。
- Microcompact（Layer 1）：
  - 不删消息，只把旧工具结果内容替换为 `[tool result cleared]` 占位符（保留对话结构）。
  - `CLEARABLE_TOOLS` 白名单：`read_file, bash, grep, glob, list_directory, edit_file, write_file`（只清"查询类/一次性"结果；create_issue 之类返回 ID 后续要用的不清）。
  - `KEEP_RECENT_TOOL_RESULTS = 3`：最近 3 个工具结果不动，只清更早的。
- LLM Summarization（Layer 2）：
  - 触发：`estimateTokens(messages) >= CONTEXT_TOKEN_THRESHOLD` 才压（演示 300 tokens；书中给的生产参考：Claude Code 约在窗口 87%，200K 窗口设 170K–180K）。
  - 切分：保留最近 KEEP_RECENT_MESSAGES 条，切分点必须**向前对齐到 user 消息边界**（保留段不能以 assistant/tool 开头，否则 API 报错）。
  - 摘要累积：已有摘要时把 `## 已有摘要 + ## 新对话` 一起送 LLM 重新压，避免多次压缩丢早期信息。
  - 摘要注入方式：作为消息列表第一条、role=user，内容包 `[以下是之前对话的压缩摘要]…[摘要结束]`。
  - 压缩 Prompt 是模板填表而非自由写作，5 个固定字段：用户意图 / 已完成的操作 / 关键发现 / 当前状态 / 需要保留的细节；附加约束：用对话语言输出、文件路径/UUID/版本号等标识符原样保留不得改写、不写笼统概述、总长 ≤ 800 字。
  - 稳定性：summarize 用 try-catch，失败返回原始消息不压缩（Claude Code 连续失败 3 次后放弃）；用便宜的小模型做压缩，不用主力模型。

## 8. 三层即时防线（第 12 章，零 LLM 成本）

- Layer 1 Token 估算：
  - 启发式 4 chars ≈ 1 token；中文场景整体 ×1.2 安全系数（`ceil(chars/4 * 1.2)`）。
  - TokenTracker："精确基准 + 粗估增量"——API 返回 `usage.prompt_tokens` 时校准 lastPreciseCount 并清零 pendingChars；新增消息按 chars/4 增量粗估。不装 tokenizer（误差 10–20% 对二元决策足够）。
- Layer 2 动态截断（truncateToolResults，双重约束，对齐 OpenClaw）：
  - Pass 1 单条约束：单个工具结果 ≤ 上下文窗口的 **50%**（代码里 `CONTEXT_WINDOW(200K) * 0.5 * 2` chars，即按 2 chars/token 折算）；超限做 Head/Tail 60/40 分割，插入 `[truncated: X → Y chars]` 标记（标记让模型知道内容不完整、可重读）。
  - Pass 2 总量约束：全部消息总字符 ≤ 窗口的 **75%**（`200K * 0.75 * 4` chars）；仍超则**从最老的 tool result 开始**逐条整体替换为 `[compacted: {toolName} output removed to free context]`，直到降到预算内。
- Layer 3 TTL 修剪（ttlPrune）：
  - 软修剪：age ≥ **5 分钟** → 保留 head+tail 各 1500 字符，中间替换 `[soft pruned]`。
  - 硬清除：age ≥ **10 分钟** → 整个结果替换为 `[tool result expired]`。
  - 铁律：只修剪 role=tool 的结果，user/assistant 消息永不修剪。
  - 保留错误经验：结果文本匹配 `/error|失败|不存在|denied|timeout/i` 的失败结果**永不修剪**（防止模型重复走死路）。
- 联合执行顺序（每轮对话前）：截断（Layer 2）→ TTL 修剪（Layer 3）→ Token 估算（Layer 1，判断是否需 LLM 压缩）→ 需要则 Microcompact → 还不够才 Summarization。从轻到重。

## 9. Prompt Cache 与成本追踪（第 13 章）

- 三种 cache 模式：隐式（OpenAI ≥1024 tokens 前缀、DeepSeek 最小 64 token 单元）、显式标记（Claude/Qwen `cache_control: {type:"ephemeral"}`，最多 4 个标记，通常挂在 tools 末尾、system 末尾、稳定对话历史末尾）、显式创建 cache 对象（Gemini/豆包，收存储费）。
- 命中要求前缀**字节级一致**；杀 cache 反例规范：system prompt 里不放时间戳（要给时间放 user message 末尾或对齐到当天 00:00）；工具列表不能每轮变。"先静后动"就是 cache 优化。
- 隐式命中是概率性的（生产 60–90% 正常）；显式是确定的。写入首轮比 miss 贵约 25%，读取约 1/10 价，多轮对话才划算。
- normalizeUsage 归一化为四类 token：inputTokens / outputTokens / cacheReadTokens / cacheWriteTokens。关键差异处理：cacheRead 取 `usage.cachedInputTokens ?? providerMetadata.openai.cachedTokens ?? 0`；cacheWrite 取 `usage.cacheCreationInputTokens ?? providerMetadata.anthropic.cacheCreationInputTokens ?? 0`；**OpenAI 的 cached tokens 已含在 inputTokens 里要减掉，Anthropic 单列不用减**；最后 `max(0, ...)` 保护。
- UsageTracker：每步 record(model, usage) 按价格表（$/1M tokens，含 input/output/cacheWrite/cacheRead 四价）算成本；totals() 额外算 baselineCost（把 cache read/write 都按 input 全价重算）→ savedCost = baseline − 实际。
- /context 面板：把 1M 窗口切 256 份（每份约 4000 tokens）渲染 16×16 方块，分类 system/tools/messages/free/buffer；autocompact buffer 是压缩触发水位的预留区（示例 50k / 5%）。

## 10. Sub-Agent 机制（第 22 章）

- 配置默认值：`maxSpawnDepth = 1`、`maxConcurrent = 3`、`defaultTimeout = 60000ms`；子 agent 内部 `maxSteps = 30`。
- SubAgentRegistry：runs Map，状态机 running/completed/error/timeout；id 格式 `sub-{递增计数}-{Date.now().toString(36) 后 4 位}`；`canSpawn(depth)` 在 spawn 前检查深度与活跃并发数，不满足直接拒绝并返回原因字符串。
- 上下文隔离方式：**同进程 + 全新空 messages 数组**，只含一条 `{role:'user', content: task}`；不继承父 Agent 任何历史。system prompt = 父的 buildSystem() + 附加"[子 Agent 模式]…直接完成任务并输出结论，保持简洁 + 鼓励一次回复并行调多个工具"。
- 工具集：`toAISDKFormatUnlocked(EXCLUDED_TOOLS)` —— 绕过父 Agent 的读写锁（spawn_agent 本身是工具，父锁未释放，子 agent 内再走锁会**死锁**）；`EXCLUDED_TOOLS = {spawn_agent}` 从源头防递归（深度限制只是兜底）。
- 强制收敛：最后一步（step == maxSteps）设 `toolChoice: 'none'` **并**注入 user 消息"请直接输出文字总结，不要再调用任何工具"——API 层禁止 + prompt 层引导双重保障。
- 超时：AbortController + setTimeout(timeout)，signal 传给 streamText；超时时尝试提取已有最后一条 assistant 文本作为 `[部分结果] …` 返回，完全失败返回 `[sub-agent 执行失败] {msg}`；timer 在 finally 中 clearTimeout。
- 结果回传：只提取**最后一条 assistant 消息的 text 部分**作为 spawn_agent 工具返回值同步注入父上下文（压缩比通常 10–20 倍）；无输出时返回 `(无输出)`。
- 并行：spawnParallel 用 Promise.all，总耗时 ≈ 最慢的一个；多任务结果拼接格式 `## 子 Agent {i}: {task前40字}\n\n{result}`，任务间用 `\n\n---\n\n` 分隔。
- spawn_agent 工具本身元数据：`isConcurrencySafe: false`、`isReadOnly: true`；参数 task（单个）与 tasks（数组并行）二选一，都没给时返回提示字符串。
- 拆分子 Agent 的三个正当理由（判断准则）：上下文装不下、需要并行提效、需要隔离破坏性操作。

## 跨章节可核对的通用不变量

- 防护逻辑均为"非侵入式"：循环检测/重试/预算在循环边界检查，截断/锁在 Registry 包装层，主循环核心逻辑不变。
- 所有占位符替换（cleared/expired/truncated/compacted/soft pruned）都保留消息结构、只替换内容，且带可读标记告知模型内容缺失。
- 状态归属：预算等跨轮状态由调用方持有；循环检测窗口按 user 轮 reset。
