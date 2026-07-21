# agent-fundamentals 工程要点清单

> 提取自 `docs/book/agent-fundamentals/` 各章节，按主题分组。每条标注出处章节与具体数字/机制，可直接用于对照 review Agent 实现（如 `crates/code-agent`）。

## 一、Agent Loop 结构（第 03 章）

1. **循环形态**：核心是 `while(true)` 开放式循环，不预设步数；退出的最基本判断 = 模型响应中没有工具调用（stop_reason 为 end_turn）。
2. **每轮五步流程**：(1) 准备上下文（先检查是否快爆，触发压缩：Snip → Microcompact → Auto-compact 渐进）；(2) 调 API 流式接收，"边说边执行"；(3) 判断是否继续（7 种退出路径）；(4) 执行工具、按序收集结果塞回 messages；(5) 处理附加任务（排队命令、Memory 预取、Skill 发现），构建下一轮 State。
3. **必须维护的循环状态**：当前轮次数、上一轮继续原因（正常/错误恢复/压缩重试——用于逐轮追溯调试）、压缩执行状态（是否触发过紧急压缩、降了多少 token）、输出被截断的次数、挂起的异步任务。
4. **保险丝示例值**：教学最小实现 maxTurns = 30。
5. **实时反馈**：Agent 不能是黑箱——必须边跑边把中间结果吐给用户（Generator/流式产出中间事件），不能跑完再说。
6. **aborted_streaming 处理**：已到达的文字保留（用户已看到），未执行完的工具调用丢弃。

## 二、流式响应（第 06 章）

1. **协议**：用 SSE（单向推送、标准 HTTP、Last-Event-ID 重连、Header 认证），不用 WebSocket。事件序列：message_start → content_block_start → content_block_delta* → content_block_stop → message_delta → message_stop。
2. **Tool call JSON 碎片解析**：`input` 在 content_block_start 时是空对象占位符；参数通过 input_json_delta 逐片推送，每片不是合法 JSON；必须攒齐所有碎片、在 content_block_stop 时才 parse。过早解析 = 崩溃。
3. **边说边执行**：工具块一完成（content_block_stop）就立刻执行，不等整条消息说完；感知延迟可减 30-50%。
4. **并发安全判断按"工具 + 具体输入"而非工具类型**：Read/Glob/Grep 只读可并发；Edit 必须独占（等所有并发工具完成后再执行，执行期间不跑别的）；Bash 看具体命令（`cat` 可并发、`npm install` 不行）。同文件两个 Edit 必须串行。
5. **结果按原始调用顺序返回给模型，不按完成顺序**（协议层靠 tool_use_id 配对，保序是可调试性的工程惯例）。
6. **级联取消规则**：只有 Bash 工具失败会取消后续兄弟 Bash 工具（shell 命令常有依赖链）；Read 等其他工具失败不级联。
7. **工具审批与 SSE 的双向通信**：审批发生在两次 SSE 流之间（模型输出 tool_use 后 stop_reason="tool_use"、流自然结束）；不需要审批的工具先并发执行，需要审批的排队逐个确认（Promise 挂起直到用户操作）；Claude Code 审批不设超时、一直等（OpenClaw 是 60 秒超时自动拒绝）。
8. **分段推送（OpenClaw Chunked Reply）**：缓冲区攒文字，切点优先级：段落边界（双换行）> 句号 > 空白；上限默认 800 字符强制切；代码块中间强切时先闭合代码块、下一段再重开，保证 Markdown 可渲染。

## 三、API 容错（第 07 章）

1. **错误三分类（先于任何重试逻辑）**：
   - 可重试：429、529、503、408、ECONNRESET（服务端/网络问题）
   - 不可重试：400、401、403、402（客户端问题，重试无效）
   - 需降级：连续多次 529、流式反复断
