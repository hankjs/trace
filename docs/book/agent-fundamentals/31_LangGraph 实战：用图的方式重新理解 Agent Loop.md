# LangGraph 实战：用图的方式重新理解 Agent Loop

课程
吃透 AI Agent 开发
LangGraph 实战：用图的方式重新理解 Agent Loop
LangGraph 实战：用图的方式重新理解 Agent Loop

约 59 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

本节可在右侧编辑器中实时编码运行。默认使用 Mock 模型，在 .env 中填入 DASHSCOPE_API_KEY 可切换到真实 Qwen 模型。API Key 从阿里云百炼平台获取。

你已经手写过完整的 Agent Loop——while(true) 里面调模型、判断工具、执行、观察、循环。你也搭过 Harness，知道保险丝、上下文压缩、权限控制这些零件怎么拼。

现在问一个问题：同样的 Agent，用 LangGraph 写会是什么样？

这不是一道"框架好不好"的辩论题。这篇的目的是：你带着前面建立的知识体系，去看 LangGraph 到底在做什么，它帮你省了哪些事、藏了哪些坑、以及在哪些地方根本没帮你。

我们会从零搭一个完整的项目：Research Agent——一个能搜索资料、自动写笔记、支持人工审核的研究助手。六个 Step，从最简单的预构建 Agent 开始，一步步加上自定义状态、流式输出、保险丝、人工审核，最终拿到一个有完整功能的 LangGraph 应用。跟着走一遍，你就能把 LangGraph 的核心能力摸透。

Step 1：Research Agent 最简版——感受图的基本结构

装依赖：

bash
运行
复制
pnpm install


先用 LangGraph 的预构建方法 createReactAgent 跑通最基本的功能：

src/index.ts
应用
复制
import 'dotenv/config';
import { createReactAgent } from "@langchain/langgraph/prebuilt";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { z } from "zod";

const searchTool = tool(
  async ({ query }) => {
    const results: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心概念是 StateGraph，用节点和边来描述 Agent 的执行流程。",
      "Agent Loop": "Agent Loop 是 AI Agent 的核心运行机制，本质是 while(true) { think → act → observe } 循环。",
    };
    const key = Object.keys(results).find(k => query.includes(k));
    return key ? results[key] : `没有找到关于「${query}」的信息`;
  },
  {
    name: "search",
    description: "搜索技术资料",
    schema: z.object({ query: z.string().describe("搜索关键词") }),
  }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 笔记已保存：${title}\n---\n${content}\n---`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "把研究结果写成笔记保存下来",
    schema: z.object({
      title: z.string().describe("笔记标题"),
      content: z.string().describe("笔记内容"),
    }),
  }
);

const agent = createReactAgent({
  llm: createModel(),
  tools: [searchTool, writeNoteTool],
});

async function main() {
  const result = await agent.invoke({
    messages: [{ role: "user", content: "帮我搜索一下 LangGraph 的资料，然后写一篇笔记总结" }],
  });
  console.log("\n最终回复:", result.messages.at(-1)?.content);
}
main().catch(console.error);

bash
运行
复制
pnpm start


跑起来了。模型先调了 search 搜索资料，再调 write_note 写了笔记，最后返回一段总结。

createReactAgent 底层就是一个 StateGraph——两个节点（agent 和 tools），一条条件边（有工具调用就去 tools，没有就结束）。它把我们前面手写的"判断工具调用→执行→塞回消息列表→再调模型"这个循环，用图的结构表达了出来。

这个快捷方式好用，但你没法控制任何细节。接下来我们把这层皮拆开，自己搭一遍。

Step 2：手搓 StateGraph——State、Node、Edge

LangGraph 的核心就三样东西：状态（State）、节点（Node）、边（Edge）。

把 src/index.ts 改成手动搭建：

src/index.ts
应用
复制
import 'dotenv/config';
import { StateGraph, START, END, Annotation } from "@langchain/langgraph";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { BaseMessage, AIMessage, ToolMessage } from "@langchain/core/messages";
import { z } from "zod";

// ========== 1. 定义工具 ==========
const searchTool = tool(
  async ({ query }) => {
    const results: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心是 StateGraph。",
      "Agent": "Agent 的核心是 while(true) { think → act → observe } 循环。",
    };
    const key = Object.keys(results).find(k => query.includes(k));
    return key ? results[key] : `没有找到「${query}」的信息`;
  },
  {
    name: "search",
    description: "搜索技术资料",
    schema: z.object({ query: z.string().describe("搜索关键词") }),
  }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 笔记已保存：${title}\n---\n${content}\n---`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "把研究结果写成笔记",
    schema: z.object({
      title: z.string().describe("笔记标题"),
      content: z.string().describe("笔记内容"),
    }),
  }
);

const tools = [searchTool, writeNoteTool];
const toolMap = Object.fromEntries(tools.map(t => [t.name, t]));

// ========== 2. 定义状态 ==========
const AgentState = Annotation.Root({   // 定义图的全局状态结构
  messages: Annotation<BaseMessage[]>({
    reducer: (current, update) => [...current, ...update],  // 追加策略：新消息拼到末尾
    default: () => [],                                       // 初始值：空数组
  }),
});

// ========== 3. 定义节点 ==========
// bindTools：把工具列表绑到模型上，模型才知道可以调哪些工具
const model = createModel().bindTools(tools);

// 节点函数签名：接收当前完整状态 → 返回状态的局部更新（由 reducer 合并）
async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);  // 把消息列表发给模型
  return { messages: [response] };                       // 返回的消息会被 reducer 追加到 state.messages
}

