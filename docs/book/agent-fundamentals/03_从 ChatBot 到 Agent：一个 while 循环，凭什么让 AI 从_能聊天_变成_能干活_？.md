# 从 ChatBot 到 Agent：一个 while 循环，凭什么让 AI 从"能聊天"变成"能干活"？

课程
吃透 AI Agent 开发
从 ChatBot 到 Agent：一个 while 循环，凭什么让 AI 从"能聊天"变成"能干活"？
从 ChatBot 到 Agent：一个 while 循环，凭什么让 AI 从"能聊天"变成"能干活"？

约 21 分钟

AI 私教已完成
专属 1v1 AI 私教，围绕本节内容深度教学
回到私教
重新学习

你有没有想过一个问题：ChatGPT 和 Claude Code，用的都是前沿的大模型，为什么体验完全不一样？

ChatGPT 你问一句它答一句，聊完就完了。Claude Code 你说一句"帮我重构这个文件"，它自己就开始读代码、跑命令、改文件、测试，中间可能要跑十几轮，最后跟你说"搞定了"。

这两者之间的差距，不在模型能力上，而在谁掌控了循环。

三种形态：谁在开车？

在聊 Agent 之前，我们先把三种形态理清楚。这个分类不是我发明的，Anthropic 在他们的很有名一篇博客 "Building Effective Agents" 里也是这么区分的，我觉得讲得很清楚。

ChatBot：一问一答
人：今天天气怎么样？
AI：今天北京晴，最高温度 25°C。
（结束）


人发一条，AI 回一条。对话是人驱动的，AI 没有自主性。你不说话，它就不动。

这是最基础的形态，也是大部分人对"AI 对话"的理解。

Copilot：AI 建议，你拍板
你在写代码……
AI：（弹出建议）要不要把这个函数提取成一个 hook？
你：Tab（接受）
AI：（修改代码）


Copilot 比 ChatBot 多了一层——它会主动观察你在干什么，然后给出建议。但最终决定权在你手里。你按 Tab 就接受，按 Esc 就拒绝。

Cursor 的 Tab 预测就是典型的 Copilot 模式。它猜你接下来要写什么，但它不会自己去跑命令、改别的文件。

Agent：AI 自己开车，你坐副驾
你：帮我把项目从 Express 迁移到 Hono
AI：（开始思考）好，我需要先看一下现在的路由结构……
AI：（读文件）读取 src/routes/index.ts
AI：（分析）发现 15 个路由，3 个中间件……
AI：（跑命令）pnpm add hono
AI：（改文件）重写 src/routes/index.ts
AI：（跑测试）pnpm test
AI：（发现报错）有两个测试挂了，看看是什么问题……
AI：（修 bug）修复类型不匹配的问题
AI：（再跑测试）全部通过
AI：搞定了，一共改了 8 个文件。


看到区别了吗？

你只说了一句话，AI 自己跑了十几轮。 每一轮它都在做三件事：想一步（我接下来该做什么）、做一步（调用工具执行）、看一步（检查结果，决定下一步）。

这个"想-做-看"的循环，就是 Agent 的核心。

本质区别在一个词：谁掌控循环。

ChatBot：人掌控循环。人不说话，AI 不动。
Copilot：人掌控循环，AI 有建议权。你不按 Tab，建议就被丢弃。
Agent：AI 掌控循环，人有否决权。AI 自己决定下一步做什么，人只在关键节点审批（比如要跑 rm -rf 的时候）。

Anthropic 在官方文章里有一个更精确的说法：Workflow 是 LLM 在预定义的代码路径中被编排，Agent 是 LLM 自己决定流程和工具使用。 关键词是"自己决定"——模型不是被动执行你的指令，而是自主规划和行动。

文档地址在下面，感兴趣的话可以跳过去读一读，当然，也可以借助 Sitor 来读：）

https://www.anthropic.com/engineering/building-effective-agents

Agent 的最小模型：while(true)

如果让你用代码来表达 Agent 的核心逻辑，最简单的版本长这样：

typescript
复制
while (true) {
  const response = await llm.chat(messages)  // 想：让模型决定下一步

  if (response.toolCalls.length === 0) {
    break  // 模型认为任务完成了，没有工具要调
  }

  for (const toolCall of response.toolCalls) {
    const result = await executeTool(toolCall)  // 做：执行工具
    messages.push(result)                        // 看：把结果加入上下文
  }
}


就这么几行。

但别小看这个循环，它有几个很关键的设计决策：

为什么是 while(true) 而不是 for 循环？

因为你不知道需要多少步。让模型把 Express 迁移到 Hono，可能要 5 轮也可能要 50 轮，取决于项目的复杂度和中间遇到什么问题。Agent 必须是开放式的——不预设步数，干完为止。

什么时候停下来？

