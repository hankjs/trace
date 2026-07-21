# 社区各大框架全景：用六大支柱透视任何 Agent 框架

课程
吃透 AI Agent 开发
社区各大框架全景：用六大支柱透视任何 Agent 框架
社区各大框架全景：用六大支柱透视任何 Agent 框架

约 40 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

每个月社区都会冒出来一些新的 Agent 框架，每个都声称"最强大"、"生产级"。你不可能每个都学，也没必要每个都学。

但你现在有一个优势——你手里已经有了一把尺子。

前面学的六大支柱（Agent Loop、Tool System、Context Engineering、Memory、Multi-Agent、Harness），拆开了 Claude Code、OpenClaw、Manus 这些产品的核心架构。拿这把尺子去量任何框架，30 分钟就能定位它在哪个维度上做了什么、在哪个维度上是空白。

这篇我们就用这把尺子，把 2026 年社区里真正有人在用的几个框架——OpenAI Agents SDK、Claude Agent SDK、Mastra、Pi——逐个量一遍。

怎么量：六个问题 + 一条红线

六大支柱对应六个具体的问题，每个框架都过一遍：

Loop：循环谁控制？你能自定义退出条件吗？有没有保险丝？
Tool：工具怎么注册？运行时能动态加载吗？MCP 是原生支持还是 adapter 支持？
Context：有没有压缩机制？有没有 Cache 意识？上下文爆了框架帮你处理吗？
Memory：跨会话记忆是内置还是外挂？有没有向量检索？
Multi-Agent：多 Agent 怎么协作？上下文怎么隔离？有没有深度限制？
Harness：保险丝、权限控制、Generator/Evaluator 这些 Harness 零件，框架有没有内置？还是全要自己搭？
OpenAI Agents SDK：Swarm 的生产版

OpenAI Agents SDK（TypeScript 包 @openai/agents）是 OpenAI 出品的一个 Agent 框架，当然也有一个 Python 的版本，在这个仓库。

无论是 JS 版本还是 Python 版本，核心的概念都是这三个：Agent、Handoff、Guardrail。接下来我先说说这几个核心概念，然后再结合六大支柱来分析。

Agent：一个带指令和工具的 LLM 实例

Agent 是最基础的单元——你给它一段 instructions（系统提示词）、一组 tools、选一个 model，它就能跑起来。

typescript
复制
import { Agent, run } from '@openai/agents';

const agent = new Agent({
  name: 'researcher',
  instructions: '你是一个技术研究助手，帮用户搜索和整理技术资料。',
  model: 'gpt-5.4',
  tools: [searchTool, writeNoteTool],
});

const result = await run(agent, '帮我查一下 LangGraph 的最新进展');
console.log(result.finalOutput);


instructions 可以是静态字符串，也可以是一个函数——接收当前的 RunContext 和 Agent，动态生成系统提示词。这个设计在我们讲 System Prompt 那篇里提过：好的系统提示词应该能根据运行时状态调整，而不是写死一段话。

run() 函数启动 Agent Loop——调模型、有工具调用就执行、没有就结束。返回 RunResult，里面有 finalOutput（最终输出）和完整的消息历史。

Handoff：Agent 之间的控制权转交

Handoff 是 OpenAI Agents SDK 最有特色的设计。一个 Agent 可以声明"我能把对话转交给哪些其他 Agent"：

typescript
复制
const bookingAgent = new Agent({
  name: 'booking',
  instructions: '你负责处理预订相关的请求。',
  tools: [bookFlightTool, bookHotelTool],
});

const refundAgent = new Agent({
  name: 'refund',
  instructions: '你负责处理退款相关的请求。',
  tools: [processRefundTool],
});

const triageAgent = new Agent({
  name: 'triage',
  instructions: '你是客服入口，根据用户的问题判断应该交给谁处理。',
  handoffs: [bookingAgent, refundAgent],  // 声明可以转交给谁
});

