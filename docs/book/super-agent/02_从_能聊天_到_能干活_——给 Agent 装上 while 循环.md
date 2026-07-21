# 从"能聊天"到"能干活"——给 Agent 装上 while 循环

课程
Super Agent 实战课
从"能聊天"到"能干活"——给 Agent 装上 while 循环
从"能聊天"到"能干活"——给 Agent 装上 while 循环

约 34 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

你好，欢迎来到三元的 Agent 实战课堂。

上一篇结束的时候，你已经有了一个能在终端里聊天的 AI。你说一句，它回一句，还能记住你前面说了什么。

但你试过让它干点实际的事吗？

You: 今天北京天气怎么样？
Assistant: 北京今天天气晴朗，气温大约在 15-25 度之间……


听起来像那么回事？但这完全是编的。它没有查任何天气接口，只是根据"北京""天气"这些词，编了一个看上去合理的回复。

再试一个：

You: 帮我读一下 package.json 的内容
Assistant: 你可以使用 cat package.json 命令来查看文件内容……


它在教你怎么做，而不是帮你做。

这就是 ChatBot 和 Agent 的本质区别——ChatBot 只能说，Agent 能做。

这篇我们来完成这个跨越。改造完之后，同样的问题：

You: 今天北京天气怎么样？
Agent: [调用 get_weather 工具] → 北京今天晴，15-25°C，东南风 2 级。


它不再编答案了，而是去查、去做、再回复。

一个循环的差距

上一篇的 ask() 函数是这样工作的：

用户输入 → 发给模型 → 拿到文本回复 → 显示 → 等下一轮输入


模型只有一次机会——收到消息，生成回复，结束。它没有机会"中途去做点什么再回来继续"。

Agent 的工作方式不一样：

用户输入 → 发给模型 → 模型说"我要调 get_weather" 
→ 执行 get_weather → 把结果告诉模型 
→ 模型继续生成最终回复 → 显示


模型在生成回复的过程中，可以决定"我需要先调用一个工具"。工具执行完之后，结果会被喂回给模型，模型拿到实际数据再生成最终回复。

这就是 Agent Loop 的核心思想：模型不止跑一次，而是跑一个循环——想、做、看结果，然后决定是继续做还是给出最终回答。

这个模式有个经典的名字：think → act → observe。

Think：模型分析当前情况，决定下一步做什么
Act：如果需要，调用工具执行操作
Observe：拿到工具返回的结果
然后回到 Think，直到模型认为可以给出最终回答

Claude Code、Cursor、Manus……所有你能叫得上名字的 AI Agent 产品，底层都是这个循环。区别只在循环里塞了多少东西——错误处理、并发控制、上下文压缩、安全检查。但骨架就是这么简单。

你可能会问：既然这么简单，为什么这门课要花 20 多篇来讲？

因为骨架简单不代表肌肉简单。一个裸的 while 循环跑起来确实只要几十行代码，但它什么保护都没有——模型可能兜圈子烧穿你的 token 额度，API 随时可能超时，上下文窗口迟早会塞满。后面每一篇课程，本质上都在回答同一个问题：怎么让这个 while 循环在生产环境里稳定运行。

但那是后面的事。这篇只需要让循环先跑起来。

先从定义一个工具开始

在动手改循环之前，我们先给 Agent 一个能力——一个假的天气查询工具。

一个工具由三样东西组成：

description：告诉模型这个工具是干什么的（模型靠这个判断什么时候该调它）
inputSchema：工具接受什么参数（用 JSON Schema 定义）
execute：实际执行函数
src/tools/utility-tools.ts
应用
复制
import { jsonSchema } from 'ai';