2. **重试参数**：指数退避 + 随机抖动（jitter）；Claude Code 基础退避 500ms、每次翻倍、最多重试 10 次（可经 CLAUDE_CODE_MAX_RETRIES 配置）。
3. **Retry-After 头优先**：服务端给的等待时间（秒转毫秒）优先于自算退避。
4. **升级阈值**：连续 3 次 529 → 触发模型降级（MAX_529_RETRIES = 3）。
5. **三层降级链**：重试（偶发网络错）→ 流式转非流式（非流式超时可设 120 秒；且流式已积累的 529 计数要传递给非流式层，失败预算连续不清零）→ 模型降级（Opus → Sonnet，只改 model 字段，上下文不重建）。
6. **流中断后的已接收内容处理（按完整性区分）**：完整工具调用（JSON 闭合）保留可执行；不完整的丢弃；用户已看到的文本保留。重试前必须对已执行的工具调用去重（防非流式重试返回同样调用被执行两次）。
7. **半开连接（无错误但无数据）**：服务端心跳每 15 秒发 SSE 注释行 `: heartbeat\n\n`（整个请求生命周期都要发，不能只在开头）；客户端看门狗 30 秒无数据判定失效（心跳间隔必须显著小于客户端超时），检查周期 5 秒，用 reader.cancel() 中断而不是 Promise.race。
8. **对账恢复**：中断后先等几秒 → 从持久化层拉取消息最终状态 → 用 DB 覆盖前端状态 → DB 也没有才保留已收部分。服务端持久化数据是 source of truth。
9. **多 Provider（OpenClaw）**：临时性故障（rate_limit/overloaded/timeout）冷却 1min → 5min → 25min，封顶 1 小时；持久性故障（402/403）退避基数 5 小时、最长 24 小时；限流优先切同 Provider 兄弟模型（速率限制按模型区分），billing/auth 是 Provider 级不适用兄弟切换。

## 四、三个保险丝（第 08 章）

1. **保险丝 1 死循环检测**：
   - 指纹 = SHA256(工具名 + 稳定序列化的参数)（key 排序后再序列化）；
   - 必须同时记录**结果指纹**——只有"相同调用 + 相同结果"才算无进展；结果变了计数重置为 1；
   - 四种检测器（OpenClaw）：① 通用重复（同工具同参数 10 次，只告警不阻断）；② 无进展轮询（参数相同且结果相同）；③ Ping-Pong（从最近调用往回扫 A→B→A→B 交替且两边结果都不变）；④ 全局熔断器（累计 30 次无进展强制停止，无例外）；
   - 三级响应：Warning 10 次（记日志继续）/ Critical 20 次（阻断工具，模型收到错误）/ Break 30 次（全局熔断强停）；
   - 防刷屏：告警每 10 次发一次，不逐次发；
   - 生产级处理顺序：先注入干预消息让模型换策略（如 "[LOOP_WARNING] 你正在反复用相同方式操作且没有进展，请换一种方式"），干预无效再强停。教学代码阈值：count≥5 warn、count≥10 break。
2. **保险丝 2 Token 预算**：
   - 设输出 token 预算（示例 30000/15000）；
   - 达到预算 **90%** 时注入 nudge 消息（措辞关键："已完成 X%……继续工作——不要总结"，防止模型收到信号后本能总结浪费预算）；
   - 递减回报检测：续写 ≥3 次且最近连续 2 次每次增量 < 500 token → 判定递减回报直接停止；教学实现中该检查只在累计输出 > 5000 后启用（避免小输出误触发）。
3. **保险丝 3 输出截断恢复**（stop_reason == max_tokens）三步递进：
   - 第一步：提高 max_output_tokens 上限（8K → 64K），静默重试用户无感；
   - 第二步：注入恢复消息，精确措辞四要素："不要道歉、不要回顾、从断点直接继续、把剩余工作拆成更小的块"；第二次起更强硬（"大幅精简，只列关键结论"）；最多执行 3 次；
   - 第三步：3 次失败后认栽，返回不完整结果并标记"输出被截断"。
4. **七种退出路径**：completed（end_turn）/ max_turns / aborted_streaming / aborted_tools / hook_stopped / blocking_limit / prompt_too_long。
   - max_turns 检查发生在"工具执行完成后、下一轮 API 调用前"——最后一轮的工具会被执行，不会差一步被硬停；
   - blocking_limit：发请求前客户端预估 token，超过 `上下文窗口 - 3000 token 缓冲` 直接不发、立刻退出（Claude Code 阈值 3000）；
   - prompt_too_long：预检漏了、API 返回 413 时不直接退出，先做两轮恢复——Context Collapse（折叠旧工具结果）→ Reactive Compact（LLM 摘要），都失败才退出；
   - aborted_tools：已启动的工具需等完成或超时后再退出。
5. **退出必须带上下文**：告诉用户三件事——停了、为什么停、下一步能做什么。

## 五、Function Calling 与工具定义（第 09 章）