const result = await run(triageAgent, '我想退掉上周订的机票');
// triage → 判断是退款 → 调用 transfer_to_refund → refund agent 接手处理


Handoff 在模型看来就是一个特殊的 tool call——transfer_to_<agent_name>。当模型决定转交时，当前 Agent 被替换，循环继续。接手的 Agent 默认能看到完整的对话历史——Triage Agent 跟用户聊的那些消息，Refund Agent 全都能看到。

这跟我们在 Multi-Agent 那篇里讲的 Parent-Child 模式有一个本质区别。Parent-Child 的设计是子 Agent 在独立的上下文里干活，干完把结论压缩回传给父级——Manus 的数据是 10-20 倍压缩比。而 Handoff 是整个对话历史原封不动地传给下一个 Agent，没有压缩、没有隔离。好处是接手的 Agent 有完整的对话上下文，不会遗漏信息；代价是对话越长，每次 Handoff 传递的 token 越多，上下文膨胀没有人管。

还有一个 agent.asTool() 模式——这个就是我们在 Multi-Agent 那篇里讲的 Parent-Child 模式了：父 Agent 保持控制权，子 Agent 作为工具被调用。

Guardrail：Agent 的安全护栏

Guardrail 是在 Agent 执行的边界做验证——分为输入 Guardrail 和输出 Guardrail。

输入 Guardrail 在用户消息进来的时候跑，可以检测恶意输入、越权请求等。里面有一个很实用的设计：输入 Guardrail 可以跟 Agent 并行执行。

Guardrail 和 Agent 的 LLM 调用同时发出，如果 Guardrail 判断不通过，立即终止 Agent。这样正常请求不会有额外延迟，只有被拦截的请求会中断。

而输出 Guardrail 是在 Agent 产出最终回复后跑——检查回复是否合规、是否泄露敏感信息等。

typescript
复制
import { Agent, run } from '@openai/agents';
import { z } from 'zod';

// 用另一个 Agent 当 Guardrail——判断用户是不是在让你做数学作业
const guardrailAgent = new Agent({
  name: 'guardrail',
  instructions: '判断用户是否在要求你帮他做数学作业。',
  outputType: z.object({ isMathHomework: z.boolean() }),
});

const agent = new Agent({
  name: 'assistant',
  instructions: '你是一个客服助手。',
  inputGuardrails: [{
    name: 'math_homework_check',
    execute: async ({ input, context }) => {
      const result = await run(guardrailAgent, input, { context });
      return {
        tripwireTriggered: result.finalOutput?.isMathHomework ?? false,
        outputInfo: result.finalOutput,
      };
    },
  }],
});


tripwireTriggered 返回 true 时，Agent 立即终止并抛出异常。这个例子还展示了一个有意思的模式——用一个专门的 Agent 来做 Guardrail，让模型自己判断输入是否违规。

三个概念讲完了，接下来结合六大支柱来看看 Agents SDK 的能力边界。

六大支柱分析

Loop 层非常标准——就是一个 tool-call 循环，maxTurns 控制轮次。有一个设计挺实用：toolUseBehavior 可以配成 { stopAtToolNames: ['get_weather'] }——指定的工具一旦被调用，结果直接变成最终输出，模型不再继续。在"模型只是用来决定调哪个工具"的场景下，这能省不少 token。

Tool 层用 tool() 函数定义，参数 schema 用 Zod。除了函数工具，还有托管工具（webSearchTool()、codeInterpreterTool() 这些跑在 OpenAI 服务端的）、Agent-as-Tool（agent.asTool()）、MCP 工具。还有一个 toolSearchTool() + deferLoading 机制——工具定义时设 deferLoading: true，运行时模型通过 toolSearchTool() 按需发现和加载。这里的机制跟我们在 Deferred Loading 那篇里讲的思路完全一样。

Context 层几乎没有内置的 Context Engineering 策略，官方 context management 文档 也没有最佳实践的分享。