export const weatherTool = {
  description: '查询指定城市的天气信息',
  inputSchema: jsonSchema({
    type: 'object',
    properties: {
      city: { type: 'string', description: '城市名称，如"北京"、"上海"' },
    },
    required: ['city'],
    additionalProperties: false,
  }),
  execute: async ({ city }: { city: string }) => {
    // 先用假数据，后面课程会接真实 API
    const mockWeather: Record<string, string> = {
      '北京': '晴，15-25°C，东南风 2 级',
      '上海': '多云，18-22°C，西南风 3 级',
      '深圳': '阵雨，22-28°C，南风 2 级',
    };
    return mockWeather[city] || `${city}：暂无数据`;
  },
};

export const calculatorTool = {
  description: '计算数学表达式的结果。当用户提问涉及数学运算时使用',
  inputSchema: jsonSchema({
    type: 'object',
    properties: {
      expression: { type: 'string', description: '数学表达式，如 "2 + 3 * 4"' },
    },
    required: ['expression'],
    additionalProperties: false,
  }),
  execute: async ({ expression }: { expression: string }) => {
    try {
      // 生产环境不要用 eval，这里纯粹为了演示
      const result = new Function(`return ${expression}`)();
      return `${expression} = ${result}`;
    } catch {
      return `无法计算: ${expression}`;
    }
  },
};


看一下这个结构。每个工具由三部分组成：

description 不是给人看的，是给模型看的——模型通过这段描述来判断"用户问天气的时候，我应该调这个工具"。描述写得越准确，模型调用的时机就越精准。

inputSchema 用 jsonSchema() 定义工具的参数结构——本质就是一段 JSON Schema。AI SDK 会把它跟 description 一起塞进请求发给模型。模型看到的东西大概长这样：

json
复制
{
  "type": "function",
  "function": {
    "name": "get_weather",
    "description": "查询指定城市的天气信息",
    "parameters": {
      "type": "object",
      "properties": {
        "city": { "type": "string", "description": "城市名称" }
      },
      "required": ["city"]
    }
  }
}


模型根据这个描述来决定什么时候调这个工具、传什么参数。如果参数格式不对，SDK 会在执行之前拦截。

这里面有个很重要的直觉：工具的 description 和 inputSchema 里的属性 description，本质上就是在写 prompt。 你写得越清楚、越具体，模型调用的准确率就越高。"查天气"不如"查询指定城市的实时天气信息，包括温度、风向等"。

execute 就是一个普通的 async 函数。模型决定调用工具时，SDK 会自动用模型返回的参数调用 execute，然后把返回值序列化成字符串，作为 tool-result 消息塞回对话历史里。

两个工具就够了，足以验证 Agent Loop 能工作。

把工具接进 streamText

在上一篇的代码里，streamText 只传了 model 和 messages。现在加上 tools：

typescript
复制
import { streamText, type ModelMessage } from 'ai';
import { weatherTool, calculatorTool } from './tools.js';

const tools = { get_weather: weatherTool, calculator: calculatorTool };


tools 是一个对象，key 是工具名（模型在调用时会引用这个名字），value 是工具定义对象。

但只加 tools 还不够。回想一下上一篇的代码——streamText 返回的 textStream 只包含文本 chunk。如果模型决定调用工具而不是直接回复文本，textStream 里什么都没有。

我们需要用 fullStream 来替代 textStream。

textStream 只给你文本片段，是上一篇够用的简化接口。但现在模型除了文本，还可能返回工具调用——textStream 会把这些全部丢掉。fullStream 包含完整的事件流，每个事件都有 type 字段告诉你发生了什么：

text-delta：文本片段（跟 textStream 一样）
tool-call：模型决定调用某个工具，包含工具名和参数
tool-result：工具执行完毕，包含返回值
step-start / step-finish：每一步的开始和结束
finish：所有步骤都完成了

你在 for await 里通过 switch(part.type) 来分别处理每种事件。

这也引出了一个关键问题：谁来执行工具？

两种方式：SDK 自动循环 vs 手动循环