async function callTools(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  const toolCalls = lastMessage.tool_calls ?? [];  // 从模型回复中取出工具调用列表

  const results = await Promise.all(
    toolCalls.map(async (tc) => {
      const toolFn = toolMap[tc.name];              // 根据工具名找到对应的工具函数
      const result = await toolFn.invoke(tc.args);  // 执行工具，传入模型生成的参数
      return new ToolMessage({                      // 包装成 ToolMessage 返回给模型
        content: typeof result === "string" ? result : JSON.stringify(result),
        tool_call_id: tc.id!,                       // 必须带上 tool_call_id，模型靠它匹配调用和结果
      });
    })
  );

  return { messages: results };
}

// ========== 4. 定义边（路由逻辑）==========
// 路由函数：返回下一个节点的名字，或 END 表示结束
function shouldContinue(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  if (lastMessage.tool_calls?.length) {
    return "tools";   // 模型想调工具 → 走 tools 节点
  }
  return END;          // 没有工具调用 → 结束
}

// ========== 5. 组装并运行 ==========
const graph = new StateGraph(AgentState)  // 用状态定义初始化图
  .addNode("agent", callModel)            // 注册节点：名字 + 处理函数
  .addNode("tools", callTools)
  .addEdge(START, "agent")                // 固定边：图启动后第一个走 agent 节点
  .addConditionalEdges(
    "agent",          // 从哪个节点出发
    shouldContinue,   // 路由函数：返回下一个节点的名字（或 END）
    {                 // 路由映射：路由函数的返回值 → 实际节点名
      tools: "tools",
      [END]: END,
    }
  )
  .addEdge("tools", "agent")             // 固定边：tools 执行完回到 agent
  .compile();                             // 编译成可执行的图

async function main() {
  const result = await graph.invoke({
    messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }],
  });
  console.log("\n最终回复:", result.messages.at(-1)?.content);
}
main().catch(console.error);

bash
运行
复制
pnpm start


跟 Step 1 的效果一样，但现在你控制了每一个环节。

几个值得注意的细节：

bindTools(tools) 是容易漏掉的一步。ChatOpenAI 本身不知道有哪些工具可用，你得显式地把工具列表绑到模型上。不绑的话，模型永远不会生成 tool_calls——它压根不知道有工具可以调。这跟 Vercel AI SDK 把 tools 传进 generateText() 参数里是同一件事，只是 LangGraph 把模型和工具的绑定独立了出来。

reducer 定义了状态更新规则。messages 用追加策略——每个节点往里加新消息，不覆盖之前的。这跟我们前面手写 Agent 时 messages.push() 的效果一样，但 LangGraph 把规则声明在了一个地方，而不是散落在代码各处。

节点函数的返回值是状态的局部更新（{ messages: [...] }），不是完整状态。LangGraph 用 reducer 把更新合并进去。这个设计让每个节点只关心自己该做什么。

用图来表示整个执行流程：

对比一下前面手写的版本：

typescript
复制
while (true) {
  const response = await model.call(messages);
  messages.push(response);
  if (!response.toolCalls?.length) break;
  for (const tc of response.toolCalls) {
    const result = await executeTool(tc);
    messages.push(result);
  }
}


同样的逻辑，一个用显式循环，一个用图。对于这种简单的 ReAct 循环，说实话，while(true) 更清晰。 图的写法多了 State 声明、Node 函数签名、Edge 路由三层抽象，但表达的逻辑完全一样。

那 LangGraph 的价值在哪？继续往下加需求你就明白了。

Step 3：加上流式输出——逐 token 返回

任何面向用户的 Agent 都需要流式输出。LangGraph 提供了多种 stream mode（values、updates、messages、events、custom、debug 等），最常用的是这三种：

"updates" — 每个节点执行完后返回状态变更（节点级粒度）
"values" — 每个节点执行完后返回完整状态快照
"messages" — 逐 token 返回模型的输出（用户体验最好）

把最后的 invoke 改成 stream：

src/index.ts
应用
复制
// 把最后的 invoke 部分替换成：
const stream = await graph.stream(
  { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
  { streamMode: "updates" }  // "updates" 模式：每个节点执行完推送一次状态变更
);

// 每个 chunk 的 key 是节点名，value 是该节点返回的状态更新
for await (const chunk of stream) {
  const nodeName = Object.keys(chunk)[0];
  const update = chunk[nodeName];

  if (nodeName === "agent") {
    const msg = update.messages[0] as AIMessage;
    if (msg.tool_calls?.length) {
      console.log(`\n🔧 Agent 决定调用工具: ${msg.tool_calls.map((tc: any) => tc.name).join(", ")}`);
    } else {
      console.log(`\n💬 Agent 最终回复: ${msg.content}`);
    }
  }

  if (nodeName === "tools") {
    for (const msg of update.messages) {
      console.log(`📦 工具返回: ${msg.content}`);
    }
  }
}

bash
运行
复制
pnpm start


现在你能看到每一步的执行过程了——Agent 决定调什么工具、工具返回了什么、Agent 的最终回复。

"updates" 模式的粒度是节点级别的——每个节点跑完就推一次数据。如果你想要更细粒度的逐 token 流式，可以用 "messages" 模式：

src/index.ts
应用
复制
const stream = await graph.stream(
  { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
  { streamMode: "messages" }  // "messages" 模式：逐 token 推送模型输出
);

// 每次拿到 [消息片段, 元数据] 的 tuple
for await (const [chunk, metadata] of stream) {
  if (chunk.content) {
    process.stdout.write(chunk.content.toString());  // 逐 token 打印，不换行
  }
}
console.log();


"messages" 模式下，每次拿到的是一个 [消息片段, 元数据] 的 tuple。chunk.content 就是当前这个 token 的文本——跟我们在流式架构那篇里讲的 SSE text-delta 事件是同一个粒度。

Step 4：加上保险丝——轮次限制

没有保险丝的 Agent 就是没有刹车的车。我们在保险丝那篇里讲过，max_turns 是最基本的安全机制。

在 LangGraph 里实现这个，往状态里加一个 turnCount 字段：

src/index.ts
应用
复制
const AgentState = Annotation.Root({
  messages: Annotation<BaseMessage[]>({
    reducer: (current, update) => [...current, ...update],
    default: () => [],
  }),
  turnCount: Annotation<number>({
    reducer: (_, update) => update,
    default: () => 0,
  }),
});


turnCount 的 reducer 用的是覆盖策略（(_, update) => update）——每次直接用新值替换旧值，不像 messages 那样追加。

然后修改 callModel 和 shouldContinue：

src/index.ts
应用
复制
const MAX_TURNS = 5;

async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);
  return { messages: [response], turnCount: state.turnCount + 1 };
}

