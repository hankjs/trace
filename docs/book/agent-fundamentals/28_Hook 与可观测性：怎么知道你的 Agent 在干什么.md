# Hook 与可观测性：怎么知道你的 Agent 在干什么

课程
吃透 AI Agent 开发
Hook 与可观测性：怎么知道你的 Agent 在干什么
Hook 与可观测性：怎么知道你的 Agent 在干什么

约 20 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

传统应用出了 bug，你打开日志，看调用栈，找到报错的那一行代码，定位、修复、上线。整个过程可能半小时就搞定。

Agent 出了 bug 就没这么轻松了。

同样的输入，Agent 可能给出不同的输出——模型是非确定性的。一次任务可能跨 50 轮对话、20 次工具调用，调用链无比的长。更麻烦的是 Agent 还有副作用——它写了文件、跑了命令、改了代码，这些操作不可逆。你想复现一个 bug，得把整个对话历史从头重放一遍——这就是为什么 Claude Code 把所有对话记录存成 JSONL 格式的 transcript 文件，支持 --resume 从断点恢复。

没有这个完整的执行记录，Agent 的 bug 几乎无法排查。

传统应用的 bug 是确定性的——同样的输入必然复现同样的错误。Agent 的 bug 是概率性的——跑 10 次可能只有 3 次出问题，而且每次出问题的路径还不一样。你以为修好了，其实只是这次碰巧没触发。

这就是为什么 Agent 开发需要两个传统应用不太需要的东西：Hook（在关键节点插入拦截）和可观测性（知道 Agent 在干什么、花了多少钱、干得好不好）。

Hook：不改源码就能定制 Agent 行为

先说 Hook。

上一篇讲 Harness 的时候说过，HumanLayer 有一条原则："每次 Agent 犯错，分析根因，加一个最小组件防止再犯。" Hook 就是加这个"最小组件"最轻量的方式——你不需要改 Agent 的源码，写个外部脚本挂上去就行。

前面工具管线那篇简单提过 PreToolUse 和 PostToolUse 两种 Hook。但实际上 Claude Code 的 Hook 系统远不止这两种——它在 Agent 生命周期的几乎每个关键节点都埋了 Hook 点，一共有 27 种事件类型。现在，我觉得是时候仔细拿出来说一说了。

按功能分这些事件大概可以分为五组：

工具相关：PreToolUse（工具执行前）、PostToolUse（工具执行后）、PostToolUseFailure（工具失败后）。这三个是最常用的——你可以在工具执行前做安全检查，执行后自动 format，失败后记录日志。

会话生命周期：SessionStart（会话开始）、SessionEnd（会话结束）、Stop（Agent 要结束回复了）、UserPromptSubmit（用户提交消息）。这组 Hook 适合做统计和通知——比如会话结束时自动把结果推送到 Slack。

上下文管理：PreCompact（压缩前）、PostCompact（压缩后）。前面讲上下文压缩的时候说过，压缩时机很关键——PreCompact Hook 甚至可以阻止一次不合时宜的压缩（返回 exit code 2 就行）。

协作相关：SubagentStart、SubagentStop、TeammateIdle、TaskCreated、TaskCompleted。这组是上一章 Multi-Agent 里讲的 Swarm 场景用的——Leader 可以通过这些 Hook 追踪每个 teammate 的状态。

文件和工作区：FileChanged（文件被修改）、CwdChanged（工作目录切换）、WorktreeCreate/WorktreeRemove（Worktree 创建/删除）。这组 Hook 让你能监控 Agent 对文件系统的操作——比如在 FileChanged Hook 里记录 Agent 改了哪些文件，方便事后来审查。

Hook 的执行机制

Hook 的执行过程很简单：Agent 触发事件时，把事件的上下文信息（工具名、参数、结果等）以 JSON 格式通过 stdin 传给 Hook 脚本，脚本处理完通过 exit code 告诉 Agent 下一步怎么走：

exit 0：放行，一切正常
exit 2：阻塞，把 stderr 的内容作为错误信息返回给模型
其他 exit code：非阻塞性错误，只给用户看，不影响 Agent 继续工作

