# 给 Agent 一双手——Tool 注册、执行、截断与并发

课程
Super Agent 实战课
给 Agent 一双手——Tool 注册、执行、截断与并发
给 Agent 一双手——Tool 注册、执行、截断与并发

约 41 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇给 Agent 装了三层防护，它不会再失控了。但你试过让它干点正经事吗？

You: 帮我看一下当前目录有哪些文件
Agent: 当前目录下通常会有 src、package.json、node_modules……


它又在编了。它没有 list_directory 工具，所以只能靠猜。

再试一个：

You: 帮我读一下 package.json
Agent: 你可以用 cat package.json 查看……


它在教你怎么做，而不是帮你做。这就是现在的问题——Agent 只有两个玩具工具（天气和计算器），面对真实任务完全无能为力。

更大的问题是，就算你想加 10 个工具，现在的代码结构也不支持。工具定义散落在 tools.ts 里，和 Agent Loop 耦合在一起，没有统一的注册、没有结果管理。一个工具返回 10 万字符的文件内容，直接塞进上下文——一次调用就能把你的 token 预算吃掉一半。

这篇我们来解决这些问题：搭一个正经的工具系统，从注册到执行到截断，每一层都有明确的职责。

先装依赖：

bash
运行
复制
pnpm install

Tool 到底是什么

在动手之前，先想清楚一个根本问题：对模型来说，一个 Tool 到底是什么？

说实话，就三样东西：

一段描述——告诉模型"这个工具干什么、什么时候该用"
一份参数 Schema——告诉模型"调用时需要传什么参数、什么类型"
一个执行函数——真正干活的代码

但在生产环境里，光有这三样不够。你还需要知道：这个工具能不能和别的工具并行执行？它是只读的还是会修改系统状态？它的返回结果最大有多长？

所以我们定义的工具接口长这样：

typescript
复制
interface ToolDefinition {
  name: string;
  description: string;        // 给模型看的描述
  parameters: Record<string, unknown>;  // JSON Schema
  execute: (input: any) => Promise<unknown>;

  // 元数据——给 Agent Loop 做决策用
  isConcurrencySafe?: boolean;  // 能否并行
  isReadOnly?: boolean;         // 是否只读
  maxResultChars?: number;      // 结果最大长度
}


前三个字段是模型需要的，后三个是 Agent Loop 需要的。这个区分很重要——模型只关心"怎么调"，Agent Loop 关心"怎么管"。 把这两类信息放在同一个接口里，注册一次就能同时服务两个消费者。

你可能会问，为什么不直接用 AI SDK 的 tool() helper？因为 AI SDK 的工具定义只关心模型交互——描述、参数、执行。它不管并发安全、不管结果截断、不管权限。我们需要一个更厚的抽象层，把运行时的控制逻辑也打包进去。

搭建 ToolRegistry

有了统一接口，下一步是给工具找个"家"。ToolRegistry 做三件事：注册工具、查找工具、转换成 AI SDK 需要的格式。

创建 src/tool-registry.ts：

src/tools/registry.ts
应用
复制
import { jsonSchema } from 'ai';

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  isConcurrencySafe?: boolean;
  isReadOnly?: boolean;
  maxResultChars?: number;
  execute: (input: any) => Promise<unknown>;
}

const DEFAULT_MAX_RESULT_CHARS = 3000;

export class ToolRegistry {
  private tools = new Map<string, ToolDefinition>();

  register(...tools: ToolDefinition[]): void {
    for (const tool of tools) {
      this.tools.set(tool.name, tool);
    }
  }

  get(name: string): ToolDefinition | undefined {
    return this.tools.get(name);
  }

  getAll(): ToolDefinition[] {
    return Array.from(this.tools.values());
  }

  toAISDKFormat(): Record<string, any> {
    const result: Record<string, any> = {};
    for (const [name, tool] of this.tools) {
      const maxChars = tool.maxResultChars;
      const executeFn = tool.execute;
      result[name] = {
        description: tool.description,
        inputSchema: jsonSchema(tool.parameters as any),
        execute: async (input: any) => {
          const raw = await executeFn(input);
          const text = typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2);
          return truncateResult(text, maxChars);
        },
      };
    }
    return result;
  }
}