function shouldContinue(state: typeof AgentState.State) {
  if (state.turnCount >= MAX_TURNS) {
    console.log(`\n⚠️ 达到最大轮次 ${MAX_TURNS}，强制停止`);
    return END;
  }
  const lastMessage = state.messages.at(-1) as AIMessage;
  return lastMessage.tool_calls?.length ? "tools" : END;
}

bash
运行
复制
pnpm start


这就是用图的方式实现保险丝——路由函数里加一个判断。跟手写循环里的 while (turns < maxTurns) 效果一样，只是表达方式不同。

Step 5：加上人工审核——interrupt() 和 checkpoint

到这里你可能还是觉得 LangGraph 没什么特别的——while(true) 能做的事，图也能做，还多了一层抽象。

接下来这个需求就是 LangGraph 真正拉开差距的地方：工具执行前要人工审核，而且用户可以不立即回复——关掉浏览器，明天再来审核，Agent 要能恢复执行。

用手写循环实现这个，你得自己搞一套持久化——把 Agent 当前的消息列表、轮次、待审核的工具调用全部序列化存到 Redis 或数据库里，等用户回来后反序列化恢复。这不复杂但琐碎，边界情况很多。

LangGraph 用 interrupt() + checkpointer 直接搞定。

先加一个 humanReview 节点，然后重新组装图：

src/index.ts
应用
复制
import { interrupt, Command, MemorySaver } from "@langchain/langgraph";

async function humanReview(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  const toolCalls = lastMessage.tool_calls ?? [];

  // interrupt() 会冻结整个图的执行状态，把参数传给调用方展示
  // 调用方用 Command({ resume: value }) 恢复后，interrupt() 返回那个 value
  const decision = interrupt({
    question: "是否允许执行以下工具调用？",
    toolCalls: toolCalls.map(tc => ({ name: tc.name, args: tc.args })),
  });

  if (decision === "reject") {
    return {
      messages: toolCalls.map(tc =>
        new ToolMessage({ content: "用户拒绝了此操作", tool_call_id: tc.id! })
      ),
    };
  }

  return { messages: [] };  // 审核通过，不加消息，继续往下走
}

const graph = new StateGraph(AgentState)
  .addNode("agent", callModel)
  .addNode("review", humanReview)
  .addNode("tools", callTools)
  .addEdge(START, "agent")
  .addConditionalEdges("agent", (state) => {
    if (state.turnCount >= MAX_TURNS) return END;   // 保险丝
    const last = state.messages.at(-1) as AIMessage;
    return last.tool_calls?.length ? "review" : END; // 有工具调用先走审核
  })
  .addConditionalEdges("review", (state) => {
    const last = state.messages.at(-1);
    if (last instanceof ToolMessage) return "agent";  // 被拒绝了（已生成拒绝消息），回 agent 重新决策
    return "tools";                                    // 审核通过，继续执行工具
  })
  .addEdge("tools", "agent")
  .compile({ checkpointer: new MemorySaver() });  // 传入 checkpointer 才能使用 interrupt


注意 .compile() 里多了一个 checkpointer: new MemorySaver()。MemorySaver 是内存版的 checkpoint 存储，开发调试用。生产环境换成 SqliteSaver（@langchain/langgraph-checkpoint-sqlite）或 PostgresSaver（@langchain/langgraph-checkpoint-postgres），只需要改一行：

typescript
复制
import { SqliteSaver } from "@langchain/langgraph-checkpoint-sqlite";
const checkpointer = SqliteSaver.fromConnString("./agent-checkpoints.db");
// 或
import { PostgresSaver } from "@langchain/langgraph-checkpoint-postgres";
const checkpointer = PostgresSaver.fromConnString("postgresql://...");


现在运行这个图的方式也要变——因为 interrupt() 会暂停执行，你需要分两步走：

src/index.ts
应用
复制
// 交互式审核：CLI 阻塞等用户输入 y/n
import { createInterface } from "node:readline";

async function main() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string) => new Promise<string>(r => rl.question(q, r));

  const config = { configurable: { thread_id: "research-session-1" } };

  console.log("=== 发送请求 ===\n");
  let currentStream = await graph.stream(
    { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
    { ...config, streamMode: "updates" }
  );

  while (true) {
    let needsReview = false;

    for await (const chunk of currentStream) {
      const node = Object.keys(chunk)[0];
      if (node === "agent") { /* ... 打印工具调用或最终回复 ... */ }
      if (node === "tools") { /* ... 打印工具结果 ... */ }
      if (node === "__interrupt__") { needsReview = true; }
    }

    if (!needsReview) break;

    const answer = await ask("\nApprove? (y/n): ");
    const decision = answer.trim().toLowerCase() === "y" ? "approve" : "reject";

    currentStream = await graph.stream(
      new Command({ resume: decision }),
      { ...config, streamMode: "updates" }
    );
  }

  rl.close();
}
main().catch(console.error);