这个设计有一个很重要的特性——Hook 是外部 shell 命令，不是代码内部的回调函数。你不需要导入 Agent 的 SDK、不需要理解 Agent 的内部结构，写个 bash 脚本就能挂上去。这跟 Git hooks 的设计思路是一样的——.git/hooks/pre-commit 就是一个普通的可执行文件，Git 不关心你用什么语言写的。

Hook 还支持 async 模式——脚本在后台执行，不阻塞 Agent 的主循环。这对那些不影响 Agent 下一步决策的操作特别有用，比如发通知、记日志、推送到外部系统。默认 Hook 有 10 分钟的超时限制，超时了会被强制终止。

几个实际的使用场景

说几个我觉得特别有用的场景。

自动 lint：在 PostToolUse 挂一个脚本，每次 Edit 工具修改了文件后自动跑 eslint --fix。Agent 改完代码，格式自动就对了，不需要浪费一次模型调用来做格式化。

安全拦截：在 PreToolUse 挂一个脚本，检查 Bash 工具要执行的命令。遇到 rm -rf、DROP TABLE 这类危险操作直接 exit 2 阻塞，把"这个操作被安全策略禁止"返回给模型。

完成通知：在 Stop 事件挂一个脚本，Agent 完成任务后自动发一条飞书消息通知你。尤其是长时间运行的任务——你不用盯着终端等它跑完。

CI 触发：在 PostToolUse 挂一个脚本，每次 Agent 提交了 Git commit 后自动触发 CI 流水线。Agent 改代码、跑测试、提交，CI 自动启动——整个链路无人值守。这个场景在 Swarm 模式下更有价值——多个 Worker 各自提交代码，CI 自动在后台验证每一次提 commit。

这里有一个 HumanLayer 团队发现的实操细节值得跟大家分享一下：验证机制的输出应该"成功沉默、失败发声"。他们一开始每次代码修改后在 Hook 里跑完整的测试套件，4000 个通过的测试结果全灌进 Agent 的上下文，导致上下文被无用信息淹没了。而改成"hook"之后，效果好了很多。

Hook 的安全边界

Hook 有一个容易被忽视的安全问题：Agent 自己能不能修改 Hook 配置？

如果可以的话就很危险了——Agent 遇到一个被 Hook 阻止的操作，它可能会尝试修改 Hook 配置来绕过限制。Claude Code 的安全模型明确禁止了这一点：Hook 配置是只读的，Agent 在会话期间无法修改。

除了 shell 命令类型的 Hook，Claude Code 还支持 HTTP Hook（向指定 URL 发 POST 请求）和 Agent Hook（用另一个 LLM 来评估当前操作是否应该放行）。HTTP Hook 有一个安全细节值得注意——它要求显式声明允许传递的环境变量（allowedEnvVars 白名单），防止 API Key 之类的敏感信息通过 Hook 泄露到外部。

可观测性：不只是日志，是一整套监控体系

Hook 解决的是"在关键节点插入行为"的问题。但你还需要知道：Agent 整体在干什么、花了多少钱、哪些环节是瓶颈。

这就是可观测性要解决的问题。

一个真实的排查场景

假设你的 Agent 跑了一个重构任务，花了 $15，跑了 40 分钟，最后产出的代码还有 bug。你想知道三件事：钱花在哪了、哪一步最慢、质量问题出在哪。

钱花在哪了？ 打开成本面板，你会看到这次任务总共消耗了 120K input tokens 和 30K output tokens。但更关键的信息是 cache 命中率——如果只有 20%，说明大量 token 在反复计算，没有命中 Prompt Cache。

Opus 4.6 的 cache read 价格只有正常 input 的十分之一（
0.50
𝑣
𝑠
0.50vs5.00/百万 token），命中率从 20% 提到 80% 能直接把成本砍掉一半以上。回头去看前面 Cache 那篇讲的"前缀稳定性原则"，大概率能找到优化点。

哪一步最慢？ 打开调用链追踪，你能看到 40 分钟的任务里，有 25 分钟花在了一个 npm test 的 bash 工具调用上——Agent 每次改完一行代码就跑完整的测试套件，跑了 8 遍、巨慢无比。这不是模型的问题，是 Harness 没有限制测试频率。加一个 Hook 让 Agent 攒够一批修改再统一跑测试，时间可以缩短到 15 分钟。