Vercel AI SDK 提供了一个很方便的能力——自动多步执行。当模型返回工具调用时，SDK 会自动执行工具、把结果喂回模型、让模型继续生成，直到模型不再调用工具为止。

控制这个行为的参数叫 stopWhen：

typescript
复制
import { streamText, stepCountIs } from 'ai';

const result = streamText({
  model,
  tools,
  messages,
  stopWhen: stepCountIs(5), // 最多跑 5 步
});


stepCountIs(5) 的意思是：模型最多可以进行 5 轮"思考→调用工具→拿到结果→继续"的循环。如果 5 步之后模型还在调用工具，强制停止。你也可以传自定义的停止条件函数，不限于步数。

这很好用，但有个问题：它把循环藏起来了。

对于课程来说，我们需要看到循环在干什么。而且在生产级 Agent 里，你几乎一定会自己控制这个循环——因为你需要在每一步之间做很多事：打日志、检查 token 用量、判断是不是陷入死循环、决定要不要中断。

所以我们两个都要学。先用 SDK 的自动循环快速跑通，再手动实现一遍，理解底层发生了什么。

方式一：SDK 自动循环

改造 ask() 函数。核心变化：用 fullStream 替代 textStream，加上 tools 和 stopWhen。

src/index.ts
应用
复制
import { streamText, stepCountIs, type ModelMessage } from 'ai';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';

// ... model 定义同上一篇 ...

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];

const rl = createInterface({ input: process.stdin, output: process.stdout });

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    const result = streamText({
      model,
      system: '你是 Super Agent，一个有工具调用能力的 AI 助手。需要时主动使用工具获取信息，不要编造数据。',
      tools,
      messages,
      stopWhen: stepCountIs(5),
    });

    process.stdout.write('Assistant: ');
    let fullResponse = '';

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullResponse += part.text;
          break;
        case 'tool-call':
          console.log(`\n  [调用工具: ${part.toolName}(${JSON.stringify(part.input)})]`);
          break;
        case 'tool-result':
          console.log(`  [工具返回: ${JSON.stringify(part.output)}]`);
          break;
      }
    }

    console.log(); // 换行
    messages.push({ role: 'assistant', content: fullResponse });

    ask();
  });
}

console.log('Super Agent v0.2 — Agent Loop (type "exit" to quit)\n');
ask();


先安装依赖：

bash
运行
复制
pnpm install


运行起来：

bash
运行
复制
pnpm start


❤️ 温馨提示：右侧 WebContainer 终端对退格（Backspace）键的支持有限，输入时建议一次打完再回车。本地终端没有这个问题。

You: 北京今天天气怎么样？
  [调用工具: get_weather({"city":"北京"})]
  [工具返回: "晴，15-25°C，东南风 2 级"]

北京今天天气晴朗，气温 15-25°C，东南风 2 级。

You: 15 加 28 等于多少？
  [调用工具: calculator({"expression":"15 + 28"})]
  [工具返回: "15 + 28 = 43"]
15 加 28 等于 43。


它真的去调工具了，而不是编答案。

这里有个很微妙的变化你可能没注意到——我们没有写任何循环代码。streamText 加了 tools 和 stopWhen 之后，SDK 内部会自动循环。模型说"我要调 get_weather"，SDK 执行工具，把结果塞回模型，模型拿到真实数据生成最终回复。这一切发生在 fullStream 的迭代过程中，对你来说就是一个 for-await 循环。

方便，但定制性太差——你没法在步骤之间插入自己的逻辑。打日志、追踪 token、检测死循环、中断执行……这些全都做不了，因为循环被 SDK 藏起来了。

生产级 Agent 几乎都自己控制循环。接下来我们自己实现。

方式二：手动 Agent Loop

现在我们自己来实现这个循环。不用 stopWhen，不靠 SDK 的自动多步——自己写 while。

为什么要手动实现？因为在真实的 Agent 产品里，你需要在循环的每一步做很多额外的事情：