bash
运行
复制
pnpm start


运行后你会看到 Agent 先执行了 callModel，决定调用 search 工具，然后走到 review 节点——interrupt() 触发，整个图的状态被冻结到 checkpoint 里，执行暂停。

第二步用 Command({ resume: "approve" }) 恢复执行——interrupt() 返回 "approve"，humanReview 函数返回空消息（表示通过），接着走到 tools 节点执行搜索，然后回到 agent 节点继续处理。如果 Agent 又要调工具，又会被 review 拦住，等下一次审核。

这里有几个关键概念需要理解：

thread_id 就是我们前面讲的 Session。 每个 thread_id 对应一个独立的对话状态——这跟 ACP 那篇里的 newSession 是同一个概念，只是换了个名字。

checkpointer 就是我们在部署那篇里讲的崩溃恢复机制。 LangGraph 在每个节点执行完后自动创建 checkpoint。进程挂了，用同一个 thread_id 重新调用，它会从最后一个 checkpoint 恢复。这跟我们讲的 JSONL append-only log 思路一样，只是 LangGraph 帮你做了序列化和恢复的脏活。

Step 6：Research Agent 完整版——所有功能合到一起

把前面所有的改动合并成一个可以直接运行的完整文件：

src/index.ts
应用
复制
// src/index.ts — 完整的 LangGraph 研究助手
import 'dotenv/config';
import { StateGraph, START, END, Annotation, interrupt, Command, MemorySaver } from "@langchain/langgraph";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { BaseMessage, AIMessage, ToolMessage } from "@langchain/core/messages";
import { z } from "zod";