Memory 层——Session 管了对话历史的持久化，但长期记忆（跨会话的用户画像、知识积累）和向量检索——没有内置，需要你自己来接。

Multi-Agent 层就是前面讲的 Handoff + asTool 两种模式。Handoff 是线性的——一次只能一个 Agent 执行，不支持并行协作。如果你需要多个 Agent 同时工作然后汇总结果，只能用 asTool 模式（模型在一个 turn 里并行调用多个 agent-tool）。

Harness 层，除了前面讲的 Guardrail，保险丝只有 max_turns，没有 token 预算、没有成本上限。其他的 Harness 零件——上下文压缩触发、权限控制、Generator/Evaluator 分离都没有。

整体来说，这个 SDK 封装比较薄，很多的 Agent 支柱都完成度不高，Agent、Handoff、Guardrail 三个核心概念可以了解一下，当做自己知识面的扩充。

Claude Agent SDK：你已经认识它了

这个就不用继续多说了，之前我们讲六大支柱的时候，没有一个绕开了 Claude Code 的底层实现的，而 Claude Agent SDK(@anthropic-ai/claude-agent-sdk) 正是 Claude Code 官方对外的 SDK。

不过 Claude Agent SDK（TypeScript 包）跟其他框架不一样——说它是"框架"其实不太准确。它本质上是把 Claude Code 作为库来调用。

这个 SDK 里面没有 Agent 类。你调用一个 query() 函数，它启动一个完整的 agent loop，返回 AsyncGenerator<SDKMessage> 流式消息流——天然就是我们在流式架构那篇里讲的逐 token 推送模式（有个参数叫 includePartialMessages: true，可以拿到跟 SSE text-delta 同粒度的流式输出）。底层实际上是启动了 Claude Code 的二进制文件作为子进程，通过进程间通信来做消息的同步。

我们整个课程里分析的那些 Claude Code 的工程决策——上下文压缩、CLAUDE.md、权限系统、Hook 机制——全部在 SDK 里可用。它跑的是同一套代码，我们这里可以基于平时常用的一些 Claude Code 参数帮助大家来快速回顾一下：

Loop 层——就是 Claude Code 的循环。maxTurns 参数限制工具调用轮次，maxBudgetUsd 参数设成本上限，effort 参数控制推理深度。这是目前所有框架里唯一同时有轮次保险丝和成本保险丝的。

Tool 层——Claude Code 有 20+ 内置工具，自定义工具通过 tool() 函数定义。MCP 是一等公民——@modelcontextprotocol/sdk 是直接依赖，支持 stdio/sse/http/SDK 内置四种传输方式。还有我们在 Skills 那篇里讲的 Skills 系统——SDK 的子 Agent 也可以配 skills 字段。权限控制做的也很细——我们在权限那篇里讲的四层防线，这里全有：allowedTools / disallowedTools / permissionMode，甚至支持 "Bash(npm *)" 这种模式级规则。

Context 层是五个框架里最完备的。上下文快满时自动触发 compaction(/compact 命令可以手动触发)——我们在压缩那篇里讲的那套机制。通过加载 CLAUDE.md 注入持久化上下文。还有 PreCompact hook 自定义压缩行为。

Memory 层——对话存为 JSONL 文件，支持历史 session 恢复(/resume)、分叉(/fork)。没有做向量检索，基本靠 grep、glob 这种字符串检索。

Multi-Agent 层——通过 agents 选项定义子 Agent，主 Agent 通过内置的 Agent 工具调用它们。每个子 Agent 新的对话、新的上下文，只有最终结果返回给父级。这就是我们在 Multi-Agent 那篇里讲的 Parent-Child 模式的标准实现——10-20 倍的上下文压缩。还能设 background: true 做异步执行。

Harness 层在五个框架里覆盖度最高。Claude Code 保险丝实现的最全面，权限系统也最复杂，而且有 28 种 Hook 事件——PreToolUse、PostToolUse、PermissionRequest、PreCompact 等——几乎是我们在 Hook 那篇里讲的完整实现。Hook 不只是能观察——还能修改工具输入、注入上下文、阻止 Agent 停止。