export function truncateResult(text: string, maxChars: number = DEFAULT_MAX_RESULT_CHARS): string {
  if (text.length <= maxChars) return text;

  const headSize = Math.floor(maxChars * 0.6);
  const tailSize = maxChars - headSize;
  const head = text.slice(0, headSize);
  const tail = text.slice(-tailSize);
  const dropped = text.length - headSize - tailSize;

  return `${head}\n\n... [省略 ${dropped} 字符] ...\n\n${tail}`;
}


注意 toAISDKFormat() 这个方法。它做了一件关键的事：把我们自定义的 ToolDefinition 转换成 Vercel AI SDK 的工具格式，同时在 execute 里包了一层截断逻辑。

这意味着工具注册一次，AI SDK 拿到的就是已经包含截断保护的版本。Agent Loop 不需要关心截断细节——ToolRegistry 帮你兜底了。

从两个玩具到五个正经工具

现在来把工具定义迁移到新的 ToolDefinition 格式，顺便加三个真正有用的文件操作工具。

创建 src/tools.ts：

src/tools/index.ts
应用
复制
import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { ToolDefinition } from './tool-registry.js';

export const weatherTool: ToolDefinition = {
  name: 'get_weather',
  description: '查询指定城市的天气信息',
  parameters: {
    type: 'object',
    properties: {
      city: { type: 'string', description: '城市名称，如"北京"、"上海"' },
    },
    required: ['city'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ city }: { city: string }) => {
    const data: Record<string, string> = {
      '北京': '晴，15-25°C，东南风 2 级',
      '上海': '多云，18-22°C，西南风 3 级',
      '深圳': '阵雨，22-28°C，南风 2 级',
    };
    return data[city] || `${city}：暂无数据`;
  },
};

export const calculatorTool: ToolDefinition = {
  // ... 同前，省略 ...
};

export const readFileTool: ToolDefinition = {
  name: 'read_file',
  description: '读取指定路径的文件内容',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
    },
    required: ['path'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 500,  // 演示用，生产环境通常 50000+
  execute: async ({ path }: { path: string }) => {
    return readFileSync(resolve(path), 'utf-8');
  },
};

export const writeFileTool: ToolDefinition = {
  name: 'write_file',
  description: '写入内容到指定文件',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
      content: { type: 'string', description: '要写入的内容' },
    },
    required: ['path', 'content'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,  // 写操作不能并行
  isReadOnly: false,
  execute: async ({ path, content }: { path: string; content: string }) => {
    writeFileSync(resolve(path), content, 'utf-8');
    return `已写入 ${content.length} 字符到 ${path}`;
  },
};

export const listDirectoryTool: ToolDefinition = {
  name: 'list_directory',
  description: '列出指定目录下的文件和子目录',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '目录路径，默认为当前目录' },
    },
    required: [],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ path = '.' }: { path?: string }) => {
    const resolved = resolve(path);
    return readdirSync(resolved).map(name => {
      const stat = statSync(join(resolved, name));
      return `${stat.isDirectory() ? '[DIR]' : '[FILE]'} ${name}`;
    }).join('\n');
  },
};

export const allTools: ToolDefinition[] = [
  weatherTool, calculatorTool, readFileTool, writeFileTool, listDirectoryTool,
];


你看，每个工具除了 execute 之外，都带了 isConcurrencySafe 和 isReadOnly 元数据。read_file 是只读的，可以和其他只读工具并行；write_file 有副作用，必须独占执行。这个设计的灵感来源是：并发安全性不是按工具名决定的，而是按行为决定的。 同一个 bash 工具，ls 是只读的可以并发，rm -rf 就必须串行。

另外注意 readFileTool 的 maxResultChars: 500——这里故意设低了方便演示截断效果。生产环境通常设 50000 甚至更高。

更新入口和 Agent Loop