// ===== 工具定义 =====
const searchTool = tool(
  async ({ query }) => {
    const db: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心是 StateGraph，用节点和边描述执行流程。支持 checkpoint 持久化和 interrupt 人工审核。",
      "Agent": "Agent 的核心是 while(true) { think → act → observe } 循环，通过 Function Calling 调用外部工具。",
      "Harness": "Harness 是模型外面的壳——一组组件，每个组件对应一个模型的局限性假设。",
    };
    const key = Object.keys(db).find(k => query.includes(k));
    return key ? db[key] : `没有找到「${query}」的信息`;
  },
  { name: "search", description: "搜索技术资料", schema: z.object({ query: z.string() }) }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 [笔记] ${title}\n${content}\n`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "写研究笔记",
    schema: z.object({ title: z.string(), content: z.string() }),
  }
);

const tools = [searchTool, writeNoteTool];
const toolMap = Object.fromEntries(tools.map(t => [t.name, t]));

// ===== 状态 =====
const AgentState = Annotation.Root({
  messages: Annotation<BaseMessage[]>({
    reducer: (curr, update) => [...curr, ...update],
    default: () => [],
  }),
  turnCount: Annotation<number>({
    reducer: (_, v) => v,
    default: () => 0,
  }),
});

// ===== 节点 =====
const model = createModel().bindTools(tools);
const MAX_TURNS = 5;

async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);
  return { messages: [response], turnCount: state.turnCount + 1 };
}

async function humanReview(state: typeof AgentState.State) {
  const last = state.messages.at(-1) as AIMessage;
  const calls = last.tool_calls ?? [];

  const decision = interrupt({
    question: "允许执行以下工具调用？",
    tools: calls.map(tc => `${tc.name}(${JSON.stringify(tc.args)})`),
  });

  if (decision === "reject") {
    return {
      messages: calls.map(tc =>
        new ToolMessage({ content: "用户拒绝了此操作", tool_call_id: tc.id! })
      ),
    };
  }
  return { messages: [] };
}

async function callTools(state: typeof AgentState.State) {
  const last = state.messages.at(-1) as AIMessage;
  const calls = last.tool_calls ?? [];
  const results = await Promise.all(
    calls.map(async tc => {
      const fn = toolMap[tc.name];
      const result = await fn.invoke(tc.args);
      return new ToolMessage({
        content: typeof result === "string" ? result : JSON.stringify(result),
        tool_call_id: tc.id!,
      });
    })
  );
  return { messages: results };
}

// ===== 组装图 =====
const graph = new StateGraph(AgentState)
  .addNode("agent", callModel)
  .addNode("review", humanReview)
  .addNode("tools", callTools)
  .addEdge(START, "agent")
  .addConditionalEdges("agent", (state) => {
    if (state.turnCount >= MAX_TURNS) return END;
    const last = state.messages.at(-1) as AIMessage;
    return last.tool_calls?.length ? "review" : END;
  })
  .addConditionalEdges("review", (state) => {
    const last = state.messages.at(-1);
    return last instanceof ToolMessage ? "agent" : "tools";
  })
  .addEdge("tools", "agent")
  .compile({ checkpointer: new MemorySaver() });

// ===== 运行（交互式审核）=====
import { createInterface } from "node:readline";

async function main() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string) => new Promise<string>(r => rl.question(q, r));

  const config = { configurable: { thread_id: "session-1" } };

  console.log("=== 发送请求 ===\n");
  let currentStream = await graph.stream(
    { messages: [{ role: "user", content: "帮我搜索 LangGraph 和 Agent 的资料，写一篇对比笔记" }] },
    { ...config, streamMode: "updates" }
  );

  while (true) {
    let needsReview = false;

    for await (const chunk of currentStream) {
      const node = Object.keys(chunk)[0];
      if (node === "agent") {
        const msg = chunk[node].messages[0] as AIMessage;
        if (msg.tool_calls?.length) {
          console.log(`[TOOL] ${msg.tool_calls.map((tc: any) => `${tc.name}(${JSON.stringify(tc.args)})`).join(", ")}`);
        } else {
          console.log(`\n[REPLY] ${msg.content}`);
        }
      }
      if (node === "tools") {
        chunk[node].messages.forEach((m: any) => console.log(`[RES] ${m.content}`));
      }
      if (node === "__interrupt__") {
        needsReview = true;
      }
    }

    if (!needsReview) break;

    const answer = await ask("\nApprove? (y/n): ");
    const decision = answer.trim().toLowerCase() === "y" ? "approve" : "reject";
    console.log(decision === "approve" ? "\n=== Approved ===\n" : "\n=== Rejected ===\n");

    currentStream = await graph.stream(
      new Command({ resume: decision }),
      { ...config, streamMode: "updates" }
    );
  }

  rl.close();
}
main().catch(console.error);

bash
运行
复制
pnpm start


这个 Agent 会：搜索 LangGraph 和 Agent 的资料（每次搜索前暂停等你审核）→ 拿到结果后写一篇对比笔记（写之前也要审核）→ 最终返回总结。如果超过 5 轮还没完成，保险丝强制停止。

到这里，你已经用 LangGraph 搭了一个完整的、有实战价值的 Agent。回头看一下整个项目用到了哪些 LangGraph 的核心能力：

能力	对应前面课程的什么概念
StateGraph + 条件边	Agent Loop（while(true) 循环）
bindTools()	Function Calling（工具管线那篇）
turnCount + 路由判断	Harness 保险丝（max_turns）
interrupt() + Command	Harness 权限系统（requestPermission）
stream() + streamMode	流式架构（SSE text-delta）
thread_id + checkpointer	Memory 会话持久化（Session 管理）

每一个 LangGraph 的特性，你都能在前面的课程里找到对应的第一性原理。 框架只是把这些原理用另一种方式封装了起来。

用六大支柱来透视 LangGraph

项目做完了，现在退一步，用课程里建立的六大支柱框架来系统评估 LangGraph。

Agent Loop：图替代了 while(true)。 对简单 ReAct 模式是等价替换。复杂分支（多子流程、条件路由）用图比嵌套 if-else 清晰，但图一旦复杂起来调试更难——手写循环加一行 console.log 就能定位问题，图里你得理解节点调度顺序、状态合并和路由逻辑。

Tool System：和手写的没有本质区别。 底层走 OpenAI Function Calling 协议。MCP 通过官方的 @langchain/mcp-adapters 包支持，不是 LangGraph 核心库内置的——需要额外装一个包，但属于官方维护的集成路径。

Context Engineering：核心库不内置。 LangGraph 本身的 reducer 只做消息追加，没有压缩、没有 KV Cache 意识、没有 token 预算。不过 LangChain 生态提供了配套方案——LangMem SDK 可以做消息摘要压缩，Deep Agents 方案支持大结果 offloading 和阈值触发压缩。但这些都不是 LangGraph 开箱即用的，需要自己集成。LangGraph 管流程编排，上下文管理得自己搭。

Memory：通过 Checkpoint 实现短期记忆，长期记忆靠生态。 LangGraph 的 Checkpoint 机制天然支持会话级短期记忆——同一个 thread_id 下的所有对话状态会自动持久化，进程崩了也能恢复，这是 LangGraph 对比手写 while(true) 最大的价值之一。interrupt() + checkpoint 还让人工审核变得很简单——冻结状态等用户回来继续。但跨会话的长期记忆（用户画像、知识沉淀、记忆衰减）LangGraph 核心库不管，需要通过 LangMem SDK 等生态工具自己接。

Multi-Agent：通过子图（SubGraph）嵌入和 Swarm 实现。 跟我们讲的 Parent-Child 模式结构一样。LangGraph 支持线程级隔离（每个 thread_id 独立的 checkpoint history）。

Harness：LangGraph 不是 Harness。 保险丝、上下文压缩、权限控制、Generator/Evaluator 分离——这些 Harness 零件都没有现成的。你可以用图来组织 Harness 组件，但设计和实现每个组件本身还是你的事。

学完这门课再看框架

LangGraph 核心解决的是 Agent Loop 编排和 Memory 中的会话持久化——六大支柱里的两根。 Tool System 和 Multi-Agent 通过生态包支持（adapter、swarm），Context Engineering、长期 Memory 和 Harness 基本得自己来。

这不是批评 LangGraph。一个框架不可能覆盖所有维度。但这也解释了为什么很多团队"用 LangGraph 用到一半就想弃"——他们以为用了框架就解决了 Agent 开发的主要问题，结果发现上下文管理、权限控制、Harness 设计这些真正的硬骨头，框架一个都没帮他们啃。

回到我们在框架那篇开头的观点：框架解决的是"搭建过程中的体力活"，不解决"设计过程中的脑力活"。LangGraph 帮你省了写持久化层的体力，但 Agent 该怎么编排、上下文该怎么管理、工具该怎么配置——这些设计决策还是你的。

所以如果你是先学了 LangGraph 再来学这门课的读者——你现在应该明白了，为什么光会用框架是不够的。框架告诉你"怎么把节点连起来"，但它不告诉你"为什么要这样连"。

如果你是跟着课程一路学到这里的——你已经有了自己的判断标准。任何框架放到你面前，你都能用六大支柱快速定位它的能力边界：Loop 层做了什么？Tool 层做了什么？Context 层有没有？Memory 怎么管？Multi-Agent 怎么编排？

这把尺子比任何一个框架都耐用。

这篇我们用 LangGraph 完整走了一遍。下一篇把视野拉开——LangGraph 之外还有 Mastra、Pi 等一堆框架，各自覆盖了六大支柱的哪些部分、留了哪些空白？用同一把尺子量一遍，你就能在做技术选型的时候快速判断：哪个框架做了什么事情，哪些 Agent 能力需要完全由我们自己来接管。让我们下一节再见。

参考资料
LangGraph 官方文档
LangGraph npm 包
LangGraph interrupt() 博客
AI Agent 框架对比 2026
Harness Engineering（Anthropic 博客）
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
ACP：标准化 Agent 的控制接口
下一篇 · 第七章：回到框架
社区各大框架全景：用六大支柱透视任何 Agent 框架
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
import 'dotenv/config';
import { createReactAgent } from "@langchain/langgraph/prebuilt";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { z } from "zod";

const searchTool = tool(
  async ({ query }) => {
    const results: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心概念是 StateGraph，用节点和边来描述 Agent 的执行流程。",
      "Agent Loop": "Agent Loop 是 AI Agent 的核心运行机制，本质是 while(true) { think → act → observe } 循环。",
    };
    const key = Object.keys(results).find(k => query.includes(k));
    return key ? results[key] : `没有找到关于「${query}」的信息`;
  },
  {
    name: "search",
    description: "搜索技术资料",
    schema: z.object({ query: z.string().describe("搜索关键词") }),
  }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 笔记已保存：${title}\n---\n${content}\n---`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "把研究结果写成笔记保存下来",
    schema: z.object({
      title: z.string().describe("笔记标题"),
      content: z.string().describe("笔记内容"),
    }),
  }
);