不过这个 SDK 用的模型锁定在 Claude，不能直接用 GPT 或 Gemini。如果你的场景必须要切换模型提供商，这是一个非常大的限制。

还有一点值得一提：query() 的 session 管理、流式消息推送、中途权限请求——这些跟我们在 ACP 那篇里讲的协议概念高度对齐。Claude Agent SDK 本身就可以看成是 ACP 思想的一种落地实现。

Mastra：TypeScript 领域的全栈选择

Mastra（核心包 @mastra/core，当前 v1.27.0）是做 Gatsby 的那个团队做的，有 YC 融资的背景，2024 年 10 月上线，2026 年 1 月正式发布 1.0。

我们同样会六大支柱来分析这个框架，看看它主要做了哪些工作：

Loop 层——底层建在 Vercel AI SDK 上，agent.generate() 和 agent.stream() 跑的就是 AI SDK 的 tool-call 循环。maxSteps 控制轮次，默认 5。如果你已经在用 Vercel AI SDK，Mastra 的 Loop 层对你来说没什么新东西。

Tool 层——createTool() 用 Zod 定义输入输出 schema。MCP 通过 @mastra/mcp 原生支持。一个有意思的双向设计：Mastra 不光能当 MCP 客户端（用 MCPClient 连接外部 MCP Server 获取工具），还能反过来用 MCPServer 把自己的工具暴露出去。你的 Mastra Agent、工具——Cursor、Claude Desktop 这些支持 MCP 的客户端就能直接连上来调用。比如你用 Mastra 写了个天气查询工具，包成 MCP Server 发到 npm，别人在 Cursor 里 npx your-mcp-server 就能用了。

Context 层——跟 LangGraph 一样是空白。没有压缩、没有 Cache 意识、没有 token 预算。

Memory 层是 Mastra 在所有框架里做得最复杂的。@mastra/memory 不是简单的"存一下对话历史"，它是一个完整的记忆工程模块。

最基础的是 Message History——自动存储和加载最近的对话消息（默认最近 10 条），给 Agent 提供短期上下文。

在这之上有三种高级记忆能力：

Working Memory 是 Agent 的"便签本"——持久化存储用户偏好、目标、任务状态这类需要跨对话记住的结构化信息。支持两种定义方式：markdown 模板（自由格式，替换更新）和 Zod schema（类型安全，增量合并）。还能选择隔离级别：resource scope 跨所有对话共享（适合用户画像），thread scope 按对话隔离，前者适合存一些用户级别的。

Semantic Recall 是基于向量检索的语义记忆——新消息进来时，自动用 embedding 在历史对话里搜语义相似的片段注入上下文。需要配一个向量存储（如 Pinecone）和一个 embedding 模型（比如 OpenAI 的 text-embedding-3-small，就是我们在 RAG 管线那篇里讲的把文本转成高维向量的模型）。

Observational Memory 是最有意思的设计——它在后台跑了两个 Agent：Observer 在消息历史超过阈值（默认 30,000 token）时把原始对话压缩成"观察记录"（通常 5-40 倍压缩比），Reflector 在观察记录本身也太长时（默认 40,000 token）再做一轮反思和浓缩。最终上下文里放的是三层结构：最近的原始消息 + 观察记录 + 反思摘要。

这套设计跟我们前面讲的概念有不少呼应：Working Memory 的定位跟我们在记忆系统那篇里讲的 MEMORY.md（索引 + 记忆文件）非常像——都是跨会话持久化用户偏好和项目上下文；Semantic Recall 就是我们在 RAG 管线那篇里讲的向量检索 + 注入上下文的完整流程；Observational Memory 的思路则更接近我们在上下文压缩那篇里讲的 compaction——把膨胀的对话历史压缩成精炼的摘要，而 Mastra 做了 Observer + Reflector 机制，适配了超长上下文的场景。