入口文件改动不大，把硬编码的工具对象换成 Registry：

src/index.ts
应用
复制
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { ToolRegistry } from './tool-registry.js';
import { allTools } from './tools.js';
import { agentLoop } from './agent-loop.js';

const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});

const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();

const registry = new ToolRegistry();
registry.register(...allTools);

console.log(`已注册 ${registry.getAll().length} 个工具：`);
for (const tool of registry.getAll()) {
  const flags = [
    tool.isConcurrencySafe ? '可并发' : '串行',
    tool.isReadOnly ? '只读' : '读写',
  ].join(', ');
  console.log(`  - ${tool.name}（${flags}）`);
}

// ... readline + agentLoop 调用，结构同上一篇 ...


Agent Loop 的改动也很小——把 tools: any 换成 registry: ToolRegistry，用 registry.toAISDKFormat() 传给 streamText。工具执行和结果截断都在 Registry 的 execute 包装里自动完成了，Agent Loop 本身不需要改多少：

src/agent/loop.ts
应用
复制
import { streamText, type ModelMessage } from 'ai';
import { ToolRegistry } from './tool-registry.js';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;
const TOKEN_BUDGET = 50000;

export async function agentLoop(
  model: any,
  registry: ToolRegistry,
  messages: ModelMessage[],
  system: string,
) {
  // ... 核心结构同上一篇 ...
  // 关键变化：tools 参数改为 registry.toAISDKFormat()
  // tool-result 输出加了预览截断（超 120 字符只显示前 120）

  const result = streamText({
    model,
    system,
    tools: registry.toAISDKFormat(), // ← 这里
    messages,
    maxRetries: 0,
    onError: () => {},
  });
  // ... 其余处理逻辑同上一篇 ...
}


跑起来试试：

bash
运行
复制
pnpm start


右侧 WebContainer 终端对退格键支持有限，输入时建议一次打完再回车。

先确认老功能没坏——输入「北京天气怎么样」，应该和之前一样正常返回。

然后试试新能力：

You: 帮我看看当前目录有什么文件

--- Step 1 ---
  [调用: list_directory({"path":"."})]
  [结果: list_directory] [FILE] .env
[DIR] node_modules
[FILE] package.json
[FILE] sample-data.txt
[DIR] src
[FILE] tsconfig.json
  → 继续下一步...

--- Step 2 ---
当前目录的文件列表：
[FILE] .env  [DIR] node_modules  [FILE] package.json ...


Agent 能列目录了。再试读文件：

You: 读取 package.json

--- Step 1 ---
  [调用: read_file({"path":"package.json"})]
  [结果: read_file] { "name": "super-agent-04-tool-system", ...


从两个玩具工具到五个真实工具，整个过程只做了两件事：定义 ToolDefinition，注册到 Registry。Agent Loop 一行核心代码没改。

结果截断：上下文保卫战

刚才读 package.json 的时候，你可能注意到结果完整显示了——因为它只有 360 多个字符，没超过 maxResultChars: 500 的限制。

现在试试读一个大文件。输入「测试截断」：

You: 测试截断

--- Step 1 ---
  [调用: read_file({"path":"sample-data.txt"})]
  [结果: read_file] Super Agent 工具系统设计文档
=============================

一、工具注册机制
每个工具通过 ToolRegistry 统一注册...

... [省略 538 字符] ...

...错误信息要对模型友好——模型需要理解为什么失败才能换策略
4. 结果格式要结构化——JSON 比自然语言更容易被模型准确解析


看到了吗？文件有 1000 多字符，超过了 500 的限制。truncateResult 自动做了 Head/Tail 60/40 分割——保留前 60%（文件头部的标题和开头内容）和后 40%（文件末尾的最佳实践），中间用 [省略 N 字符] 标记。

为什么是 60/40 而不是 100/0（只保留头部）？因为很多时候文件尾部的信息比中间更有价值。日志文件的最新条目在尾部，代码文件的函数实现在尾部，配置文件的最后一个 section 往往是你刚改的那个。只截头部会丢掉这些关键信息。

截断看起来是个小功能，但它直接决定了 Agent 能走多远。没有截断的 Agent，一次 read_file 返回 5 万字符，上下文窗口直接被吃掉一大半。后面的推理、工具调用全部挤在剩余空间里，质量断崖式下降。

这就是为什么 Anthropic 在 Context Engineering 博客 里把「工具结果管理」列为 Agent 开发的核心挑战之一。

并发控制：读写锁模式

当模型在一次回复里调用多个工具时，AI SDK 会并发执行所有带 execute 的工具。同时查北京和上海的天气，没问题。但如果一个 write_file 和一个 read_file 同时跑，就可能出问题——你还没写完文件，另一个工具就开始读了。

我们之前在 ToolDefinition 留了个 isConcurrencySafe 字段一直没用，现在派上用场。基本思路是经典的读写锁：

只读工具（isConcurrencySafe: true）→ 获取共享锁，可以和其他只读工具同时持有
读写工具（isConcurrencySafe: false）→ 获取独占锁，必须等所有其他工具执行完才能开始

升级 tool-registry.ts，在锁字段、锁方法、toAISDKFormat() 的 execute 包装层三处加上锁逻辑：

src/tools/registry.ts
应用
复制
import { jsonSchema } from 'ai';

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  isConcurrencySafe?: boolean;
  isReadOnly?: boolean;
  maxResultChars?: number;
  execute: (input: any) => Promise<unknown>;
}