打印当前是第几步（调试用）
追踪累计 token 消耗（成本控制）
检测是否陷入重复调用（防死循环）
决定某个工具是否需要用户确认才能执行（安全控制）
在内存快满的时候做上下文压缩

这些都需要你掌控循环本身。

src/agent/loop.ts
应用
复制
import { streamText, type ModelMessage } from 'ai';

const MAX_STEPS = 10;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    const result = streamText({
      model,
      system,
      tools,
      messages,
      // 不设 stopWhen，每次只跑一步
    });

    let hasToolCall = false;
    let fullText = '';

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullText += part.text;
          break;

        case 'tool-call':
          hasToolCall = true;
          console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);
          break;

        case 'tool-result':
          console.log(`  [结果: ${JSON.stringify(part.output)}]`);
          break;
      }
    }

    // 拿到这一步的完整结果，追加到消息历史
    const stepMessages = await result.response;
    messages.push(...stepMessages.messages);

    // 退出条件：模型没有调用任何工具，说明它认为可以直接回复了
    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    // 还有工具调用 → 继续循环，让模型看到工具结果后继续思考
    console.log('  → 模型还在工作，继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}


看这个 while 循环的结构：

调一次 streamText，不设 stopWhen（默认只跑一步）
遍历 fullStream，收集文本和工具调用
把这一步的消息追加到 messages
判断退出条件：如果这一步没有工具调用，说明模型直接给出了文本回复，循环结束
如果有工具调用，回到步骤 1，模型会看到工具的执行结果，决定下一步做什么

退出条件是整个循环最关键的设计决策。当前我们用的是最简单的策略：模型不再调用工具 → 停止。这在大多数场景下够用了——模型调完该调的工具、拿到了需要的信息，自然会切换到生成文本回复。

但生产环境里，退出条件会复杂得多：

步数上限：防止模型陷入无限循环（我们这里的 MAX_STEPS）
Token 预算：累计输出超过阈值就强制停止
重复检测：连续调用同一个工具、传同样的参数——明显是在兜圈子
用户中断：AbortSignal 随时可以打断

这些会在后面的"保险丝"那篇详细展开。

实际上像 Claude Code 这样的生产级 Agent，退出路径有 7 种之多——用户中断、token 预算耗尽、步数上限、模型主动结束、API 错误、超时、权限被拒。我们当前只实现了最基础的两种（模型主动结束 + 步数上限），但已经足以让 Agent 正常工作了。

result.response 里有什么

你可能注意到了这行代码：

typescript
复制
const stepMessages = await result.response;
messages.push(...stepMessages.messages);


result.response 是一个 Promise，resolve 之后包含这一步的完整信息。其中 messages 是一个数组，包含模型在这一步里产生的所有消息——文本回复和/或工具调用+工具结果。

把它们追加到 messages 数组后，下一次循环调用 streamText 时，模型就能看到"我刚才调了什么工具、结果是什么"，然后决定下一步做什么。

这就是 Agent 的"记忆"在循环内的运作方式——不是什么黑魔法，就是把每一步的对话记录完整地传回给模型。

把手动循环接入对话

改造 ask() 函数，用 agentLoop 替代原来的 streamText：

src/index.ts
应用
复制
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';
import { agentLoop } from './agent-loop.js';

// ... model 定义同上一篇 ...

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];
const rl = createInterface({ input: process.stdin, output: process.stdout });