Multi-Agent 层用 Supervisor 模式——一个 Agent 当 Supervisor，决定委派给哪个子 Agent。每个子 Agent 的记忆隔离。有 onDelegationStart hook 可以在委派前修改或拒绝。

Harness 层——Agent 层面的 Harness 零件比较少（只有 maxSteps，没有权限控制、没有 Generator/Evaluator）。但 Mastra 还有一个独立的 Workflow 系统——基于图的状态机，用 .then() / .after() / .parallel() / .dountil() 组合步骤。

Workflow 跟 Agent 的区别是：Agent 的下一步由模型决定，Workflow 的下一步由你定义。Workflow 还支持 suspend() 暂停——类似 LangGraph 的 interrupt()，状态序列化后可以恢复。

回想一下我们在 Harness 那篇里讲的定义——Harness 的本质是"对模型局限性的假设"。Workflow 背后的假设就是"模型不擅长严格按照固定流程走"。比如一个报销审批流程：先提交，再主管审批，再财务复核，最后打款。你让纯 Agent 来做，它可能跳过财务复核直接打款，或者审批没过就往下走了。Workflow 把这些步骤钉死——该走哪步就走哪步，该暂停等人审批就暂停。这就是在用确定性的流程约束来弥补模型在"严格遵循规则"上的不可靠——这就是 Harness 在做的事。

Pi：OpenClaw 底层的 Agent 引擎

Pi 是 Mario Zechner 开发的 Agent SDK，可以说是这些框架里面最轻量的一个了。OpenClaw 整个 Agent 系统就是建在它上面的。我们课程里反复提到的 OpenClaw 的 Lane 队列、上下文压缩、工具管线，底层都跑在 Pi 的 Agent Loop 里。

在用六大支柱拆解之前，先说清楚 Pi 的包结构——因为它不是一个包，而是四个包各管一件事：

pi-ai：LLM 通信层。管模型注册、API 调用、流式响应，支持 Anthropic、OpenAI、Google、Azure 等多个 Provider。你可以理解为它是 Pi 的"模型适配器"——所有跟 LLM API 打交道的事都在这里。
pi-agent-core：Agent Loop 层。这是整个架构最薄的一层——只负责"调模型、执行工具、循环"这个核心逻辑，其他什么都不管。
pi-coding-agent：应用层。这一层才开始有具体的功能：内置的编码工具（比如 read、write、edit、bash、grep、find、ls 等等）、Session 管理（对话持久化、分支、压缩）、Extension 系统（生命周期钩子）和 Skills 系统。对话历史的存储和 compaction 就在这一层。
pi-tui：终端 UI 层。Markdown 渲染、代码高亮、编辑器组件——纯 UI，跟 Agent 逻辑无关。

这个分层很关键：pi-agent-core 极其薄，只管循环；pi-ai 只管 LLM 调用；具体的工具、Session、压缩都在 pi-coding-agent 里。OpenClaw 主要依赖前三个包，在它们上面加业务逻辑。

理解了这个结构，先看一个最简单的用法——用 pi-coding-agent 提供的高层 API 创建一个 Agent Session：

typescript
复制
import { createAgentSession, codingTools } from '@mariozechner/pi-coding-agent';
import { getModel } from '@mariozechner/pi-ai';

// 创建 session，指定模型和工具
const { session } = await createAgentSession({
  model: getModel('anthropic', 'claude-sonnet-4-6'),
  thinkingLevel: 'medium',
  tools: codingTools, // [read, bash, edit, write]
});

// 先订阅事件——prompt() 返回 Promise<void>，事件通过 subscribe 回调接收
session.subscribe((event) => {
  if (event.type === 'message_update') {
    process.stdout.write(event.assistantMessageEvent.text ?? '');
  }
  if (event.type === 'tool_execution_end') {
    console.log(`\n工具 ${event.toolName} 执行完毕`);
  }
});

