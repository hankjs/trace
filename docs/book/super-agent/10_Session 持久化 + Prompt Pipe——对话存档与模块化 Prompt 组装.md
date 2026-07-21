# Session 持久化 + Prompt Pipe——对话存档与模块化 Prompt 组装

课程
Super Agent 实战课
Session 持久化 + Prompt Pipe——对话存档与模块化 Prompt 组装
Session 持久化 + Prompt Pipe——对话存档与模块化 Prompt 组装

约 22 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

你跟 Agent 聊了 10 轮，帮你查了 GitHub Issue、读了几个文件、编辑了代码，聊得很顺利。然后你关掉终端去吃饭，回来重新 pnpm start——对话没了，Agent 不认识你了，之前聊的全部归零。

这个问题在之前的几篇里一直存在，因为我们的对话历史只活在内存里。进程一退出，messages 数组就没了。

同时还有另一个问题：现在的 system prompt 是一个写死的字符串常量。功能越加越多——工具描述、延迟工具列表、会话信息——全部硬编码在一个模板字符串里，改一个地方要小心翼翼避免影响其他部分。

这篇我们解决这两个问题。Session 持久化让对话关掉再打开还在，Prompt Pipe 让 system prompt 变成可维护的模块化系统。

先装依赖：

bash
运行
复制
pnpm install

项目结构调整

从这篇开始，我们把之前平铺在 src/ 下的十几个文件按职责拆进了子目录。随着功能越加越多，一个扁平的 src/ 目录会变得很难维护。调整后的结构如下：

src/
  index.ts              # 入口
  mock-model.ts         # Mock Model
  agent/                # Agent Loop 相关
    loop.ts, loop-detection.ts, retry.ts
  tools/                # 工具系统
    registry.ts, mcp-client.ts
    file-tools.ts, search-tools.ts, shell-tools.ts, utility-tools.ts
    index.ts            # 汇总导出
  session/              # 会话持久化（本篇新增）
    store.ts
  context/              # Prompt 与上下文管理（本篇新增）
    prompt-builder.ts


后面几篇会继续往这个结构里加模块——context/ 下会多出压缩和防御相关的文件，memory/ 和 rag/ 也会在对应章节出现。import 路径变了，但每个模块的代码（除了 import 语句）本身没变。

Session 持久化：JSONL

对话持久化的方案有很多——SQLite、Redis、直接存 JSON 文件。我们选 JSONL（JSON Lines），每行一条 JSON 记录。

为什么不用数据库？三个原因：

Append-only——只往文件末尾追加，不需要事务，写入操作天然是崩溃安全的。写到一半断电了？最多丢最后一行不完整的数据，前面的全部完好
可调试——打开文件就能看到完整的对话历史，不需要查询工具。出了问题 cat .sessions/default.jsonl 一看就知道
零依赖——不需要装数据库驱动，fs.appendFileSync 就搞定了

Claude Code 的对话记录也是 JSONL 格式（它叫 transcript），支持 --continue 和 --resume 恢复历史会话。

创建 src/session-store.ts：

src/session/store.ts
应用
复制
import { existsSync, mkdirSync, readFileSync, appendFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ModelMessage } from 'ai';

const SESSION_DIR = '.sessions';

export interface SessionEntry {
  type: 'message';
  timestamp: string;
  message: ModelMessage;
}

export class SessionStore {
  private dir: string;
  private sessionId: string;

  constructor(sessionId: string = 'default') {
    this.sessionId = sessionId;
    this.dir = SESSION_DIR;
    if (!existsSync(this.dir)) {
      mkdirSync(this.dir, { recursive: true });
    }
  }

  private get filePath(): string {
    return join(this.dir, `${this.sessionId}.jsonl`);
  }

  append(message: ModelMessage): void {
    const entry: SessionEntry = {
      type: 'message',
      timestamp: new Date().toISOString(),
      message,
    };
    appendFileSync(this.filePath, JSON.stringify(entry) + '\n', 'utf-8');
  }

  appendAll(messages: ModelMessage[]): void {
    for (const msg of messages) {
      this.append(msg);
    }
  }

  load(): ModelMessage[] {
    if (!existsSync(this.filePath)) return [];
    const content = readFileSync(this.filePath, 'utf-8').trim();
    if (!content) return [];

    const messages: ModelMessage[] = [];
    for (const line of content.split('\n')) {
      if (!line.trim()) continue;
      try {
        const entry: SessionEntry = JSON.parse(line);
        if (entry.type === 'message') {
          messages.push(entry.message);
        }
      } catch { /* skip malformed lines */ }
    }
    return messages;
  }