const agent = createReactAgent({
  llm: createModel(),
  tools: [searchTool, writeNoteTool],
});

async function main() {
  const result = await agent.invoke({
    messages: [{ role: "user", content: "帮我搜索一下 LangGraph 的资料，然后写一篇笔记总结" }],
  });
  console.log("\n最终回复:", result.messages.at(-1)?.content);
}
main().catch(console.error);
```


```bash
pnpm start
```


```typescript
import 'dotenv/config';
import { StateGraph, START, END, Annotation } from "@langchain/langgraph";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { BaseMessage, AIMessage, ToolMessage } from "@langchain/core/messages";
import { z } from "zod";

// ========== 1. 定义工具 ==========
const searchTool = tool(
  async ({ query }) => {
    const results: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心是 StateGraph。",
      "Agent": "Agent 的核心是 while(true) { think → act → observe } 循环。",
    };
    const key = Object.keys(results).find(k => query.includes(k));
    return key ? results[key] : `没有找到「${query}」的信息`;
  },
  {
    name: "search",
    description: "搜索技术资料",
    schema: z.object({ query: z.string().describe("搜索关键词") }),
  }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 笔记已保存：${title}\n---\n${content}\n---`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "把研究结果写成笔记",
    schema: z.object({
      title: z.string().describe("笔记标题"),
      content: z.string().describe("笔记内容"),
    }),
  }
);

const tools = [searchTool, writeNoteTool];
const toolMap = Object.fromEntries(tools.map(t => [t.name, t]));

// ========== 2. 定义状态 ==========
const AgentState = Annotation.Root({   // 定义图的全局状态结构
  messages: Annotation<BaseMessage[]>({
    reducer: (current, update) => [...current, ...update],  // 追加策略：新消息拼到末尾
    default: () => [],                                       // 初始值：空数组
  }),
});

// ========== 3. 定义节点 ==========
// bindTools：把工具列表绑到模型上，模型才知道可以调哪些工具
const model = createModel().bindTools(tools);

// 节点函数签名：接收当前完整状态 → 返回状态的局部更新（由 reducer 合并）
async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);  // 把消息列表发给模型
  return { messages: [response] };                       // 返回的消息会被 reducer 追加到 state.messages
}

async function callTools(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  const toolCalls = lastMessage.tool_calls ?? [];  // 从模型回复中取出工具调用列表

  const results = await Promise.all(
    toolCalls.map(async (tc) => {
      const toolFn = toolMap[tc.name];              // 根据工具名找到对应的工具函数
      const result = await toolFn.invoke(tc.args);  // 执行工具，传入模型生成的参数
      return new ToolMessage({                      // 包装成 ToolMessage 返回给模型
        content: typeof result === "string" ? result : JSON.stringify(result),
        tool_call_id: tc.id!,                       // 必须带上 tool_call_id，模型靠它匹配调用和结果
      });
    })
  );

  return { messages: results };
}

// ========== 4. 定义边（路由逻辑）==========
// 路由函数：返回下一个节点的名字，或 END 表示结束
function shouldContinue(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  if (lastMessage.tool_calls?.length) {
    return "tools";   // 模型想调工具 → 走 tools 节点
  }
  return END;          // 没有工具调用 → 结束
}

// ========== 5. 组装并运行 ==========
const graph = new StateGraph(AgentState)  // 用状态定义初始化图
  .addNode("agent", callModel)            // 注册节点：名字 + 处理函数
  .addNode("tools", callTools)
  .addEdge(START, "agent")                // 固定边：图启动后第一个走 agent 节点
  .addConditionalEdges(
    "agent",          // 从哪个节点出发
    shouldContinue,   // 路由函数：返回下一个节点的名字（或 END）
    {                 // 路由映射：路由函数的返回值 → 实际节点名
      tools: "tools",
      [END]: END,
    }
  )
  .addEdge("tools", "agent")             // 固定边：tools 执行完回到 agent
  .compile();                             // 编译成可执行的图

async function main() {
  const result = await graph.invoke({
    messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }],
  });
  console.log("\n最终回复:", result.messages.at(-1)?.content);
}
main().catch(console.error);
```


```bash
pnpm start
```