// 发送 prompt，等待执行完成
await session.prompt('帮我看一下当前目录有哪些文件');


createAgentSession 是 pi-coding-agent 提供的上层封装——一行就拿到一个配好了工具、Session 管理和上下文压缩的 Agent。事件通过 subscribe() 监听，prompt() 本身返回 Promise<void>（执行完成时 resolve）。如果你想更底层地控制，可以直接用 pi-agent-core 的 agentLoop()——下面会看到。

Loop 层跟其他框架都不一样——agentLoop() 返回一个 EventStream<AgentEvent, AgentMessage[]>，本质是一个 async generator。内部的流程跟我们之前说的 Agent Loop 没有什么区别，但值得一提的是，内部暴露了各个事件（agent_start、turn_start、message_update、tool_execution_start 等）的 Hook，也就是说，你可以在任意环节注入逻辑。

如果你不用 createAgentSession 这个高层封装，直接用 pi-agent-core 的底层 API 长这样：

typescript
复制
import { agentLoop } from '@mariozechner/pi-agent-core';
import { getModel } from '@mariozechner/pi-ai';

const config = {
  model: getModel('anthropic', 'claude-sonnet-4-6'),
  // 把应用消息转成 LLM 能理解的格式
  convertToLlm: (messages) => messages.filter(m => m.role !== 'custom'),
  // 每次调模型前可以对上下文做变换（压缩、裁剪、注入）
  transformContext: async (messages) => {
    if (estimateTokens(messages) > 100_000) {
      return pruneOldMessages(messages);
    }
    return messages;
  },
};

const context = {
  systemPrompt: '你是一个编码助手。',
  messages: [],
  tools: myTools,
};

// agentLoop 返回 EventStream（async generator），可以用 for await 逐事件消费
const events = agentLoop(
  [{ role: 'user', content: '帮我写一个 hello world' }],
  context,
  config,
);

for await (const event of events) {
  if (event.type === 'message_update') {
    process.stdout.write(event.assistantMessageEvent.text ?? '');
  }
}


两层 API 的区别是：createAgentSession 帮你配好了 convertToLlm、transformContext、工具注册、Session 持久化这些东西，事件通过回调来订阅；而直接用 agentLoop 的话这些全要你自己来，你可以用 for await 逐个事件消费，中间能做的事情就更多了。

这里重点说一下 transformContext 这个钩子——每次调模型前，你可以对上下文做任意变换：压缩、裁剪、注入外部信息。看起来不起眼，但 OpenClaw 的整个上下文管理策略就建在这一个 Hook 上。换句话说，这个 Hook 是你定制 Context Engineering 的命门。

Tool 层用 TypeBox 定义参数 schema（不是 Zod），有 onUpdate 回调在执行过程中推送进度。

Pi 的工具接口很干净——就一个 execute 函数，接收参数返回结果。Pi 不管权限、不管策略、不管循环检测，这些全留给上层应用来做。

那 OpenClaw 是怎么加策略的？用装饰器模式——给 Pi 的每个工具一层一层地包（wrap）。这里说的"装饰器"是设计模式里的那个 Decorator Pattern（输入一个对象，返回同接口的新对象，中间加行为），不是 TypeScript 里 @decorator 那个语法糖，两者不是一回事。

这些装饰器全是 OpenClaw 自己写的，不是 Pi 提供的。比如 wrapToolWithBeforeToolCallHook 这个装饰器：它接收一个 Pi 原生工具，返回一个新工具。新工具的 execute 在真正执行前，先跑一遍 OpenClaw 的 hook 逻辑，再执行工具本身的逻辑。

可以看到，Pi 在这一层的扩展性确实做的很薄，不过也给上层的 Agent（比如 OpenClaw）提供了更大的自定义的空间。

Context 层——框架本身不做压缩，但 transformContext 提供了一个干净的接入点。OpenClaw 在这个钩子上实现了自适应分块压缩（adaptive chunking）、标识符保留、最近 N 轮保护（Context Engineering 那一章都讲过）。