const DEFAULT_MAX_RESULT_CHARS = 3000;

export class ToolRegistry {
  private tools = new Map<string, ToolDefinition>();

  // 三个状态变量构成一把读写锁
  private exclusiveLock = false;          // 当前是否有独占锁持有者
  private concurrentCount = 0;            // 当前共享锁持有数
  private waitQueue: Array<() => void> = [];  // 阻塞等待中的 resolve 函数

  register(...tools: ToolDefinition[]): void {
    for (const tool of tools) {
      this.tools.set(tool.name, tool);
    }
  }

  get(name: string): ToolDefinition | undefined {
    return this.tools.get(name);
  }

  getAll(): ToolDefinition[] {
    return Array.from(this.tools.values());
  }

  // 获取共享锁：只要没人独占就能拿，多个只读工具可以同时持有
  private async acquireConcurrent(): Promise<void> {
    while (this.exclusiveLock) {
      await new Promise<void>(r => this.waitQueue.push(r));
    }
    this.concurrentCount++;
  }

  private releaseConcurrent(): void {
    this.concurrentCount--;
    if (this.concurrentCount === 0) this.drainQueue();
  }

  // 获取独占锁：必须等所有共享锁释放、且没人持独占
  private async acquireExclusive(): Promise<void> {
    while (this.exclusiveLock || this.concurrentCount > 0) {
      await new Promise<void>(r => this.waitQueue.push(r));
    }
    this.exclusiveLock = true;
  }

  private releaseExclusive(): void {
    this.exclusiveLock = false;
    this.drainQueue();
  }

  // 锁释放时把等待队列全唤醒，让它们重新去抢锁
  private drainQueue(): void {
    const waiting = this.waitQueue.splice(0);
    for (const resolve of waiting) resolve();
  }

  toAISDKFormat(): Record<string, any> {
    const result: Record<string, any> = {};
    for (const [name, tool] of this.tools) {
      const maxChars = tool.maxResultChars;
      const executeFn = tool.execute;
      const isSafe = tool.isConcurrencySafe === true;
      const registry = this;

      result[name] = {
        description: tool.description,
        inputSchema: jsonSchema(tool.parameters as any),
        execute: async (input: any) => {
          // 在真正执行前先按 isConcurrencySafe 获取锁
          if (isSafe) {
            await registry.acquireConcurrent();
            console.log(`  [并发] ${name} 获取共享锁`);
          } else {
            await registry.acquireExclusive();
            console.log(`  [串行] ${name} 获取独占锁，等待其他工具完成`);
          }
          try {
            const raw = await executeFn(input);
            const text = typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2);
            return truncateResult(text, maxChars);
          } finally {
            // 不管成功还是抛异常，锁都要释放
            if (isSafe) {
              registry.releaseConcurrent();
            } else {
              registry.releaseExclusive();
            }
          }
        },
      };
    }
    return result;
  }
}