```typescript
while (true) {
  const response = await model.call(messages);
  messages.push(response);
  if (!response.toolCalls?.length) break;
  for (const tc of response.toolCalls) {
    const result = await executeTool(tc);
    messages.push(result);
  }
}
```


```typescript
// 把最后的 invoke 部分替换成：
const stream = await graph.stream(
  { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
  { streamMode: "updates" }  // "updates" 模式：每个节点执行完推送一次状态变更
);

// 每个 chunk 的 key 是节点名，value 是该节点返回的状态更新
for await (const chunk of stream) {
  const nodeName = Object.keys(chunk)[0];
  const update = chunk[nodeName];

  if (nodeName === "agent") {
    const msg = update.messages[0] as AIMessage;
    if (msg.tool_calls?.length) {
      console.log(`\n🔧 Agent 决定调用工具: ${msg.tool_calls.map((tc: any) => tc.name).join(", ")}`);
    } else {
      console.log(`\n💬 Agent 最终回复: ${msg.content}`);
    }
  }

  if (nodeName === "tools") {
    for (const msg of update.messages) {
      console.log(`📦 工具返回: ${msg.content}`);
    }
  }
}
```


```bash
pnpm start
```


```typescript
const stream = await graph.stream(
  { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
  { streamMode: "messages" }  // "messages" 模式：逐 token 推送模型输出
);

// 每次拿到 [消息片段, 元数据] 的 tuple
for await (const [chunk, metadata] of stream) {
  if (chunk.content) {
    process.stdout.write(chunk.content.toString());  // 逐 token 打印，不换行
  }
}
console.log();
```


```typescript
const AgentState = Annotation.Root({
  messages: Annotation<BaseMessage[]>({
    reducer: (current, update) => [...current, ...update],
    default: () => [],
  }),
  turnCount: Annotation<number>({
    reducer: (_, update) => update,
    default: () => 0,
  }),
});
```


```typescript
const MAX_TURNS = 5;

async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);
  return { messages: [response], turnCount: state.turnCount + 1 };
}

function shouldContinue(state: typeof AgentState.State) {
  if (state.turnCount >= MAX_TURNS) {
    console.log(`\n⚠️ 达到最大轮次 ${MAX_TURNS}，强制停止`);
    return END;
  }
  const lastMessage = state.messages.at(-1) as AIMessage;
  return lastMessage.tool_calls?.length ? "tools" : END;
}
```


```bash
pnpm start
```


```typescript
import { interrupt, Command, MemorySaver } from "@langchain/langgraph";

async function humanReview(state: typeof AgentState.State) {
  const lastMessage = state.messages.at(-1) as AIMessage;
  const toolCalls = lastMessage.tool_calls ?? [];

  // interrupt() 会冻结整个图的执行状态，把参数传给调用方展示
  // 调用方用 Command({ resume: value }) 恢复后，interrupt() 返回那个 value
  const decision = interrupt({
    question: "是否允许执行以下工具调用？",
    toolCalls: toolCalls.map(tc => ({ name: tc.name, args: tc.args })),
  });

  if (decision === "reject") {
    return {
      messages: toolCalls.map(tc =>
        new ToolMessage({ content: "用户拒绝了此操作", tool_call_id: tc.id! })
      ),
    };
  }

  return { messages: [] };  // 审核通过，不加消息，继续往下走
}

const graph = new StateGraph(AgentState)
  .addNode("agent", callModel)
  .addNode("review", humanReview)
  .addNode("tools", callTools)
  .addEdge(START, "agent")
  .addConditionalEdges("agent", (state) => {
    if (state.turnCount >= MAX_TURNS) return END;   // 保险丝
    const last = state.messages.at(-1) as AIMessage;
    return last.tool_calls?.length ? "review" : END; // 有工具调用先走审核
  })
  .addConditionalEdges("review", (state) => {
    const last = state.messages.at(-1);
    if (last instanceof ToolMessage) return "agent";  // 被拒绝了（已生成拒绝消息），回 agent 重新决策
    return "tools";                                    // 审核通过，继续执行工具
  })
  .addEdge("tools", "agent")
  .compile({ checkpointer: new MemorySaver() });  // 传入 checkpointer 才能使用 interrupt
```


```typescript
import { SqliteSaver } from "@langchain/langgraph-checkpoint-sqlite";
const checkpointer = SqliteSaver.fromConnString("./agent-checkpoints.db");
// 或
import { PostgresSaver } from "@langchain/langgraph-checkpoint-postgres";
const checkpointer = PostgresSaver.fromConnString("postgresql://...");
```


```typescript
// 交互式审核：CLI 阻塞等用户输入 y/n
import { createInterface } from "node:readline";

async function main() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string) => new Promise<string>(r => rl.question(q, r));

  const config = { configurable: { thread_id: "research-session-1" } };

  console.log("=== 发送请求 ===\n");
  let currentStream = await graph.stream(
    { messages: [{ role: "user", content: "搜索一下 LangGraph，然后写个笔记总结" }] },
    { ...config, streamMode: "updates" }
  );

  while (true) {
    let needsReview = false;

    for await (const chunk of currentStream) {
      const node = Object.keys(chunk)[0];
      if (node === "agent") { /* ... 打印工具调用或最终回复 ... */ }
      if (node === "tools") { /* ... 打印工具结果 ... */ }
      if (node === "__interrupt__") { needsReview = true; }
    }

    if (!needsReview) break;

    const answer = await ask("\nApprove? (y/n): ");
    const decision = answer.trim().toLowerCase() === "y" ? "approve" : "reject";

    currentStream = await graph.stream(
      new Command({ resume: decision }),
      { ...config, streamMode: "updates" }
    );
  }

  rl.close();
}
main().catch(console.error);
```