  exists(): boolean {
    return existsSync(this.filePath);
  }
}


SessionStore 做的事情非常简单：

append() 把一条消息追加写入 JSONL 文件，每条消息带时间戳
load() 读取文件，逐行解析，还原成 ModelMessage[]
exists() 检查会话文件是否存在

注意 load() 里对解析失败的行直接 skip——这就是 JSONL 的容错性，一行坏了不影响其他行。

入口文件里，用户输入的每条消息在发给模型之前先写入 JSONL，Agent 回复的消息在收到后也追加进去。启动时如果带了 --continue 参数并且有历史文件，就从文件恢复对话：

src/index.ts
应用
复制
// Session 持久化
const isContinue = process.argv.includes('--continue');
const store = new SessionStore('default');

let messages: ModelMessage[] = [];
if (isContinue && store.exists()) {
  messages = store.load();
  console.log(`[Session] 恢复会话，${messages.length} 条历史消息`);
} else {
  console.log(`[Session] 新会话`);
}

// 在 ask() 里：
// 用户消息 → append → 发给模型
// 模型回复 → appendAll → 持久化


跑起来试试：

bash
运行
复制
pnpm start


聊几句后输入 exit 退出，然后用 pnpm run continue 恢复：

bash
运行
复制
pnpm run continue

[Session] 恢复会话，4 条历史消息

=== Prompt Pipe Debug ===
  coreRules: [ON] 107 chars
  toolGuide: [ON] 47 chars
  deferredTools: [ON] 680 chars
  sessionContext: [ON] 31 chars
========================


历史消息恢复了，sessionContext Pipe 也自动激活了（因为检测到有历史消息）。Agent 知道你之前聊过什么，可以接着上次的话题继续。

打开 .sessions/default.jsonl 看一眼内容：

json
复制
{"type":"message","timestamp":"2026-05-01T...","message":{"role":"user","content":"你好"}}
{"type":"message","timestamp":"2026-05-01T...","message":{"role":"assistant","content":[{"type":"text","text":"你好！..."}]}}


每行一条消息，时间戳、角色、内容一目了然。调试的时候直接看这个文件就能还原整个对话过程。

Prompt Pipe：模块化 Prompt 组装

Session 解决了"对话不丢"的问题，现在来解决"prompt 不乱"的问题。

到上一节为止，我们的 system prompt 是这样的：