export function truncateResult(text: string, maxChars: number = DEFAULT_MAX_RESULT_CHARS): string {
  if (text.length <= maxChars) return text;

  const headSize = Math.floor(maxChars * 0.6);
  const tailSize = maxChars - headSize;
  const head = text.slice(0, headSize);
  const tail = text.slice(-tailSize);
  const dropped = text.length - headSize - tailSize;

  return `${head}\n\n... [省略 ${dropped} 字符] ...\n\n${tail}`;
}


锁的核心是那个 waitQueue——拿不到锁的工具不是轮询自旋，而是把自己的 resolve 推进队列然后挂起；等持有者释放锁时，drainQueue() 一次唤醒所有等待者，让它们重新抢锁。这套机制平时跑很多只读工具时近乎零开销（直接 concurrentCount++），只有真出现读写竞争才会切到等待路径。

finally 释放锁是关键——不管 executeFn 成功还是抛异常，锁都得还回去，不然整个 Registry 就锁死了。

这样 AI SDK 虽然同时触发了所有 execute 调用，但我们的锁机制保证了正确的执行顺序。Agent Loop 完全不需要感知并发细节。

输入「测试并发」看看效果：

bash
运行
复制
pnpm start

You: 测试并发

--- Step 1 ---
  [并发] get_weather 获取共享锁
  [调用: get_weather({"city":"北京"})]
  [结果: get_weather] 晴，15-25°C，东南风 2 级
  [并发] get_weather 获取共享锁
  [调用: get_weather({"city":"上海"})]
  [结果: get_weather] 多云，18-22°C，西南风 3 级
  [并发] list_directory 获取共享锁
  [调用: list_directory({"path":"."})]
  [结果: list_directory] [FILE] .env  [DIR] node_modules ...
  → 继续下一步...

--- Step 2 ---
查询到多个城市的天气：
- 晴，15-25°C，东南风 2 级
- 多云，18-22°C，西南风 3 级


三个只读工具都拿到了共享锁，并行执行。如果其中混了一个 write_file，它会等前面的只读工具都完成后再独占执行，后面的只读工具又要等它结束——这就是读写锁保证的因果一致性。

顺便提一句：虽然协议层靠 toolCallId 匹配结果和调用，乱序返回技术上没问题，但实践中建议按调用顺序组织结果。原因不是模型会搞混，而是日志排查和行为确定性——异步完成顺序每次可能不同，按调用顺序返回让整个执行过程可预测、可复现。

到这里，你的 Agent 有了一个像样的工具系统。回头看看我们做的事情：

ToolRegistry 解耦了工具定义和使用——注册一次，Agent Loop 和 AI SDK 都能用。加新工具只需要写一个 ToolDefinition 对象，调一下 registry.register()，不用改 Agent Loop 的任何代码。

结果截断 是上下文工程的第一道防线——不让单个工具的输出吃掉整个推理空间。Head/Tail 60/40 分割比简单截头更聪明，保留了文件两端的关键信息。

读写锁并发控制 让只读工具并行跑、读写工具独占执行，Agent Loop 完全不需要感知并发细节。

而且上一篇的三层防护（循环检测、API 容错、Token 预算）完全没动——它们在更外层保护 Agent Loop，和工具系统互不干扰。这就是分层设计的好处。

下一篇接 MCP——让你的 Agent 不仅能用自己的内置工具，还能连接外部的 MCP Server，调用 GitHub、数据库这些你自己没写过的工具。到时候 ToolRegistry 会多一个 registerMCPTools() 方法，MCP 工具和内置工具共享同一套截断和执行策略。