最简单的判断：模型没有再调用工具了。如果模型的回复里只有文字没有工具调用，说明它认为任务完成了（或者它放弃了）。

observe 环节是最容易被忽视的。

很多人实现 Agent 的时候只关注"让模型调用工具"，但忘了把工具结果反馈给模型。messages.push(result) 这一行看起来不起眼，但没有它，模型就看不到工具执行的结果，后面的决策全是盲猜。

好了，这就是 ReAct 模式——Reasoning（想）+ Acting（做），加上 Observation（看结果）。2022 年的论文提出的概念，到现在已经成了所有 Agent 产品的标准范式。

没有那么多花里胡哨的东西，就这么一个小的认知模型，已经跑在了如今无数的 Agent 产品了。

从 10 行到上千行：真实的 Agent Loop 有多复杂

上面那个简单版本能跑，但离生产环境差了十万八千里。

举个例子，Claude Code 的 Agent Loop 里，核心函数有上千行。同样是一个 while(true)，但里面塞了大量的工程逻辑。

我不会直接贴源码，也坚决避免去贴大量的代码——那样没意义，看起来太累，学习体验也不好。我来讲讲上千行的 Agent Loop 里到底多出了什么，以及为什么需要这些东西。

一轮循环里到底发生了什么

简单版是"想-做-看"三步，Claude Code 的实际流程是这样的：

第一步：准备上下文

在调 API 之前，先检查上下文是不是快爆了。如果快到上限了，触发压缩——先试轻量级的 Snip（删掉老消息），不行就 Microcompact（局部摘要），再不行就 Auto-compact（全局摘要）。

这就是我们上一篇讲的 Context Engineering，在 Agent Loop 里的第一个落地点。

第二步：调模型 API

把消息发给模型 API，流式接收响应。这里有一个很聪明的设计——模型还在说话的时候，已经识别出来的工具调用就开始执行了。

为什么？想象一下，模型说"我要同时读 3 个文件"，如果你等它把 3 个工具调用全说完、再一个个去执行，用户就得干等着。但如果模型刚说出第一个"读文件"的指令，你就立刻去读了，等模型说完第三个的时候，第一个可能已经读完了。

这就是"边说边执行"——流式的不只是文字，工具执行也是流式的。

当然这里有一个前提：只有不冲突的操作才能并发。 读文件可以同时读 3 个（只读不冲突），但改文件必须一个个来（两个修改同时写一个文件就乱了）。这个判断——哪些操作可以并发、哪些必须串行——是 Agent 工具系统里一个很重要的设计点。

第三步：决定是否继续

模型响应回来了。现在要判断：循环要不要继续？

这个判断比你想的复杂得多。不只是"有没有工具调用"这一个条件，Claude Code 有 7 种退出路径：

退出原因	什么时候触发
completed	模型没有调用工具，认为任务完成了
aborted_streaming	流式传输过程中被中断
aborted_tools	工具执行过程中被中断
hook_stopped	Hook 阻止了继续执行
max_turns	超过了最大轮次限制
blocking_limit	上下文太长，API 拒绝了
prompt_too_long	压缩后还是太长，无法恢复

这些退出条件不需要你背，你只需要知道是什么时机就可以，留个印象即可。

每一种退出都需要不同的处理。比如 aborted_streaming 发生的时候，模型可能已经说了一半——已经到达的文字要保留（因为用户已经看到了），但还没执行完的工具调用要丢弃。

第四步：执行工具，收集结果

如果决定继续，就执行所有工具调用，收集结果，把结果塞回消息列表，准备下一轮。

这一步也不简单——工具可能执行失败，失败了怎么给模型反馈？错误信息写得好不好直接影响模型能不能自我纠正。

第五步：处理附加任务

工具执行完了，但在开始下一轮之前，还有一堆"杂活"：消费排队的命令附件、检查 Memory 预取结果、检查 Skill 发现结果、记录已消费的命令……

然后构建下一轮的 State，回到第一步。

状态追踪：Agent 需要"记住"的不止是对话

简单版的 Agent 只维护一个消息列表。但真实的 Agent 需要追踪的东西多得多：

现在是第几轮了？ 用来判断是不是该停了（保险丝）
上一轮为什么选择了继续？ 是正常流转、还是从错误中恢复、还是压缩重试？这个信息对调试极其重要——当 Agent 行为异常的时候，你可以一轮一轮追溯，看看它到底是哪一步的判断出了问题
压缩执行到哪了？ 是否触发过紧急压缩、压缩后 token 降了多少
输出被截断了几次？ 模型说到一半被截断，第一次恢复、第二次恢复、第三次还截断就放弃
有没有挂着的异步任务？ 比如后台在生成工具摘要