质量问题出在哪？ 对比调用链里 Agent 的推理过程和最终输出，你发现 Agent 在第 5 轮的时候做了一个错误的判断——有些重要的上下文信息丢失了，或者单纯是模型容易忘记调用某个工具。这提醒你要做一个 Context Engineering 的优化了，比如追加指令、增加必要的上下文的信息。

这三件事——成本、性能、质量——就是 Agent 可观测性的核心。传统应用关心的是 QPS 和错误率，Agent 还得关心 token 消耗、Cache 命中率、工具调用成功率这些独特的指标。

而且这些问题不是"出了 bug 再查"——你应该在每次 Agent 运行时都能看到这些数据。就像开车要有仪表盘一样，Agent 跑起来你需要实时看到成本在累积、上下文在膨胀、工具在被调用。没有仪表盘的 Agent 就跟闭着眼开车没有区别，翻了车都不知道是什么原因导致翻的。

行业里的可观测性工具

做 Agent 可观测性，你不需要从零搭——行业里已经有一批成熟的工具了。这些工具大致分两类，搞清楚区别很重要：

AI Gateway（如 Helicone、Portkey）——坐在你的应用和模型 API 之间，作为代理层。它能自动记录每一次 API 调用的 token 数、耗时、成本，还能加缓存、限流、多提供商路由。集成只需要改一行 base URL：

typescript
复制
import { createAnthropic } from "@ai-sdk/anthropic"

const anthropic = createAnthropic({
  baseURL: "https://anthropic.helicone.ai",
  headers: { "Helicone-Auth": "Bearer sk-helicone-xxx" },
})
// 后面正常用 anthropic("claude-sonnet-4-6") 调模型，代码完全不用改


代码逻辑完全不用动，所有请求经过 Gateway 代理后自动被记录。但 Gateway 看不到 Agent 内部的逻辑（工具选择的推理过程、检索步骤等），只能看到 API 层面的进出。

可观测性平台（如 Langfuse、LangSmith、Arize Phoenix）——通过 SDK 嵌入你的代码，追踪 Agent 执行的完整路径。以 Langfuse 为例，在每次 LLM 调用时加几行追踪代码：

Langfuse 现在推荐的方式是通过 OpenTelemetry（简称 OTel，一套开源的可观测性标准，定义了 trace、span 等数据格式，让不同工具之间的追踪数据可以互通）自动追踪，不需要手动在每次调用前后加代码。初始化的时候注册一个 span processor：

typescript
复制
// instrumentation.ts（应用启动时加载）
import { NodeSDK } from "@opentelemetry/sdk-node"
import { LangfuseSpanProcessor } from "@langfuse/otel"

const sdk = new NodeSDK({ spanProcessors: [new LangfuseSpanProcessor()] })
sdk.start()


然后在 Vercel AI SDK 的 streamText 或 generateText 里打开 telemetry 就行：

typescript
复制
const result = await streamText({
  model,
  messages,
  experimental_telemetry: { isEnabled: true, functionId: "agent-task" },
})


不需要手动 trace、不需要手动 flush——OTel 会自动把每一次 LLM 调用的输入、输出、耗时、token 数上报到 Langfuse。你在 Langfuse 面板里就能看到完整的调用链，包括每一轮工具调用和模型推理，信息深度是 Gateway 做不到的。

两类工具的能力有重叠——Langfuse 也能看成本和 token 消耗，不是只有 Gateway 能做。区别在于集成方式和侧重点：Gateway 零代码改动（改 URL 就行）、侧重运维层面（缓存、限流、路由）；可观测性平台需要加 SDK、侧重开发调试（调用链、推理过程）。如果你两个都用，Gateway 负责运维侧的成本控制和提供商路由，可观测性平台负责开发侧的问题排查。

如果预算有限只能选一个，我的建议是 Langfuse——开源、MIT 协议、可以自部署、所有功能免费、集成了 50 多个框架（包括 OpenAI SDK、Anthropic SDK、Vercel AI SDK、LangChain、LlamaIndex 等）。它在 2026 年初被 ClickHouse 收购了，但仍然保持完全开源。

成本监控是刚需

在所有可观测性指标里，成本是 Agent 场景最独特的一个。