typescript
复制
const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
你有内置工具和 MCP 工具可用。
如果你需要的工具不在当前列表中，使用 tool_search 工具搜索。
${deferredSummary}`;


但生产环境要加的 prompt 可不止这些，随着功能越来越复杂，我们会加入更多的 prompt 进来，包括一些指令信息、记忆内容、session 上下文等等。

每加一个功能就往这个字符串里塞一段，而且有些 prompt 片段是需要按需添加的，这就导致字符串里面嵌入各种三元表达式，很快就变成 AI 都改不动的巨型屎山代码。

知识体系课讲过 Prompt Pipe 模式——把 system prompt 拆成独立的模块，每个模块是一个纯函数，接收运行时上下文，自己决定要不要出现在最终 prompt 中。

创建 src/prompt-builder.ts：

src/context/prompt-builder.ts
应用
复制
export interface PromptContext {
  toolCount: number;
  deferredToolSummary: string;
  sessionMessageCount: number;
  sessionId: string;
}

type PipeFn = (ctx: PromptContext) => string | null;

export class PromptBuilder {
  private pipes: Array<{ name: string; fn: PipeFn }> = [];

  pipe(name: string, fn: PipeFn): this {
    this.pipes.push({ name, fn });
    return this;
  }

  build(ctx: PromptContext): string {
    const sections: string[] = [];
    for (const { fn } of this.pipes) {
      const result = fn(ctx);
      if (result !== null) {
        sections.push(result);
      }
    }
    return sections.join('\n\n');
  }

  debug(ctx: PromptContext): void {
    console.log('\n=== Prompt Pipe Debug ===');
    for (const { name, fn } of this.pipes) {
      const result = fn(ctx);
      const status = result !== null
        ? `[ON] ${result.length} chars` : '[OFF]';
      console.log(`  ${name}: ${status}`);
    }
    console.log('========================\n');
  }
}


核心就两个方法：

pipe(name, fn) 注册一个 Pipe 模块。fn 返回字符串就加入 prompt，返回 null 就跳过
build(ctx) 按注册顺序调用所有 Pipe，过滤掉 null，拼接成最终 prompt

每个 Pipe 是一个工厂函数，返回实际的 Pipe 函数：

src/context/prompt-builder.ts
应用
复制
export function coreRules(): PipeFn {
  return () => `你是 Super Agent...（核心行为规则）`;
}

export function toolGuide(): PipeFn {
  return (ctx) => {
    if (ctx.toolCount === 0) return null;
    return `你有 ${ctx.toolCount} 个工具可用...`;
  };
}

export function sessionContext(): PipeFn {
  return (ctx) => {
    if (ctx.sessionMessageCount === 0) return null;
    return `[会话信息] 已有 ${ctx.sessionMessageCount} 条历史消息`;
  };
}


sessionContext 这个 Pipe 只有在恢复历史会话时才会出现（sessionMessageCount > 0），新会话时它返回 null，不占任何 prompt 空间。这就是 Pipe 模式的好处——条件逻辑和内容在同一个模块里，加新 section 零摩擦。

在入口文件里用 Builder 链式组装：

src/index.ts
应用
复制
const builder = new PromptBuilder()
  .pipe('coreRules', coreRules())
  .pipe('toolGuide', toolGuide())
  .pipe('deferredTools', deferredTools())
  .pipe('sessionContext', sessionContext());

const SYSTEM = builder.build(promptCtx);
builder.debug(promptCtx);  // 显示各模块状态


debug() 输出让你一眼看到哪些 Pipe 是开的、哪些是关的：

=== Prompt Pipe Debug ===
  coreRules: [ON] 107 chars
  toolGuide: [ON] 47 chars
  deferredTools: [ON] 680 chars
  sessionContext: [OFF]        ← 新会话，没有历史
========================


用 --continue 恢复后，sessionContext 自动变成 [ON]。后面加记忆注入、RAG 上下文，只要写一个新的 Pipe 函数，.pipe('memory', memoryContext()) 加一行就行了。

OK，考虑到很多朋友第一次接触到 Prompt Pipe 这种设计模式，我们在这里再次梳理一下这种模式的好处，帮助你加深印象：

实现 Prompt 模块化管理，避免大文件膨胀，工程上长期可维护。
每个 Prompt 模块内部可以根据环境实现动态加载，比如某个 flag 没开启，那就不加特定的 prompt。
很方便地区分出静态和动态 prompt，一目了然。我们接下来讲讲这个部分。
静态/动态分界线

Pipe 的注册顺序是有讲究的。

知识体系课讲过 KV Cache 的工作原理——prompt 前缀不变，计算结果就能复用。所以 不变的 section 放前面，变的放后面：

coreRules — 永远不变，放最前面，cache 稳稳命中。
toolGuide — 工具数量基本固定，变化很少。
deferredTools — 所有的工具列表也基本固定，放中间。
sessionContext — 每次启动都不同，放最后面。

这个顺序保证了 prompt 前部是稳定的，后面加的动态内容不会杀掉前面的 cache。后面讲 Prompt Cache 那篇会进一步优化这个分界线的位置，但现在先把意识建立起来——先静后动，是 Prompt 组装的基本原则。

到这里，两个核心能力都落地了。Session 持久化让对话跨终端保留，Prompt Pipe 让 system prompt 模块化可维护。但还有一个更大的问题没解决：对话越来越长，消息列表无限增长，迟早会撑爆上下文窗口。下一篇我们来讲怎么压缩。

参考链接
Anthropic - Effective Context Engineering
Anthropic - Prompt Caching
上一篇
工具太多模型选不准——实现 ToolSearch
下一篇 · 第三章：Context Engineering
对话太长了怎么办——Microcompact + LLM 摘要压缩
编辑器


---
## 代码块


```bash
pnpm install
```


```
src/
  index.ts              # 入口
  mock-model.ts         # Mock Model
  agent/                # Agent Loop 相关
    loop.ts, loop-detection.ts, retry.ts
  tools/                # 工具系统
    registry.ts, mcp-client.ts
    file-tools.ts, search-tools.ts, shell-tools.ts, utility-tools.ts
    index.ts            # 汇总导出
  session/              # 会话持久化（本篇新增）
    store.ts
  context/              # Prompt 与上下文管理（本篇新增）
    prompt-builder.ts
```


```typescript
import { existsSync, mkdirSync, readFileSync, appendFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ModelMessage } from 'ai';

const SESSION_DIR = '.sessions';

export interface SessionEntry {
  type: 'message';
  timestamp: string;
  message: ModelMessage;
}

export class SessionStore {
  private dir: string;
  private sessionId: string;

  constructor(sessionId: string = 'default') {
    this.sessionId = sessionId;
    this.dir = SESSION_DIR;
    if (!existsSync(this.dir)) {
      mkdirSync(this.dir, { recursive: true });
    }
  }