Memory 层——前面包结构里说了，对话持久化在 pi-coding-agent 里。它的 SessionManager 把对话历史存成文件（类似 JSONL），支持 session 创建、恢复、Fork。没有向量检索，也没有语义记忆。

Multi-Agent 层——Pi 本身不内置，基本没做什么，OpenClaw 在上面加了一层 subagents-tool 来实现子 Agent 调用。

Harness 层——Pi 框架不做，留给上层。OpenClaw 在 Pi 上面搭了一整套 Harness——Compaction 策略、循环保险丝、子 Agent 深度限制、工具权限策略等等。

回头看 Pi 的四层包结构，每层的职责边界非常清楚，分层很优雅——循环归循环、工具归工具、UI 归 UI。

横向对比分析

从六大支柱的维度分析完这些框架之后，我们现在更进一步，横向对比一下这些框架的核心差异：

Context 层的断层最明显。 四个框架里只有 Claude Agent SDK 真正做了上下文治理——自动 compaction、CLAUDE.md 注入、PreCompact hook 等等。Mastra、Pi 和 OpenAI Agents SDK 在这层都是空白。

Memory 层 Mastra 一骑绝尘。 三种记忆类型（Working Memory + Semantic Recall + Observational Memory），基本囊括了我们之前介绍的所有 Memory 手段，Claue Code 有 Working Memory，有 Compaction，但缺少了 Semantic Recall，而其他几个框架在这个维度上都只有基础的 Session 存储。

Harness 层的设计哲学完全不同。 OpenAI 只做了 Guardrail（而且只在首尾生效）；Claude Agent SDK 做了最完整的 Hook 体系（28 种事件，能拦截、修改、注入）；Mastra 用 Workflow 状态机做确定性约束；Pi 什么都不做，交给上层来叠加能力。

Multi-Agent 的上下文策略各有不同。 OpenAI Agents SDK 的 Handoff 传递完整历史（不压缩，上下文会膨胀），Claude Agent SDK 和 asTool 模式做独立上下文（10-20x 压缩），Mastra 的 Supervisor 隔离上下文。

回到选型这件事

看完四个框架你会发现，它们各有侧重。有的做得很重——Claude Agent SDK 把整个 coding agent 的工具链、权限系统、上下文压缩全包了进来；Mastra 从 Loop 到 Memory 到 Workflow 到部署全链路覆盖。有的做得很轻——Pi 只给你一个循环和几个钩子，剩下的全由你来搭；OpenAI Agents SDK 给了三个干净的概念，东西并不多。

没有绝对最优的框架。 重的框架帮你省了搭建的时间，但也把它的设计假设绑在了你身上——Claude Agent SDK 假设你用 Claude，Mastra 假设你用 AI SDK 生态。轻的框架不替你做决定，但每一层都得你自己实现——Pi 上面跑 OpenClaw，OpenClaw 自己写了几千行的工具策略、上下文压缩、Session 管理代码。

所以在选框架之前，最重要的事不是"哪个框架最好"，而是搞清楚框架到底帮你做了哪些事，以及有哪些事是你需要自己手动控制的。上面说的六大支柱就是一个很好的参考坐标系——拿来量一遍，哪些框架帮你做了、哪些你得自己来，一目了然。

回到整个课程反复说的那个观点：框架解决的是搭建过程中的体力活，不解决设计过程中的脑力活。工具怎么配、上下文怎么管、Harness 怎么搭——这些设计决策，不管你用不用框架，都是需要你自己拍板的。

走到这里，你也会慢慢明白，这门课教的就是做这些决策的能力。

参考资料
OpenAI Agents SDK（TypeScript）
OpenAI Agents SDK 文档
Claude Agent SDK
Mastra 官方文档
Mastra GitHub
Pi Agent Core
AI Agent 框架对比 2026
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
LangGraph 实战：用图的方式重新理解 Agent Loop
下一篇 · 收尾
结课：从 10 行代码到 Agent 六大支柱