1. **第一原则**：模型只生成 JSON、从不执行；约束解码只保证格式（100% 合法）不保证语义（值可以是编造的）。单靠训练 Schema 符合率仅 93%。
2. **工具幻觉防御三件套**：有限值用 enum；执行前业务级校验（validateInput：绝对路径、不含 `..`、目标文件存在等）；错误信息带纠正上下文（列出实际存在的候选文件，而非 ENOENT）。
3. **工具数量与准确率**（实测数据）：4 个 ~95% / 10 个 ~90% / 30 个 ~71% / 46 个（GitHub MCP，~42K token）~71% / 50+ 个 <50%（~72K token）。Tool Search + defer_loading 把 50+ 工具从 72K 降到 500 token，准确率 49%→74%。
4. **工具定义应包含的元数据**：inputSchema（运行时校验）、动态 description（函数而非字符串）、isConcurrencySafe(input)、isReadOnly(input)、isDestructive(input)、validateInput(input)、checkPermissions(input)、shouldDefer、searchHint、maxResultSizeChars（默认 50,000 字符，超出存磁盘）。isConcurrencySafe 等必须依赖输入而非按工具类型写死。
5. **描述规范**：至少 3-4 句话（Anthropic 测试：详细描述把复杂参数处理准确率从 72% 提到 90%）；相似工具用命名空间前缀（github_list_comments vs slack_list_messages）；description 比 type 更重要（写清"这是用户唯一标识符，UUID 格式"）；"什么时候不该用"比"能干什么"更有价值。
6. **阶段区分**：推理阶段自由文本、动作阶段 tool_use、输出阶段 Structured Output——不要在推理阶段强制 JSON。

## 六、工具调用全流程管线（第 10 章）

七步顺序管线，任何一步失败工具不执行、模型收到"为什么失败 + 怎么纠正"：

1. **参数格式验证**：Schema（Zod/等价物）校验，错误必须带精确路径（"input.file_path: Expected string, received number"），不能只说"参数有误"。不可假设上游一定经过约束解码。
2. **业务逻辑校验**（validateInput）：文件存在性、old_string 可找到且唯一（不唯一时返回"出现了 3 次，请提供更多上下文使其唯一"）。
3. **输入补全和标准化**：不改模型原始输入（改了会破坏 Prompt Cache——原始输入序列化进对话历史），生成带补充字段的副本用于后续步骤（如相对路径转绝对）。
4. **前置 Hook（PreToolUse）**：可修改输入 / 阻止执行 / 注入上下文；返回 0 放行、返回 2 或 `{"decision":"block"}` 阻止、`{"updatedInput":{...}}` 改后放行。
5. **权限检查**（见权限系统一节）。
6. **执行 + 结果处理**：
   - 单条工具结果阈值默认 50,000 字符，超出存磁盘、对话里放摘要 + 文件路径引用；
   - 单条消息内所有工具结果的**总预算默认 200,000 字符**（防 10 个并发工具各 40K 累计爆炸）；
   - OpenCode 参照：最多 2000 行或 50KB 截断；
   - 错误信息是给模型看的（带纠错上下文），不是给人看的错误码。
7. **后置 Hook（PostToolUse）**：修改输出 / 触发后续动作（自动 lint）/ 审计日志。

其他：OpenCode Batch Tool 应用层并发最多 25 个子调用（对不支持 parallel tool_use 的模型的兜底方案）。

## 七、权限系统（第 14 章）

1. **四种权限模式（Claude Code）**：plan（只读）/ default（读自动允许、写需确认）/ acceptEdits（编辑自动、Bash 仍需确认）/ bypassPermissions（全绕过）。分层依据 = 操作可逆性（文件可 git 恢复，Bash 不可控）。
2. **三类规则**：alwaysAllow / alwaysDeny / alwaysAsk，语法 `ToolName(prefix:*)`（如 `Bash(npm:*)`）。**优先级：Deny > Allow > 模式默认行为**。检查顺序：先 Deny、再 Allow、都没匹配走模式默认（需要时弹确认框）。
3. **配置分层**：用户全局 ~/.claude/settings.json < 项目级 .claude/settings.json（项目覆盖全局）；用户选"总是允许"自动写入规则。
4. **危险命令模式列表**：代码执行入口（python/node/ruby/perl/eval/exec）、系统修改（sudo/rm -rf/chmod/chown）、网络（curl/wget/ssh）、包管理器 run（npm run 等可执行任意脚本）。该列表还用于识别过宽的 allow 规则（如 `Bash(python:*)` 等于任意代码执行）。
5. **三层决策**：规则匹配 → 分类器判定（Bash 用 LLM 分类器做语义级判断，区分 git status 与 git push --force）→ 交互式询问。原则：能自动判定就自动判定，只有真有风险的才打断用户。
6. **OpenClaw 五层确定性过滤**：Profile 工具集 → Allow/Deny 白名单 → Owner-only 工具 → Exec Approval（两阶段：先注册审批 ID 再等待，60 秒超时自动拒绝，防 race condition）→ Workspace 路径边界（超出 workspace root 直接拒绝、不问用户）。
7. **OpenCode 两个设计**：拒绝可附带文字反馈（拒绝变学习信号）；拒绝级联（拒绝一个则自动拒绝同 session 排队中的全部；"总是允许"则放行匹配新规则的排队请求）。
8. **安全教训**：记忆文件写入按高风险写操作走审批管线（防持久化注入）；Shell 内置命令（export/typeset/declare）也要纳入校验；不能靠枚举防御，边界要沙箱化。
9. **LLM 分类器进阶方案**：只读工具走快速路径直接放行不进分类器；两阶段判断——Stage 1 限几十个 token 直觉判断"宁可错杀"，Stage 2 允许 ~4000 token 完整思维链；两阶段共享同一 Prompt Cache 前缀；用户连续否决分类器 3 次 → 自动降级为手动确认模式。