这些状态组合在一起，才能让 Agent Loop 在各种异常场景下做出正确的决策。这也是为什么真实的 Agent Loop 会比你想象的复杂——不是核心逻辑复杂，而是异常处理复杂。

实时反馈：Agent 不能是"黑箱"

还有一个很重要的设计理念：Agent 在执行过程中，必须实时把中间结果"吐"出来给用户看。

Agent 跑一个复杂任务可能要几分钟。如果用户看到的是一个空白屏幕转圈圈，然后几分钟后突然蹦出结果——这个体验是不可接受的。

所以好的 Agent Loop 不是"跑完再说"，而是"边跑边说"。模型在思考什么、调了什么工具、工具返回了什么——每一步都实时展示给用户。你在 Claude Code 里看到的那种"模型一边说话、工具一边执行"的效果，就是这个设计的体现。

这个机制在技术上是通过 Generator（生成器） 实现的——函数可以在执行过程中不断"吐出"中间结果，而不是等全部跑完才返回一个最终结果。后面讲流式架构的时候我们会深入这个话题。

手术直播：一个真实任务的完整 trace

理论讲多了容易虚。下面这张图是一个真实场景的完整 trace——让 Agent 给 fetchUser 函数加上重试机制，从头到尾 7 轮，每一轮的 Think / Act / Observe 都在里面。

你只说了一句话，Agent 自己跑了 7 轮。这 7 轮里有几个值得注意的行为模式：

先看后改（第 1-3 轮）。 好的 Agent 不会上来就改代码。它先读了目标文件看现状，然后搜了一下项目里有没有现成的重试工具可以复用，发现有一个 withRetry 函数正好能用。如果模型直接动手写一个新的重试逻辑，不仅多余，还可能跟项目现有的风格不一致。

自我检查（第 5 轮）。 改完代码之后，模型自己意识到漏了 import 语句，主动补上了。这就是 observe 环节的价值——模型看到了修改后的文件状态，发现了遗漏。如果没有 observe，模型根本不知道自己漏了东西。

验证结果（第 6 轮）。 不是改完就走，而是跑测试确认没搞坏别的东西。这一步经常被模型"忘掉"——怎么通过 prompt 设计引导模型养成"改完必测"的习惯，是后面课程的重点之一。

这些行为不是我们硬编码进去的，是模型在 ReAct 循环里自主涌现出来的。当然，模型不是每次都表现这么好。怎么让它稳定地表现好，才是真正的工程挑战。

最小可运行版本

理解了这些之后，我们来写一个 "麻雀虽小五脏俱全" 的 Agent。不是 Claude Code 那么复杂，但核心的设计决策都在：

typescript
复制
import { generateText } from 'ai'
import { anthropic } from '@ai-sdk/anthropic'

const tools = {
  read_file: { /* ... */ },
  write_file: { /* ... */ },
  run_command: { /* ... */ },
}

async function agent(task: string) {
  const messages = [{ role: 'user', content: task }]
  let turnCount = 0
  const maxTurns = 30  // 保险丝：防止无限循环

  while (true) {
    // 保险丝检查
    if (++turnCount > maxTurns) {
      console.log('超过最大轮次，停止')
      break
    }

    // 调模型
    const result = await generateText({
      model: anthropic('claude-sonnet-4-6'),
      messages,
      tools,
      maxSteps: 1,  // 每次只走一步，循环由我们控制
    })

    // 没有工具调用 = 任务完成
    if (result.toolCalls.length === 0) {
      console.log('Agent:', result.text)
      break
    }

    // 执行工具，收集结果
    for (const call of result.toolCalls) {
      console.log(`调用工具: ${call.toolName}(${JSON.stringify(call.args)})`)
      const toolResult = await executeTool(call)
      messages.push(
        { role: 'assistant', content: result.text, toolCalls: [call] },
        { role: 'tool', content: toolResult, toolCallId: call.toolCallId }
      )
    }
  }
}


30 行左右。能跑，但离生产级差了十万八千里。它缺了什么？

没有流式响应（等模型说完才展示）
没有并发工具执行（一个个跑）
没有上下文压缩（跑多了就爆）
没有重试（API 挂了就挂了）
没有权限检查（模型说跑什么就跑什么）
没有死循环检测（模型发疯了没人拦）

这些"缺的东西"，就是后面整个课程要一个个补上的。

每补一个，你就离生产级 Agent 近一步。

为什么是现在

最后聊一个问题：Agent 这个概念又不是新的，为什么 2025-2026 年突然爆发了？

红杉资本（Sequoia Capital，硅谷最顶级的风投之一，投过 Apple、Google、OpenAI）今年初发了一篇文章，用"三层能力"来解释这个趋势，我觉得讲得挺准的：

第一层：知识（2022 年跨过）。 ChatGPT 的出现证明了大模型拥有广泛的知识储备。但光有知识不够——一个知道很多东西但不会推理的模型，只能当问答机器。