const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
需要查询信息时，主动使用工具，不要编造数据。
回答要简洁直接。`;

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    await agentLoop(model, tools, messages, SYSTEM);

    ask();
  });
}

console.log('Super Agent v0.2 — Agent Loop (type "exit" to quit)\n');
ask();


应用代码后运行：

bash
运行
复制
pnpm start


试一个需要多步的问题：

You: 北京和上海今天哪个更热？

--- Step 1 ---
  [调用: get_weather({"city":"北京"})]
  [结果: "晴，15-25°C，东南风 2 级"]
  [调用: get_weather({"city":"上海"})]
  [结果: "多云，18-22°C，西南风 3 级"]
  → 模型还在工作，继续下一步...

--- Step 2 ---
Assistant: 对比两个城市今天的天气：
- 北京：15-25°C
- 上海：18-22°C

北京的最高温更高（25°C vs 22°C），所以今天北京更热一些。


注意 Step 1 里模型一次调了两个工具（并发）——它知道需要两个城市的数据才能回答。拿到两个结果后，Step 2 直接给出对比分析，没有再调工具，循环结束。

这就是 Agent 和 ChatBot 的本质区别。 ChatBot 一步到位、直接编答案。Agent 会规划——"我需要什么数据 → 去获取 → 拿到之后再分析"。模型自己决定调什么工具、调几次、什么时候停。

再试一个需要组合多个工具的问题：

You: 北京今天最高温 25 度，上海最高温 22 度，温差是多少？

--- Step 1 ---
  [调用: calculator({"expression":"25 - 22"})]
  [结果: "25 - 22 = 3"]
  → 模型还在工作，继续下一步...

--- Step 2 ---
Assistant: 北京和上海今天的最高温温差是 3°C。


这个例子里模型做了一个判断：用户已经给了温度数据，不需要再查天气，直接用计算器算温差就行。它没有多余地调用 get_weather——这就是"think"阶段在起作用。模型不是机械地执行预定义流程，而是根据上下文灵活决定下一步做什么。

这种灵活性是 Agent 区别于传统工作流（workflow）的根本差异。工作流是写死的流程图——步骤 1 做什么、步骤 2 做什么。Agent 每一步都在重新评估"当前的目标是什么、我已经有哪些信息、还需要做什么"。这也是为什么 Agent 适合处理那些你没法提前把流程全部想清楚的任务。

到底发生了什么

回顾一下这篇做了什么。我们把上一篇的 ChatBot 改成了 Agent，代码层面的变化其实不大：

定义了工具（description + inputSchema + execute）
streamText 加了 tools 参数
用 fullStream 替代 textStream，处理工具调用事件
加了一个 while 循环，让模型能够多步执行

但行为上的变化是质的——AI 从"只会说"变成了"能做"。

我们可以来对比一下改造前后的代码结构差异，从单次调用变成了循环调用，从只处理文本变成了处理多种事件类型。实际上就是加了一层"决策"——模型每一轮都可以选择"继续调工具"还是"直接回复"。

这个 while 循环就是 Agent 的心脏。后面所有课程的内容——保险丝、工具系统、上下文管理、流式响应——都是在这个循环里面或者围绕这个循环做文章。

你可以把它想象成一个操作系统的主循环——CPU 不断地取指令、执行、写回结果、再取下一条指令。Agent Loop 也是一样：取模型输出 → 如果是工具调用就执行 → 把结果写回 messages → 再调模型。CPU 的退出条件是关机指令，Agent 的退出条件是模型决定"我已经有足够的信息了，可以回复用户了"。

下一篇预告

现在你的 Agent 能调工具了，但它很脆弱——如果模型反复调同一个工具（死循环），token 会被快速烧完；如果 API 超时或报错，整个程序直接崩溃。

下一篇我们给 Agent 装上三个"保险丝"：

循环检测：连续调用相同工具 + 相同参数？打断它
Token 预算：烧了多少钱，心里得有数
API 容错：超时重试、错误降级、Provider 切换

让 Agent 从"能跑"变成"跑不挂"。

参考资料
Vercel AI SDK — Tool Calling
Vercel AI SDK — Multi-Step Calls
Anthropic — Tool Use
上一篇
10 分钟，让你的 AI 开口说话
下一篇 · 第一章：起步 + Agent Loop
Agent 不能这么脆——循环检测、API 容错与 Token 预算
编辑器


---
## 代码块


```
You: 今天北京天气怎么样？
Assistant: 北京今天天气晴朗，气温大约在 15-25 度之间……
```


```
You: 帮我读一下 package.json 的内容
Assistant: 你可以使用 cat package.json 命令来查看文件内容……
```


```
You: 今天北京天气怎么样？
Agent: [调用 get_weather 工具] → 北京今天晴，15-25°C，东南风 2 级。
```


```
用户输入 → 发给模型 → 拿到文本回复 → 显示 → 等下一轮输入
```


```
用户输入 → 发给模型 → 模型说"我要调 get_weather" 
→ 执行 get_weather → 把结果告诉模型 
→ 模型继续生成最终回复 → 显示
```


```typescript
import { jsonSchema } from 'ai';