参考链接
Vercel AI SDK - Tool Reference
Anthropic - Advanced Tool Use
OpenAI - Function Calling Best Practices
Anthropic - Effective Context Engineering for AI Agents
上一篇
Agent 不能这么脆——循环检测、API 容错与 Token 预算
下一篇 · 第二章：Tool System
补齐装备——edit_file、grep、glob 与 bash
编辑器


---
## 代码块


```
You: 帮我看一下当前目录有哪些文件
Agent: 当前目录下通常会有 src、package.json、node_modules……
```


```
You: 帮我读一下 package.json
Agent: 你可以用 cat package.json 查看……
```


```bash
pnpm install
```


```typescript
interface ToolDefinition {
  name: string;
  description: string;        // 给模型看的描述
  parameters: Record<string, unknown>;  // JSON Schema
  execute: (input: any) => Promise<unknown>;

  // 元数据——给 Agent Loop 做决策用
  isConcurrencySafe?: boolean;  // 能否并行
  isReadOnly?: boolean;         // 是否只读
  maxResultChars?: number;      // 结果最大长度
}
```


```typescript
import { jsonSchema } from 'ai';

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  isConcurrencySafe?: boolean;
  isReadOnly?: boolean;
  maxResultChars?: number;
  execute: (input: any) => Promise<unknown>;
}

const DEFAULT_MAX_RESULT_CHARS = 3000;

export class ToolRegistry {
  private tools = new Map<string, ToolDefinition>();

  register(...tools: ToolDefinition[]): void {
    for (const tool of tools) {
      this.tools.set(tool.name, tool);
    }
  }

  get(name: string): ToolDefinition | undefined {
    return this.tools.get(name);
  }

  getAll(): ToolDefinition[] {
    return Array.from(this.tools.values());
  }

  toAISDKFormat(): Record<string, any> {
    const result: Record<string, any> = {};
    for (const [name, tool] of this.tools) {
      const maxChars = tool.maxResultChars;
      const executeFn = tool.execute;
      result[name] = {
        description: tool.description,
        inputSchema: jsonSchema(tool.parameters as any),
        execute: async (input: any) => {
          const raw = await executeFn(input);
          const text = typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2);
          return truncateResult(text, maxChars);
        },
      };
    }
    return result;
  }
}

export function truncateResult(text: string, maxChars: number = DEFAULT_MAX_RESULT_CHARS): string {
  if (text.length <= maxChars) return text;

  const headSize = Math.floor(maxChars * 0.6);
  const tailSize = maxChars - headSize;
  const head = text.slice(0, headSize);
  const tail = text.slice(-tailSize);
  const dropped = text.length - headSize - tailSize;

  return `${head}\n\n... [省略 ${dropped} 字符] ...\n\n${tail}`;
}
```


```typescript
import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { ToolDefinition } from './tool-registry.js';

export const weatherTool: ToolDefinition = {
  name: 'get_weather',
  description: '查询指定城市的天气信息',
  parameters: {
    type: 'object',
    properties: {
      city: { type: 'string', description: '城市名称，如"北京"、"上海"' },
    },
    required: ['city'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ city }: { city: string }) => {
    const data: Record<string, string> = {
      '北京': '晴，15-25°C，东南风 2 级',
      '上海': '多云，18-22°C，西南风 3 级',
      '深圳': '阵雨，22-28°C，南风 2 级',
    };
    return data[city] || `${city}：暂无数据`;
  },
};

export const calculatorTool: ToolDefinition = {
  // ... 同前，省略 ...
};

export const readFileTool: ToolDefinition = {
  name: 'read_file',
  description: '读取指定路径的文件内容',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
    },
    required: ['path'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 500,  // 演示用，生产环境通常 50000+
  execute: async ({ path }: { path: string }) => {
    return readFileSync(resolve(path), 'utf-8');
  },
};

export const writeFileTool: ToolDefinition = {
  name: 'write_file',
  description: '写入内容到指定文件',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '文件路径' },
      content: { type: 'string', description: '要写入的内容' },
    },
    required: ['path', 'content'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,  // 写操作不能并行
  isReadOnly: false,
  execute: async ({ path, content }: { path: string; content: string }) => {
    writeFileSync(resolve(path), content, 'utf-8');
    return `已写入 ${content.length} 字符到 ${path}`;
  },
};

export const listDirectoryTool: ToolDefinition = {
  name: 'list_directory',
  description: '列出指定目录下的文件和子目录',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: '目录路径，默认为当前目录' },
    },
    required: [],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ path = '.' }: { path?: string }) => {
    const resolved = resolve(path);
    return readdirSync(resolved).map(name => {
      const stat = statSync(join(resolved, name));
      return `${stat.isDirectory() ? '[DIR]' : '[FILE]'} ${name}`;
    }).join('\n');
  },
};

export const allTools: ToolDefinition[] = [
  weatherTool, calculatorTool, readFileTool, writeFileTool, listDirectoryTool,
];
```