## 八、Context Engineering 总纲（第 15 章）

1. **五个维度**：Offload / Reduce / Retrieve / Isolate / Cache。
2. **优先级顺序**：先 Offload → 再 Cache → 然后 Reduce（先 Compaction 后 Summarization）→ 需要时 Isolate → 贯穿 Retrieve。从轻到重。
3. **Compaction vs Summarization 关键区分**：Compaction 不改消息结构、只缩内容、可逆；Summarization 用摘要替换整段历史、结构破坏、不可逆。
4. **Manus 三层 Offload**：Function Calling → Sandbox shell 命令（不占工具定义 token、Cache 稳定）→ 写脚本批处理（上下文只留"脚本执行完了，结果在 /tmp/xxx.json"）。原则：上下文留给推理，数据操作交给代码。
5. **隔离两种模式**：by communicating（子 Agent 独立上下文只回传结果）vs by sharing/Fork（复制完整上下文）。Claude Code：子 Agent 用前者，Auto-compact 摘要生成用 Fork。

## 九、System Prompt 工程化（第 16 章）

1. **模块化**：prompt 拆成独立 section（identity / rules / task guidelines / risk / tool guide / output style）；Prompt Pipe 模式——每个 section 是 `(ctx) => string | null` 的纯函数，自己决定是否出现，Builder 过滤 null 后 join。
2. **静态/动态分界线**：静态（身份、规则、工具指南、输出风格）全放前面做 cache prefix；动态（cwd、git 状态、用户配置、语言、Memory）放后面。动态内容变一个字不能连累前面缓存。
3. **配置分层**：用户全局 CLAUDE.md < 项目级 < 本地私有（CLAUDE.local.md）；低优先级先加载、高优先级后加载（模型对 prompt 末尾注意力更强）；注入用户配置时要显式声明"这些指令覆盖默认行为"。
4. **高频变化信息走消息内注入**（如 `<system-context>` 标签附在用户消息里），不进 system prompt——不破坏缓存、每轮可不同、不增加 turn。
5. **Context Rot 事实**：Lost in the Middle；200K 窗口在 60-70% 占用时表现已开始下降；模型有"上下文焦虑"（快满时主动偷懒、跳步骤）。
6. **第一性原理**：最好的压缩是不需要压缩——控制入口，一开始就少放东西。
7. **todo.md 反直觉**：把任务清单写文件再读回是主动的注意力操控，值得花这点 token。

## 十、上下文压缩（第 17 章）