传统应用的计算成本是可预测的——一台机器一个月多少钱，跟用户数和请求量大致线性相关。Agent 的成本完全不可预测——一个简单任务可能 2 轮就完成花 
0.05
，一个复杂任务可能跑
80
轮花
0.05，一个复杂任务可能跑80轮花50。如果 Agent 陷入死循环（前面保险丝那篇讲过的），一分钟就能烧掉几十美元。

所以生产环境的 Agent 必须有成本熔断器——给每个会话或每个任务设一个成本上限，超了就强制停止。这跟前面讲的 token 预算保险丝是一回事。

另外一个值得监控的指标是 Cache 命中率。如果你的 Agent 每次对话的 Cache 命中率都很低（比如低于 50%），说明你的上下文前缀不够稳定——可能是 system prompt 在变、工具列表在变、或者对话结构不够一致。回头去看前面 Cache 那篇讲的"前缀稳定性原则"，大概率能找到优化点。

还有一类异常值得自动检测：死循环和重复工具调用。前面保险丝那篇讲过，Agent 可能会陷入重复调用同一个工具、传同样参数的循环。可观测性系统应该能自动识别这种模式并告警——比如"同一个工具在最近 5 轮内被调用了 3 次以上、且参数完全相同"。这种告警比简单的错误率告警有价值得多，因为死循环不会产生"错误"——每次调用都"成功"了，只是在做无用功烧钱。

如果你之前没做过可观测性，不知道从哪开始，我给一个最简单的落地路径：第一步加 Gateway（Helicone 或 Portkey，改一行 URL，5 分钟搞定），先把成本和 token 消耗看清楚。大部分 Agent 的第一个问题都是"不知道钱花在哪了"，Gateway 能立刻回答这个问题。等你发现需要排查具体哪一步出了问题的时候，再加 Langfuse 做调用链追踪——这是第二步，需要改代码但也不复杂。不要一上来就搭一个大而全的监控系统，从最痛的问题开始解决。

回头看这一篇，Hook 和可观测性解决的其实是 Harness 的"感知能力"——你的 Harness 能不能感知到 Agent 在做什么、做得对不对、什么时候该干预。没有这个感知能力，前面讲的所有 Harness 组件（保险丝、压缩、权限、Evaluator）都是盲人摸象——你不知道它们什么时候被触发了、触发的效果好不好、有没有漏掉什么。

下一篇我们聊 Agent 的部署与调度——Agent 跑在哪、什么时候跑。传统的 serverless 架构对 Agent 几乎不可用，Agent 需要的是长驻进程、状态持久化、崩溃恢复这些跟传统 Web 应用完全不同的基础设施。

参考资料
Claude Code Hooks 文档: https://docs.claude.com/en/docs/claude-code/hooks
Langfuse (开源 LLM 可观测性): https://langfuse.com/
Arize Phoenix (OpenTelemetry-native): https://phoenix.arize.com/
Helicone (AI Gateway): https://www.helicone.ai/
Portkey (AI Gateway): https://portkey.ai/
LangSmith (可观测性): https://www.langchain.com/langsmith/observability
HumanLayer: Skill Issue - Harness Engineering: https://www.humanlayer.dev/blog/skill-issue-harness-engineering-for-coding-agents
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
Harness：聊清楚模型外面的这层壳到底是什么？为什么这么重要？
下一篇 · 第六章：Agent 与 Harness 进阶 —— 编排、观测与部署
部署与调度：Agent 跑在哪、什么时候跑


---
## 代码块


```typescript
import { createAnthropic } from "@ai-sdk/anthropic"

const anthropic = createAnthropic({
  baseURL: "https://anthropic.helicone.ai",
  headers: { "Helicone-Auth": "Bearer sk-helicone-xxx" },
})
// 后面正常用 anthropic("claude-sonnet-4-6") 调模型，代码完全不用改
```


```typescript
// instrumentation.ts（应用启动时加载）
import { NodeSDK } from "@opentelemetry/sdk-node"
import { LangfuseSpanProcessor } from "@langfuse/otel"

const sdk = new NodeSDK({ spanProcessors: [new LangfuseSpanProcessor()] })
sdk.start()
```


```typescript
const result = await streamText({
  model,
  messages,
  experimental_telemetry: { isEnabled: true, functionId: "agent-task" },
})
```