```bash
pnpm start
```


```typescript
// src/index.ts — 完整的 LangGraph 研究助手
import 'dotenv/config';
import { StateGraph, START, END, Annotation, interrupt, Command, MemorySaver } from "@langchain/langgraph";
import { createModel } from "./model.js";
import { tool } from "@langchain/core/tools";
import { BaseMessage, AIMessage, ToolMessage } from "@langchain/core/messages";
import { z } from "zod";

// ===== 工具定义 =====
const searchTool = tool(
  async ({ query }) => {
    const db: Record<string, string> = {
      "LangGraph": "LangGraph 是 LangChain 团队开发的图编排框架，核心是 StateGraph，用节点和边描述执行流程。支持 checkpoint 持久化和 interrupt 人工审核。",
      "Agent": "Agent 的核心是 while(true) { think → act → observe } 循环，通过 Function Calling 调用外部工具。",
      "Harness": "Harness 是模型外面的壳——一组组件，每个组件对应一个模型的局限性假设。",
    };
    const key = Object.keys(db).find(k => query.includes(k));
    return key ? db[key] : `没有找到「${query}」的信息`;
  },
  { name: "search", description: "搜索技术资料", schema: z.object({ query: z.string() }) }
);

const writeNoteTool = tool(
  async ({ title, content }) => {
    console.log(`\n📝 [笔记] ${title}\n${content}\n`);
    return `笔记「${title}」保存成功`;
  },
  {
    name: "write_note",
    description: "写研究笔记",
    schema: z.object({ title: z.string(), content: z.string() }),
  }
);

const tools = [searchTool, writeNoteTool];
const toolMap = Object.fromEntries(tools.map(t => [t.name, t]));

// ===== 状态 =====
const AgentState = Annotation.Root({
  messages: Annotation<BaseMessage[]>({
    reducer: (curr, update) => [...curr, ...update],
    default: () => [],
  }),
  turnCount: Annotation<number>({
    reducer: (_, v) => v,
    default: () => 0,
  }),
});

// ===== 节点 =====
const model = createModel().bindTools(tools);
const MAX_TURNS = 5;

async function callModel(state: typeof AgentState.State) {
  const response = await model.invoke(state.messages);
  return { messages: [response], turnCount: state.turnCount + 1 };
}

async function humanReview(state: typeof AgentState.State) {
  const last = state.messages.at(-1) as AIMessage;
  const calls = last.tool_calls ?? [];

  const decision = interrupt({
    question: "允许执行以下工具调用？",
    tools: calls.map(tc => `${tc.name}(${JSON.stringify(tc.args)})`),
  });

  if (decision === "reject") {
    return {
      messages: calls.map(tc =>
        new ToolMessage({ content: "用户拒绝了此操作", tool_call_id: tc.id! })
      ),
    };
  }
  return { messages: [] };
}

async function callTools(state: typeof AgentState.State) {
  const last = state.messages.at(-1) as AIMessage;
  const calls = last.tool_calls ?? [];
  const results = await Promise.all(
    calls.map(async tc => {
      const fn = toolMap[tc.name];
      const result = await fn.invoke(tc.args);
      return new ToolMessage({
        content: typeof result === "string" ? result : JSON.stringify(result),
        tool_call_id: tc.id!,
      });
    })
  );
  return { messages: results };
}

// ===== 组装图 =====
const graph = new StateGraph(AgentState)
  .addNode("agent", callModel)
  .addNode("review", humanReview)
  .addNode("tools", callTools)
  .addEdge(START, "agent")
  .addConditionalEdges("agent", (state) => {
    if (state.turnCount >= MAX_TURNS) return END;
    const last = state.messages.at(-1) as AIMessage;
    return last.tool_calls?.length ? "review" : END;
  })
  .addConditionalEdges("review", (state) => {
    const last = state.messages.at(-1);
    return last instanceof ToolMessage ? "agent" : "tools";
  })
  .addEdge("tools", "agent")
  .compile({ checkpointer: new MemorySaver() });

// ===== 运行（交互式审核）=====
import { createInterface } from "node:readline";

async function main() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string) => new Promise<string>(r => rl.question(q, r));

  const config = { configurable: { thread_id: "session-1" } };

  console.log("=== 发送请求 ===\n");
  let currentStream = await graph.stream(
    { messages: [{ role: "user", content: "帮我搜索 LangGraph 和 Agent 的资料，写一篇对比笔记" }] },
    { ...config, streamMode: "updates" }
  );

  while (true) {
    let needsReview = false;

    for await (const chunk of currentStream) {
      const node = Object.keys(chunk)[0];
      if (node === "agent") {
        const msg = chunk[node].messages[0] as AIMessage;
        if (msg.tool_calls?.length) {
          console.log(`[TOOL] ${msg.tool_calls.map((tc: any) => `${tc.name}(${JSON.stringify(tc.args)})`).join(", ")}`);
        } else {
          console.log(`\n[REPLY] ${msg.content}`);
        }
      }
      if (node === "tools") {
        chunk[node].messages.forEach((m: any) => console.log(`[RES] ${m.content}`));
      }
      if (node === "__interrupt__") {
        needsReview = true;
      }
    }

    if (!needsReview) break;

    const answer = await ask("\nApprove? (y/n): ");
    const decision = answer.trim().toLowerCase() === "y" ? "approve" : "reject";
    console.log(decision === "approve" ? "\n=== Approved ===\n" : "\n=== Rejected ===\n");

    currentStream = await graph.stream(
      new Command({ resume: decision }),
      { ...config, streamMode: "updates" }
    );
  }

  rl.close();
}
main().catch(console.error);
```


```bash
pnpm start
```