1. **总原则**：先 Compaction 后 Summarization；能可逆不做不可逆；先清工具结果、再砍老消息、最后才 LLM 摘要。
2. **Claude Code 三层**：
   - **Microcompact**：把老工具结果替换为 `[Old tool result content cleared]`（3000 token → <10）；清理结果的白名单：Read/Bash/Grep/Glob/WebSearch/WebFetch；清理调用参数的白名单：Edit/Write（输入比返回大）；保留最近 N 轮工具结果不动。
   - **Snip**：从对话头部砍老消息、保留尾部；必须保证砍后序列合法——第一条必须 user 角色、tool_use 与 tool_result 成对（不能产生孤儿消息，否则 API 报错）；砍后插入边界标记告知模型"前面历史被截断"；Snip 释放的 token 数传给 Auto-compact 参与其触发判断（两者可同时生效，非互斥）。
   - **Auto-compact（Summarization）**：触发阈值公式 `effectiveContextWindow - 13K 缓冲`，effectiveContextWindow = 窗口 - 20K 摘要输出预留；200K 窗口约在 167K（~87%）触发。五步：① 剥离图片/文档为 `[image]` 标记；② Fork 独立子线程生成摘要（不阻塞主循环）；③ 摘要 = `<analysis>`（后处理剥离）+ `<summary>` 严格 9 段结构（用户意图/技术概念/文件改动/错误修复/问题解决/用户消息/待办任务/当前工作/下一步）；④ 压缩后恢复最近读过的 5 个文件，每个最多 5K token，恢复阶段（文件+skills）总预算 50K token；⑤ 替换消息为 [压缩边界标记][摘要][恢复文件][最近对话]。压缩本身连续失败 3 次后放弃自动压缩。
3. **OpenClaw 四层**（对照参考）：
   - Tool Result Guard：每次发送前实时检查；单个 tool result 上限 = 窗口 50%（2 chars/token 换算），总预算 = 窗口 75%（4 chars/token）；超限先在内容 70% 处找最近换行截断并附 `[truncated: ...]`；仍超预算则从最老 tool result 开始逐条替换为 `[compacted: tool output removed to free context]`；
   - History Limiting：按轮次保留最近 N 轮，裁剪后做 tool_use/tool_result 配对修复；
   - LLM Compaction：默认保留最近 3 轮不压（可配至 12 轮）；token 估算 chars/4 × 1.2 安全系数；分段摘要再合并；结构化 5 段（Decisions / Open TODOs / Constraints / Pending user asks / Exact identifiers）；质量守卫检查 5 段齐全 + 标识符完整 + 最近用户请求有体现，不过重试最多 3 次；identifierPolicy: strict（默认，UUID/hash/key 原样保留）/off/custom；
   - Overflow Retry：API 返回 context overflow 后最多重试 3 次（每次再压一轮），仍不够则激进截断所有大 tool result；全流程 5 分钟硬超时。
4. **保留错误原则**：压缩摘要必须保留"遇到了什么错误以及怎么修复的"（Claude Code prompt 中的明确指令），否则模型会重蹈失败方案。
5. **Manus 可恢复压缩**：每个工具调用维护完整版（存文件系统）+ 紧凑版（只留路径引用），信息可读回。Cursor A/B：大输出写文件 + grep/tail 按需读，token 省 46.9% 且表现更好。

## 十一、Cache（第 18 章）

1. **三层概念**：KV Cache（推理层，只能间接影响：前缀稳定）/ Prompt Cache（API 层，ROI 最高）/ Context Collapse（应用层，可逆折叠；Claude Code 实验中阈值约 90% 时把老消息 commit 到 store、可投影回来）。
2. **Anthropic 显式缓存关键数字**：cache_control `{type:"ephemeral"}`；命中 90% off，TTL 5min/1h；首次写入多付 25%，读取 1/10 价格。
3. **breakpoint 放置规则（显式缓存最佳实践）**：打两个标记——① system prompt 最后一个 block（缓存全部静态规则）；② 最后一条用户消息的最后一个 block（缓存整个对话历史）。这样每轮只有最新一条消息 cache miss。
4. **前缀稳定规则**：前缀匹配从第一个字节起，一个字符不同后面全 miss；时间戳等动态内容绝不能放开头/中间，一律集中放最后；按稳定性从高到低排列内容。
5. **隐式缓存参照**：OpenAI 前缀至少 1024 token、75-90% off；DeepSeek 最小单元 64 token、99% off、存储免费；隐式缓存命中率是概率性的（按节点、LRU、TTL），生产 60-90% 正常。
6. **模型路由**：静态按任务类型路由即可起步——主推理用大模型，摘要压缩/只读 sub-agent 用小模型；50 轮任务全 Opus $5-10 vs 混合 $1-2。
7. **成本量级**：10+ 轮会话无缓存 $2-3，有缓存 $0.3；Cache + 路由 + 入口管理叠加约省 10 倍。

## 十二、Sub-Agent / 上下文隔离（第 25 章）