```typescript
import 'dotenv/config';
import { type ModelMessage } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';
import { createMockModel } from './mock-model.js';
import { createInterface } from 'node:readline';
import { ToolRegistry } from './tool-registry.js';
import { allTools } from './tools.js';
import { agentLoop } from './agent-loop.js';

const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});

const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();

const registry = new ToolRegistry();
registry.register(...allTools);

console.log(`已注册 ${registry.getAll().length} 个工具：`);
for (const tool of registry.getAll()) {
  const flags = [
    tool.isConcurrencySafe ? '可并发' : '串行',
    tool.isReadOnly ? '只读' : '读写',
  ].join(', ');
  console.log(`  - ${tool.name}（${flags}）`);
}

// ... readline + agentLoop 调用，结构同上一篇 ...
```


```typescript
import { streamText, type ModelMessage } from 'ai';
import { ToolRegistry } from './tool-registry.js';
import { detect, recordCall, recordResult, resetHistory } from './loop-detection.js';
import { isRetryable, calculateDelay, sleep } from './retry.js';

const MAX_STEPS = 15;
const MAX_RETRIES = 3;
const TOKEN_BUDGET = 50000;

export async function agentLoop(
  model: any,
  registry: ToolRegistry,
  messages: ModelMessage[],
  system: string,
) {
  // ... 核心结构同上一篇 ...
  // 关键变化：tools 参数改为 registry.toAISDKFormat()
  // tool-result 输出加了预览截断（超 120 字符只显示前 120）

  const result = streamText({
    model,
    system,
    tools: registry.toAISDKFormat(), // ← 这里
    messages,
    maxRetries: 0,
    onError: () => {},
  });
  // ... 其余处理逻辑同上一篇 ...
}
```


```bash
pnpm start
```


```
You: 帮我看看当前目录有什么文件

--- Step 1 ---
  [调用: list_directory({"path":"."})]
  [结果: list_directory] [FILE] .env
[DIR] node_modules
[FILE] package.json
[FILE] sample-data.txt
[DIR] src
[FILE] tsconfig.json
  → 继续下一步...

--- Step 2 ---
当前目录的文件列表：
[FILE] .env  [DIR] node_modules  [FILE] package.json ...
```


```
You: 读取 package.json

--- Step 1 ---
  [调用: read_file({"path":"package.json"})]
  [结果: read_file] { "name": "super-agent-04-tool-system", ...
```


```
You: 测试截断

--- Step 1 ---
  [调用: read_file({"path":"sample-data.txt"})]
  [结果: read_file] Super Agent 工具系统设计文档
=============================

一、工具注册机制
每个工具通过 ToolRegistry 统一注册...

... [省略 538 字符] ...

...错误信息要对模型友好——模型需要理解为什么失败才能换策略
4. 结果格式要结构化——JSON 比自然语言更容易被模型准确解析
```