  private get filePath(): string {
    return join(this.dir, `${this.sessionId}.jsonl`);
  }

  append(message: ModelMessage): void {
    const entry: SessionEntry = {
      type: 'message',
      timestamp: new Date().toISOString(),
      message,
    };
    appendFileSync(this.filePath, JSON.stringify(entry) + '\n', 'utf-8');
  }

  appendAll(messages: ModelMessage[]): void {
    for (const msg of messages) {
      this.append(msg);
    }
  }

  load(): ModelMessage[] {
    if (!existsSync(this.filePath)) return [];
    const content = readFileSync(this.filePath, 'utf-8').trim();
    if (!content) return [];

    const messages: ModelMessage[] = [];
    for (const line of content.split('\n')) {
      if (!line.trim()) continue;
      try {
        const entry: SessionEntry = JSON.parse(line);
        if (entry.type === 'message') {
          messages.push(entry.message);
        }
      } catch { /* skip malformed lines */ }
    }
    return messages;
  }

  exists(): boolean {
    return existsSync(this.filePath);
  }
}
```


```typescript
// Session 持久化
const isContinue = process.argv.includes('--continue');
const store = new SessionStore('default');

let messages: ModelMessage[] = [];
if (isContinue && store.exists()) {
  messages = store.load();
  console.log(`[Session] 恢复会话，${messages.length} 条历史消息`);
} else {
  console.log(`[Session] 新会话`);
}

// 在 ask() 里：
// 用户消息 → append → 发给模型
// 模型回复 → appendAll → 持久化
```


```bash
pnpm start
```


```bash
pnpm run continue
```


```
[Session] 恢复会话，4 条历史消息

=== Prompt Pipe Debug ===
  coreRules: [ON] 107 chars
  toolGuide: [ON] 47 chars
  deferredTools: [ON] 680 chars
  sessionContext: [ON] 31 chars
========================
```


```json
{"type":"message","timestamp":"2026-05-01T...","message":{"role":"user","content":"你好"}}
{"type":"message","timestamp":"2026-05-01T...","message":{"role":"assistant","content":[{"type":"text","text":"你好！..."}]}}
```


```typescript
const SYSTEM = `你是 Super Agent，一个有工具调用能力的 AI 助手。
你有内置工具和 MCP 工具可用。
如果你需要的工具不在当前列表中，使用 tool_search 工具搜索。
${deferredSummary}`;
```


```typescript
export interface PromptContext {
  toolCount: number;
  deferredToolSummary: string;
  sessionMessageCount: number;
  sessionId: string;
}

type PipeFn = (ctx: PromptContext) => string | null;

export class PromptBuilder {
  private pipes: Array<{ name: string; fn: PipeFn }> = [];

  pipe(name: string, fn: PipeFn): this {
    this.pipes.push({ name, fn });
    return this;
  }

  build(ctx: PromptContext): string {
    const sections: string[] = [];
    for (const { fn } of this.pipes) {
      const result = fn(ctx);
      if (result !== null) {
        sections.push(result);
      }
    }
    return sections.join('\n\n');
  }

  debug(ctx: PromptContext): void {
    console.log('\n=== Prompt Pipe Debug ===');
    for (const { name, fn } of this.pipes) {
      const result = fn(ctx);
      const status = result !== null
        ? `[ON] ${result.length} chars` : '[OFF]';
      console.log(`  ${name}: ${status}`);
    }
    console.log('========================\n');
  }
}
```


```typescript
export function coreRules(): PipeFn {
  return () => `你是 Super Agent...（核心行为规则）`;
}

export function toolGuide(): PipeFn {
  return (ctx) => {
    if (ctx.toolCount === 0) return null;
    return `你有 ${ctx.toolCount} 个工具可用...`;
  };
}

export function sessionContext(): PipeFn {
  return (ctx) => {
    if (ctx.sessionMessageCount === 0) return null;
    return `[会话信息] 已有 ${ctx.sessionMessageCount} 条历史消息`;
  };
}
```


```typescript
const builder = new PromptBuilder()
  .pipe('coreRules', coreRules())
  .pipe('toolGuide', toolGuide())
  .pipe('deferredTools', deferredTools())
  .pipe('sessionContext', sessionContext());

const SYSTEM = builder.build(promptCtx);
builder.debug(promptCtx);  // 显示各模块状态
```


```
=== Prompt Pipe Debug ===
  coreRules: [ON] 107 chars
  toolGuide: [ON] 47 chars
  deferredTools: [ON] 680 chars
  sessionContext: [OFF]        ← 新会话，没有历史
========================
```