1. **拆 Agent 的唯一正当标准**：上下文真的装不下（不是"分角色"）。子 Agent 探索消耗几万 token，回传摘要仅 1-2K（10-20 倍压缩比）。角色扮演式三件套（Planner/Executor/Reviewer）是误区——36.9% 的多 Agent 失败来自协调问题。
2. **委派两种方式**：轻量委派（只给任务描述）/ 重量委派（附带上下文片段）。
3. **结果回传**：子 Agent 最终输出作为工具结果注入父上下文，同样走截断和持久化流程；OpenClaw Announce Queue：1 秒防抖、投递失败指数退避 2s→4s→8s…上限 60s。
4. **隔离三思路**：① 进程内（AsyncLocalStorage + 每个子 Agent 克隆一份文件状态缓存，不共享）；② git worktree 文件系统隔离（node_modules 软链接回主目录）；③ 队列串行化（OpenClaw Lane：main 4 并发 / cron 1 / subagent 8；session 内严格串行，嵌套队列）。
5. **并发安全**：只读工具可并发、写工具排队；隔离是默认、共享必须显式；abort signal 沿 Agent 树向下传播（父取消则全部子孙停）；深度限制——OpenClaw maxSpawnDepth = 1（子不能再派子），Claude Code 靠子 Agent token 预算自然收敛。

## 十三、Harness 设计（第 27 章）

1. **本质**：每个 Harness 组件编码一个"模型做不到"的假设；模型升级后要逐个重测组件、一次只删一个、效果没变差就删（失效假设不是无害的——多余组件拖慢速度、耗 token）。
2. **加组件原则（HumanLayer）**：每次 Agent 犯错，分析根因，加一个**最小**组件防止再犯；不要提前设计。
3. **"成功沉默、失败发声"**：验证机制通过时不向上下文输出任何内容，只在失败时注入错误（反例：4000 个通过的测试结果灌进上下文）。
4. **Generator/Evaluator 分离**：模型自评不可靠（自信地称赞平庸产出）；独立 Evaluator 不看 Generator 推理过程、只看最终输出；用实际操作验收（Playwright 点按钮）而非读代码；先对齐验收标准（Sprint Contract）再干活。
5. **编排要轻、执行要重**：≥90% 资源花在子 Agent 实际工作上；编排层成本占比超 20% = 设计过度。
6. **两层分化**：能力补偿层（Sprint、上下文重置、Deferred Loading）随模型变强会变薄可删；系统工程层（权限、成本控制、外部容错、可观测性、失败恢复）永远存在。

## 十四、Hook 与可观测性（第 28 章）

1. **Hook 事件面**（Claude Code 共 27 种），五组：工具（PreToolUse/PostToolUse/PostToolUseFailure）、会话生命周期（SessionStart/SessionEnd/Stop/UserPromptSubmit）、上下文管理（PreCompact/PostCompact——PreCompact 返回 exit 2 可阻止压缩）、协作（SubagentStart/SubagentStop/TeammateIdle/TaskCreated/TaskCompleted）、文件与工作区（FileChanged/CwdChanged/WorktreeCreate/WorktreeRemove）。
2. **执行机制**：事件上下文以 JSON 经 stdin 传给外部脚本；exit 0 放行、exit 2 阻塞并把 stderr 内容作为错误返回给模型、其他 exit code 非阻塞（只给用户看）。Hook 是外部 shell 命令而非进程内回调（Git hooks 风格）。支持 async 模式（后台不阻塞主循环）；默认超时 10 分钟强制终止。
3. **Hook 安全边界**：Hook 配置对 Agent 只读，会话期间 Agent 不能修改（防绕过）；HTTP Hook 要求 allowedEnvVars 白名单显式声明可传环境变量（防 API Key 泄露）。
4. **可观测性三大核心指标**：成本（token in/out、Cache 命中率——命中率 20%→80% 可砍一半以上成本；低于 50% 说明前缀不稳定）、性能（调用链追踪，找出最慢工具调用）、质量（对比推理过程和输出定位错误决策轮次）。
5. **必备机制**：
   - 完整执行记录：所有对话存 JSONL transcript，支持 --resume 断点恢复（没有它 Agent bug 几乎无法排查）；
   - 成本熔断器：每会话/任务设成本上限，超了强制停止；
   - 死循环自动告警：如"同一工具最近 5 轮内被调用 ≥3 次且参数完全相同"（比错误率告警有价值——死循环每次调用都"成功"）；
   - 仪表盘应实时可见（每次运行都看到成本累积、上下文膨胀、工具调用），不是出 bug 再查。
6. **落地路径**：先 Gateway（改一行 base URL 看清成本）→ 再可观测性平台（SDK/OTel 调用链追踪），不要一上来搭大而全。