```typescript
import { jsonSchema } from 'ai';

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  isConcurrencySafe?: boolean;
  isReadOnly?: boolean;
  maxResultChars?: number;
  execute: (input: any) => Promise<unknown>;
}

const DEFAULT_MAX_RESULT_CHARS = 3000;

export class ToolRegistry {
  private tools = new Map<string, ToolDefinition>();

  // 三个状态变量构成一把读写锁
  private exclusiveLock = false;          // 当前是否有独占锁持有者
  private concurrentCount = 0;            // 当前共享锁持有数
  private waitQueue: Array<() => void> = [];  // 阻塞等待中的 resolve 函数

  register(...tools: ToolDefinition[]): void {
    for (const tool of tools) {
      this.tools.set(tool.name, tool);
    }
  }

  get(name: string): ToolDefinition | undefined {
    return this.tools.get(name);
  }

  getAll(): ToolDefinition[] {
    return Array.from(this.tools.values());
  }

  // 获取共享锁：只要没人独占就能拿，多个只读工具可以同时持有
  private async acquireConcurrent(): Promise<void> {
    while (this.exclusiveLock) {
      await new Promise<void>(r => this.waitQueue.push(r));
    }
    this.concurrentCount++;
  }

  private releaseConcurrent(): void {
    this.concurrentCount--;
    if (this.concurrentCount === 0) this.drainQueue();
  }

  // 获取独占锁：必须等所有共享锁释放、且没人持独占
  private async acquireExclusive(): Promise<void> {
    while (this.exclusiveLock || this.concurrentCount > 0) {
      await new Promise<void>(r => this.waitQueue.push(r));
    }
    this.exclusiveLock = true;
  }

  private releaseExclusive(): void {
    this.exclusiveLock = false;
    this.drainQueue();
  }

  // 锁释放时把等待队列全唤醒，让它们重新去抢锁
  private drainQueue(): void {
    const waiting = this.waitQueue.splice(0);
    for (const resolve of waiting) resolve();
  }

  toAISDKFormat(): Record<string, any> {
    const result: Record<string, any> = {};
    for (const [name, tool] of this.tools) {
      const maxChars = tool.maxResultChars;
      const executeFn = tool.execute;
      const isSafe = tool.isConcurrencySafe === true;
      const registry = this;

      result[name] = {
        description: tool.description,
        inputSchema: jsonSchema(tool.parameters as any),
        execute: async (input: any) => {
          // 在真正执行前先按 isConcurrencySafe 获取锁
          if (isSafe) {
            await registry.acquireConcurrent();
            console.log(`  [并发] ${name} 获取共享锁`);
          } else {
            await registry.acquireExclusive();
            console.log(`  [串行] ${name} 获取独占锁，等待其他工具完成`);
          }
          try {
            const raw = await executeFn(input);
            const text = typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2);
            return truncateResult(text, maxChars);
          } finally {
            // 不管成功还是抛异常，锁都要释放
            if (isSafe) {
              registry.releaseConcurrent();
            } else {
              registry.releaseExclusive();
            }
          }
        },
      };
    }
    return result;
  }
}

export function truncateResult(text: string, maxChars: number = DEFAULT_MAX_RESULT_CHARS): string {
  if (text.length <= maxChars) return text;

  const headSize = Math.floor(maxChars * 0.6);
  const tailSize = maxChars - headSize;
  const head = text.slice(0, headSize);
  const tail = text.slice(-tailSize);
  const dropped = text.length - headSize - tailSize;

  return `${head}\n\n... [省略 ${dropped} 字符] ...\n\n${tail}`;
}
```


```bash
pnpm start
```


```
You: 测试并发

--- Step 1 ---
  [并发] get_weather 获取共享锁
  [调用: get_weather({"city":"北京"})]
  [结果: get_weather] 晴，15-25°C，东南风 2 级
  [并发] get_weather 获取共享锁
  [调用: get_weather({"city":"上海"})]
  [结果: get_weather] 多云，18-22°C，西南风 3 级
  [并发] list_directory 获取共享锁
  [调用: list_directory({"path":"."})]
  [结果: list_directory] [FILE] .env  [DIR] node_modules ...
  → 继续下一步...

--- Step 2 ---
查询到多个城市的天气：
- 晴，15-25°C，东南风 2 级
- 多云，18-22°C，西南风 3 级
```