第二层：推理（2024 年底跨过）。 OpenAI 的 o1 模型证明了模型可以做多步推理。Agent 不只是调工具，它需要规划——先做什么、后做什么、遇到报错怎么调整。推理能力是 Agent 能"想明白"的前提。

第三层：长期迭代（2025 年跨过）。 Claude Code 这类 Coding Agent 证明了模型可以持续自主工作很长时间——不是回答一个问题就完了，而是连续跑几十轮、持续几分钟甚至更久，直到任务完成。

这三层叠加起来，——知识 + 推理 + 迭代，构成了现在 Agent 能 work 的基础前提。

同时还有几个基础设施层面的变化：

上下文窗口从几千 token 扩展到 100K-1M+，Agent 终于有了足够的"工作记忆"
Function Calling 从"prompt hack"变成了模型原生能力，可靠性大幅提升
MCP 协议 标准化了工具接入，Agent 连接外部世界的门槛降低了
Skills把 Agent 的各个领域的技能进行封装并且分发，让模型的能力再次增强

整个行业的变化如此之快，技术更迭的速度也如此频繁，说明这个技术正在从刀耕火种的年代，迈向工程化的时代，就跟前端从 jQuery 到 Vue/React 一样，这中间蕴藏着巨大的机会。

这不是炒概念，是真的到了该学的时候了。

下一篇

这篇我们建立了对 Agent 的基本认知——它的本质就是一个 while(true) 循环，模型在循环里自主决策。从 ChatBot 到 Agent 的跨越，是控制权的转移。

但要真正理解 Agent 的很多设计决策——为什么要做上下文压缩、为什么 KV Cache 命中率这么重要、为什么约束解码能控制模型行为——你需要对大模型底层的工作原理有一些了解。

不讲论文、不推公式，只讲跟 Agent 开发直接相关的几个概念——Token 化、自回归生成、KV Cache、约束解码。这些东西不理解的话，后面的课你会看得很吃力。

下一篇，我们继续聊聊。

检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
搞定 Agent 六大支柱：今天出个 Manus 明天出个 OpenClaw，你到底应该学什么？
下一篇 · 第一章：认知校准
做 Agent 开发，有些大模型本身的底层机制，你不得不了解


---
## 代码块


```
人：今天天气怎么样？
AI：今天北京晴，最高温度 25°C。
（结束）
```


```
你在写代码……
AI：（弹出建议）要不要把这个函数提取成一个 hook？
你：Tab（接受）
AI：（修改代码）
```


```
你：帮我把项目从 Express 迁移到 Hono
AI：（开始思考）好，我需要先看一下现在的路由结构……
AI：（读文件）读取 src/routes/index.ts
AI：（分析）发现 15 个路由，3 个中间件……
AI：（跑命令）pnpm add hono
AI：（改文件）重写 src/routes/index.ts
AI：（跑测试）pnpm test
AI：（发现报错）有两个测试挂了，看看是什么问题……
AI：（修 bug）修复类型不匹配的问题
AI：（再跑测试）全部通过
AI：搞定了，一共改了 8 个文件。
```


```typescript
while (true) {
  const response = await llm.chat(messages)  // 想：让模型决定下一步

  if (response.toolCalls.length === 0) {
    break  // 模型认为任务完成了，没有工具要调
  }

  for (const toolCall of response.toolCalls) {
    const result = await executeTool(toolCall)  // 做：执行工具
    messages.push(result)                        // 看：把结果加入上下文
  }
}
```


```typescript
import { generateText } from 'ai'
import { anthropic } from '@ai-sdk/anthropic'

const tools = {
  read_file: { /* ... */ },
  write_file: { /* ... */ },
  run_command: { /* ... */ },
}

async function agent(task: string) {
  const messages = [{ role: 'user', content: task }]
  let turnCount = 0
  const maxTurns = 30  // 保险丝：防止无限循环

  while (true) {
    // 保险丝检查
    if (++turnCount > maxTurns) {
      console.log('超过最大轮次，停止')
      break
    }

    // 调模型
    const result = await generateText({
      model: anthropic('claude-sonnet-4-6'),
      messages,
      tools,
      maxSteps: 1,  // 每次只走一步，循环由我们控制
    })

    // 没有工具调用 = 任务完成
    if (result.toolCalls.length === 0) {
      console.log('Agent:', result.text)
      break
    }

    // 执行工具，收集结果
    for (const call of result.toolCalls) {
      console.log(`调用工具: ${call.toolName}(${JSON.stringify(call.args)})`)
      const toolResult = await executeTool(call)
      messages.push(
        { role: 'assistant', content: result.text, toolCalls: [call] },
        { role: 'tool', content: toolResult, toolCallId: call.toolCallId }
      )
    }
  }
}
```