export const weatherTool = {
  description: '查询指定城市的天气信息',
  inputSchema: jsonSchema({
    type: 'object',
    properties: {
      city: { type: 'string', description: '城市名称，如"北京"、"上海"' },
    },
    required: ['city'],
    additionalProperties: false,
  }),
  execute: async ({ city }: { city: string }) => {
    // 先用假数据，后面课程会接真实 API
    const mockWeather: Record<string, string> = {
      '北京': '晴，15-25°C，东南风 2 级',
      '上海': '多云，18-22°C，西南风 3 级',
      '深圳': '阵雨，22-28°C，南风 2 级',
    };
    return mockWeather[city] || `${city}：暂无数据`;
  },
};

export const calculatorTool = {
  description: '计算数学表达式的结果。当用户提问涉及数学运算时使用',
  inputSchema: jsonSchema({
    type: 'object',
    properties: {
      expression: { type: 'string', description: '数学表达式，如 "2 + 3 * 4"' },
    },
    required: ['expression'],
    additionalProperties: false,
  }),
  execute: async ({ expression }: { expression: string }) => {
    try {
      // 生产环境不要用 eval，这里纯粹为了演示
      const result = new Function(`return ${expression}`)();
      return `${expression} = ${result}`;
    } catch {
      return `无法计算: ${expression}`;
    }
  },
};
```


```json
{
  "type": "function",
  "function": {
    "name": "get_weather",
    "description": "查询指定城市的天气信息",
    "parameters": {
      "type": "object",
      "properties": {
        "city": { "type": "string", "description": "城市名称" }
      },
      "required": ["city"]
    }
  }
}
```


```typescript
import { streamText, type ModelMessage } from 'ai';
import { weatherTool, calculatorTool } from './tools.js';

const tools = { get_weather: weatherTool, calculator: calculatorTool };
```


```typescript
import { streamText, stepCountIs } from 'ai';

const result = streamText({
  model,
  tools,
  messages,
  stopWhen: stepCountIs(5), // 最多跑 5 步
});
```


```typescript
import { streamText, stepCountIs, type ModelMessage } from 'ai';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';

// ... model 定义同上一篇 ...

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];

const rl = createInterface({ input: process.stdin, output: process.stdout });

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    const result = streamText({
      model,
      system: '你是 Super Agent，一个有工具调用能力的 AI 助手。需要时主动使用工具获取信息，不要编造数据。',
      tools,
      messages,
      stopWhen: stepCountIs(5),
    });

    process.stdout.write('Assistant: ');
    let fullResponse = '';

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullResponse += part.text;
          break;
        case 'tool-call':
          console.log(`\n  [调用工具: ${part.toolName}(${JSON.stringify(part.input)})]`);
          break;
        case 'tool-result':
          console.log(`  [工具返回: ${JSON.stringify(part.output)}]`);
          break;
      }
    }

    console.log(); // 换行
    messages.push({ role: 'assistant', content: fullResponse });

    ask();
  });
}