---
## 代码块


```typescript
import { Agent, run } from '@openai/agents';

const agent = new Agent({
  name: 'researcher',
  instructions: '你是一个技术研究助手，帮用户搜索和整理技术资料。',
  model: 'gpt-5.4',
  tools: [searchTool, writeNoteTool],
});

const result = await run(agent, '帮我查一下 LangGraph 的最新进展');
console.log(result.finalOutput);
```


```typescript
const bookingAgent = new Agent({
  name: 'booking',
  instructions: '你负责处理预订相关的请求。',
  tools: [bookFlightTool, bookHotelTool],
});

const refundAgent = new Agent({
  name: 'refund',
  instructions: '你负责处理退款相关的请求。',
  tools: [processRefundTool],
});

const triageAgent = new Agent({
  name: 'triage',
  instructions: '你是客服入口，根据用户的问题判断应该交给谁处理。',
  handoffs: [bookingAgent, refundAgent],  // 声明可以转交给谁
});

const result = await run(triageAgent, '我想退掉上周订的机票');
// triage → 判断是退款 → 调用 transfer_to_refund → refund agent 接手处理
```


```typescript
import { Agent, run } from '@openai/agents';
import { z } from 'zod';

// 用另一个 Agent 当 Guardrail——判断用户是不是在让你做数学作业
const guardrailAgent = new Agent({
  name: 'guardrail',
  instructions: '判断用户是否在要求你帮他做数学作业。',
  outputType: z.object({ isMathHomework: z.boolean() }),
});

const agent = new Agent({
  name: 'assistant',
  instructions: '你是一个客服助手。',
  inputGuardrails: [{
    name: 'math_homework_check',
    execute: async ({ input, context }) => {
      const result = await run(guardrailAgent, input, { context });
      return {
        tripwireTriggered: result.finalOutput?.isMathHomework ?? false,
        outputInfo: result.finalOutput,
      };
    },
  }],
});
```


```typescript
import { createAgentSession, codingTools } from '@mariozechner/pi-coding-agent';
import { getModel } from '@mariozechner/pi-ai';

// 创建 session，指定模型和工具
const { session } = await createAgentSession({
  model: getModel('anthropic', 'claude-sonnet-4-6'),
  thinkingLevel: 'medium',
  tools: codingTools, // [read, bash, edit, write]
});

// 先订阅事件——prompt() 返回 Promise<void>，事件通过 subscribe 回调接收
session.subscribe((event) => {
  if (event.type === 'message_update') {
    process.stdout.write(event.assistantMessageEvent.text ?? '');
  }
  if (event.type === 'tool_execution_end') {
    console.log(`\n工具 ${event.toolName} 执行完毕`);
  }
});

// 发送 prompt，等待执行完成
await session.prompt('帮我看一下当前目录有哪些文件');
```


```typescript
import { agentLoop } from '@mariozechner/pi-agent-core';
import { getModel } from '@mariozechner/pi-ai';

const config = {
  model: getModel('anthropic', 'claude-sonnet-4-6'),
  // 把应用消息转成 LLM 能理解的格式
  convertToLlm: (messages) => messages.filter(m => m.role !== 'custom'),
  // 每次调模型前可以对上下文做变换（压缩、裁剪、注入）
  transformContext: async (messages) => {
    if (estimateTokens(messages) > 100_000) {
      return pruneOldMessages(messages);
    }
    return messages;
  },
};

const context = {
  systemPrompt: '你是一个编码助手。',
  messages: [],
  tools: myTools,
};

// agentLoop 返回 EventStream（async generator），可以用 for await 逐事件消费
const events = agentLoop(
  [{ role: 'user', content: '帮我写一个 hello world' }],
  context,
  config,
);

for await (const event of events) {
  if (event.type === 'message_update') {
    process.stdout.write(event.assistantMessageEvent.text ?? '');
  }
}
```