console.log('Super Agent v0.2 — Agent Loop (type "exit" to quit)\n');
ask();
```


```bash
pnpm install
```


```bash
pnpm start
```


```
You: 北京今天天气怎么样？
  [调用工具: get_weather({"city":"北京"})]
  [工具返回: "晴，15-25°C，东南风 2 级"]

北京今天天气晴朗，气温 15-25°C，东南风 2 级。

You: 15 加 28 等于多少？
  [调用工具: calculator({"expression":"15 + 28"})]
  [工具返回: "15 + 28 = 43"]
15 加 28 等于 43。
```


```typescript
import { streamText, type ModelMessage } from 'ai';

const MAX_STEPS = 10;

export async function agentLoop(
  model: any,
  tools: any,
  messages: ModelMessage[],
  system: string,
) {
  let step = 0;

  while (step < MAX_STEPS) {
    step++;
    console.log(`\n--- Step ${step} ---`);

    const result = streamText({
      model,
      system,
      tools,
      messages,
      // 不设 stopWhen，每次只跑一步
    });

    let hasToolCall = false;
    let fullText = '';

    for await (const part of result.fullStream) {
      switch (part.type) {
        case 'text-delta':
          process.stdout.write(part.text);
          fullText += part.text;
          break;

        case 'tool-call':
          hasToolCall = true;
          console.log(`  [调用: ${part.toolName}(${JSON.stringify(part.input)})]`);
          break;

        case 'tool-result':
          console.log(`  [结果: ${JSON.stringify(part.output)}]`);
          break;
      }
    }

    // 拿到这一步的完整结果，追加到消息历史
    const stepMessages = await result.response;
    messages.push(...stepMessages.messages);

    // 退出条件：模型没有调用任何工具，说明它认为可以直接回复了
    if (!hasToolCall) {
      if (fullText) console.log();
      break;
    }

    // 还有工具调用 → 继续循环，让模型看到工具结果后继续思考
    console.log('  → 模型还在工作，继续下一步...');
  }

  if (step >= MAX_STEPS) {
    console.log('\n[达到最大步数限制，强制停止]');
  }
}
```


```typescript
const stepMessages = await result.response;
messages.push(...stepMessages.messages);
```


```typescript
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { weatherTool, calculatorTool } from './tools.js';
import { agentLoop } from './agent-loop.js';

// ... model 定义同上一篇 ...

const tools = { get_weather: weatherTool, calculator: calculatorTool };
const messages: ModelMessage[] = [];
const rl = createInterface({ input: process.stdin, output: process.stdout });

const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
需要查询信息时，主动使用工具，不要编造数据。
回答要简洁直接。`;

function ask() {
  rl.question('\nYou: ', async (input) => {
    const trimmed = input.trim();
    if (!trimmed || trimmed === 'exit') {
      console.log('Bye!');
      rl.close();
      return;
    }

    messages.push({ role: 'user', content: trimmed });

    await agentLoop(model, tools, messages, SYSTEM);

    ask();
  });
}

console.log('Super Agent v0.2 — Agent Loop (type "exit" to quit)\n');
ask();
```


```bash
pnpm start
```


```
You: 北京和上海今天哪个更热？

--- Step 1 ---
  [调用: get_weather({"city":"北京"})]
  [结果: "晴，15-25°C，东南风 2 级"]
  [调用: get_weather({"city":"上海"})]
  [结果: "多云，18-22°C，西南风 3 级"]
  → 模型还在工作，继续下一步...

--- Step 2 ---
Assistant: 对比两个城市今天的天气：
- 北京：15-25°C
- 上海：18-22°C

北京的最高温更高（25°C vs 22°C），所以今天北京更热一些。
```


```
You: 北京今天最高温 25 度，上海最高温 22 度，温差是多少？

--- Step 1 ---
  [调用: calculator({"expression":"25 - 22"})]
  [结果: "25 - 22 = 3"]
  → 模型还在工作，继续下一步...

--- Step 2 ---
Assistant: 北京和上海今天的最高温温差是 3°C。
```
