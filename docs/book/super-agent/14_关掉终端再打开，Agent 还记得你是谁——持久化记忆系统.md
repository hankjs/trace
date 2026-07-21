# 关掉终端再打开，Agent 还记得你是谁——持久化记忆系统

课程
Super Agent 实战课
关掉终端再打开，Agent 还记得你是谁——持久化记忆系统
关掉终端再打开，Agent 还记得你是谁——持久化记忆系统

约 46 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇做完 Cache 和成本追踪之后，我们的 Agent 在一次对话里已经很强了——能用工具、能压缩上下文、能省钱。但有一个尴尬的问题：关掉终端，一切归零。

你跟 Agent 说"我喜欢用 TypeScript，别给我写 Python"，它记住了，这轮对话里表现很好。关掉终端，明天重新打开，同样的话你得再说一遍。你告诉它"这个项目的数据库迁移上周出过事故，改表结构要小心"，它理解了，帮你避开了风险。下次开工，它又是一个什么都不知道的新手。

Session 持久化（前面做的 JSONL）解决的是一次对话内的连续性——关掉终端、重新打开，上次聊到哪接着聊。但它本质上是在回放历史消息，不是真正的"记住"。你不可能把几十次对话的完整历史全塞进上下文——token 预算撑不住，信噪比也会很糟糕。

这篇要做的是跨会话记忆：Agent 能从对话中提取值得长期保留的信息，存到文件里，下次开工时自动加载。

先装依赖：

bash
运行
复制
pnpm install

项目结构调整

在加新功能之前，先整理一下代码。

当前小节的初始化代码已经更新了，可以直接在右侧编辑器查看。

src/index.ts 经过前面这么多节的累加已经膨胀了不少——tool 定义、prompt pipe、主循环全堆在一个文件里。趁着这节要加记忆模块，我们做一轮整理。

工具定义拆成工厂函数。 上一篇里 tool_search 直接写在 index.ts 里，现在提到 src/tools/tool-search.ts，导出 createToolSearchTool(registry) 工厂函数。这节新增的 src/tools/memory-tools.ts 也一样——createMemoryTool(memoryStore)。每个工厂接收自己的依赖实例，index.ts 里就变成一行 registry.register(createToolSearchTool(registry))。后面再加 RAG tools、lint 之类的新功能也不用在 index.ts 里堆代码了。

命令系统从冗长的 if/else 代码重构为 dispatcher。 上一篇的 handleQuickTrigger 是一个大函数，所有快捷命令（sim、defend、status、/context、/cache off...）全用 if/else 串在一起。命令越加越长，而且每个命令都要闭包捕获 messages、timestamps、builder 一堆变量。

新的做法是在 src/commands/ 目录下按职责拆分，然后用一个 dispatcher 串起来：

src/commands/index.ts
应用
复制
import type { ModelMessage } from 'ai';
import type { ToolRegistry } from '../tools/registry.js';
import type { PromptBuilder, PromptContext } from '../context/prompt-builder.js';
import type { UsageTracker } from '../usage/tracker.js';
import type { SessionStore } from '../session/store.js';
import type { MemoryStore } from '../memory/store.js';

export interface CommandContext {
  messages: ModelMessage[];
  timestamps: Map<number, number>;
  registry: ToolRegistry;
  builder: PromptBuilder;
  tracker: UsageTracker;
  sessionStore: SessionStore;
  model: any;
  makePromptCtx: () => PromptContext;
  ask: () => void;
  memoryStore?: MemoryStore;
  [key: string]: any;
}

export type CommandHandler = (cmd: string, ctx: CommandContext) => boolean | 'async';

export function createDispatcher(handlers: CommandHandler[]): CommandHandler {
  return (cmd, ctx) => {
    for (const h of handlers) {
      const result = h(cmd, ctx);
      if (result) return result;
    }
    return false;
  };
}


CommandContext 把命令可能用到的所有依赖打包成一个对象——messages、registry、builder、memoryStore——各命令模块只从 context 里取自己需要的东西，不用再闭包捕获外层变量。CommandHandler 返回 boolean | 'async'：true 表示命令已处理（同步），'async' 表示命令是异步的（由命令自己决定何时调用 ctx.ask() 继续 REPL 循环），false 表示不认识这个命令，交给下一个 handler。

createDispatcher 就是个简单的责任链——遍历 handlers，第一个返回真值的就接管。这样每个命令文件只管自己那几条命令：

src/commands/debug.ts — sim、defend、status、cache on/off
src/commands/context.ts — /context、/usage
src/commands/memory.ts — /memory、/memory search（这节新增）

index.ts 里组装一行搞定：

typescript
复制
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
]);


makePromptCtx() 提取为独立函数。 上一篇的 PromptContext 对象是在 main() 入口处构建一次就完事了。但这节加了记忆之后，每轮对话都要重建 system prompt（记忆内容可能变了），所以需要反复构建 PromptContext。我们把它提成一个函数，命令模块（比如 /context 命令要实时显示上下文构成）和主循环都能调用：

typescript
复制
function makePromptCtx(): PromptContext {
  return {
    toolCount: registry.getActiveTools().length,
    deferredToolSummary: registry.getDeferredToolSummary(),
    sessionMessageCount: messages.length,
    sessionId: 'default',
  };
}


Prompt Pipe 相关的 memoryContext 这节直接写在 builder 链里（后面会看到），等下一篇 RAG 再拆文件也不迟。

计算机存储层次的类比

在动手之前，先建立一个直觉。

Agent 的信息管理和计算机的存储层次结构很像。知识体系课里详细讲过这个类比（MemGPT/Letta 是最早把它系统化推广的项目），这里快速回顾一下：

计算机存储	Agent 对应	特征
CPU 寄存器	当前 token 注意力	最快，容量最小
L1/L2 缓存	上下文窗口	快速访问，容量有限
RAM	会话历史	关掉就丢
硬盘/SSD	长期记忆	跨会话，需要检索

前面几篇做的所有事情——Session、Prompt Pipe、Compaction、Defense、Cache——全都在 L1/L2 和 RAM 这两层打转。这篇要做的，就是给 Agent 装一块"硬盘"。

该记什么，不该记什么

做记忆系统最容易犯的错是"什么都存"。看起来信息越多越好，实际上，无差别存储是记忆系统最大的坑。

记忆条目越多，检索噪音越大，注入到上下文里的无关信息越多，模型的推理质量就越差。Mem0 2026 年的报告给了一个数据：33% 的记忆事实在 90 天内变得不准确。存的越多，过期的越多，踩坑的概率越高。

Claude Code 的做法是排除法——先定义什么不该存，剩下的才考虑存：

能从代码推导的不存。项目用什么技术栈、目录结构长什么样、某个函数在哪个文件——grep 一下就知道了，存到记忆里反而会过期。代码改了，记忆还指着老路径，Agent 会基于错误信息做出自信的错误决策。

能从 git 推导的不存。谁改了什么、上次发布是什么时候——git log 是权威来源。记忆里存的是快照，git 里的是实时数据。

文档里已经有的不存。CLAUDE.md（或你项目里的任何配置文件）里写过的规则，不需要再存一份到记忆里。

只存"只存在于对话中、无法从其他地方获取"的信息。 用户的偏好、纠正反馈、项目决策的背景原因、外部资源的位置——这些信息只在对话里出现过一次，如果不存下来，就永远丢了。

四种记忆类型

知道了"什么不该存"，接下来的问题是"该存的怎么分类"。Claude Code 把记忆分成四种类型，每种类型有不同的触发时机和使用方式：

user（用户画像）——关于用户是谁的信息。角色、偏好、技术背景、工作习惯。"用户是后端工程师，Go 写了十年但第一次碰 React"——有了这条记忆，解释前端概念时就能用后端类比，而不是从零讲起。

feedback（行为反馈）——用户对 Agent 行为的纠正和确认。"不要在测试里 mock 数据库"、"单个大 PR 比拆成多个小 PR 好"。这类记忆最重要，因为它直接影响 Agent 的行为模式。纠正的内容和后续确认结果的内容都要存——只存纠正会让 Agent 越来越保守，因为它只知道什么不该做，不知道什么做法被验证过了。

project（项目动态）——进行中的工作、决策、截止日期。"下周四之前冻结非核心合并，移动端要切分支"。这类记忆衰减最快，过了截止日期就没用了。存的时候要把相对日期转成绝对日期——"下周四"存成 "2026-05-07"，不然一个月后看到"下周四"，完全不知道指的是哪一天。

reference（外部资源）——指向外部系统的一个渠道。"bug 跟踪在 Github 看板的 xxx 栏目里"、"oncall 看 Grafana 的 api-latency 面板"。这类记忆帮 Agent 知道去哪里找信息。

来，我们把这套分类实现出来。

实现 MemoryStore

新建 src/memory-store.ts，这是整个记忆系统的存储层。设计思路是 MEMORY.md 索引 + 独立文件：

src/memory/store.ts
应用
复制
import fs from 'node:fs';
import path from 'node:path';

export interface MemoryEntry {
  name: string;
  description: string;
  type: 'user' | 'feedback' | 'project' | 'reference';
  content: string;
  filePath: string;
}

const MEMORY_DIR = '.memory';
const INDEX_FILE = 'MEMORY.md';
const MAX_INDEX_LINES = 200;
const MAX_FILE_CHARS = 4000;

export class MemoryStore {
  private readonly baseDir: string;

  constructor(baseDir: string = '.') {
    this.baseDir = baseDir;
  }

  private get memoryDir(): string {
    return path.join(this.baseDir, MEMORY_DIR);
  }

  private get indexPath(): string {
    return path.join(this.memoryDir, INDEX_FILE);
  }

  init(): void {
    if (!fs.existsSync(this.memoryDir)) {
      fs.mkdirSync(this.memoryDir, { recursive: true });
    }
    if (!fs.existsSync(this.indexPath)) {
      fs.writeFileSync(this.indexPath, '# Memory Index\n', 'utf-8');
    }
  }
}


有两个硬性约束值得关注：

MAX_INDEX_LINES = 200——索引文件最多 200 行。200 行上限迫使 Agent 只保留真正重要的信息，低价值的记忆自然被淘汰。Claude Code 用的也是这个数字。

MAX_FILE_CHARS = 4000——单个记忆文件最多 4000 字符。读取时如果超限就截断，防止一条记忆把上下文预算吃光。

每个记忆文件用 YAML frontmatter 格式存储：

markdown
复制
---
name: 用户偏好 TypeScript
description: 用户偏好 TypeScript，不喜欢 Python
type: user
---

用户明确表示偏好 TypeScript，在需要写示例代码时优先用 TypeScript。


description 字段不是装饰——后续做记忆检索时，就是根据这个字段判断"这条记忆跟当前对话有没有关"。写得越精确，检索质量越高。

现在加上存储和检索方法：

src/memory/store.ts
应用
复制
  save(entry: Omit<MemoryEntry, 'filePath'>): string {
    this.init();
    const slug = entry.name
      .toLowerCase()
      .replace(/[^a-z0-9一-鿿]+/g, '-')
      .replace(/^-|-$/g, '');
    const filename = `${entry.type}_${slug}.md`;
    const filePath = path.join(this.memoryDir, filename);

    const fileContent = [
      '---',
      `name: ${entry.name}`,
      `description: ${entry.description}`,
      `type: ${entry.type}`,
      '---',
      '',
      entry.content,
    ].join('\n');

    fs.writeFileSync(filePath, fileContent, 'utf-8');
    this.updateIndex(entry.name, filename, entry.description);
    return filename;
  }

  list(): MemoryEntry[] {
    this.init();
    const files = fs.readdirSync(this.memoryDir)
      .filter(f => f.endsWith('.md') && f !== INDEX_FILE);
    // ... parse frontmatter for each file
  }

  search(query: string): MemoryEntry[] {
    const all = this.list();
    const keywords = query.toLowerCase().split(/\s+/);
    return all.filter(entry => {
      const text = `${entry.name} ${entry.description} ${entry.content}`.toLowerCase();
      return keywords.some(kw => text.includes(kw));
    });
  }


save 做两件事：写文件 + 更新索引。文件名格式是 {type}_{slug}.md，类型前缀方便人眼扫描。索引更新时会检查是否已存在同名条目——已存在就覆盖，不存在就追加。如果索引达到 200 行上限，移除最早的条目腾出位置。

search 目前是关键词匹配，够用了。下一篇做 RAG 时会升级成向量检索 + BM25 混合搜索。

bash
运行
复制
pnpm start

memory 工具：让 Agent 自己管理记忆

MemoryStore 是底层存储，但 Agent 不能直接调用 TypeScript 方法。它需要一个工具来操作记忆——跟读文件需要 read_file 工具一样。

新建 src/tools/memory-tools.ts，用工厂函数封装（跟前面 createToolSearchTool 一个模式）：

src/tools/memory-tools.ts
应用
复制
export function createMemoryTool(memoryStore: MemoryStore): ToolDefinition {
  return {
    name: 'memory',
  description: '管理跨会话记忆。action: save（保存）| list（列表）| search（搜索）| read（读取）| delete（删除）',
  parameters: {
    type: 'object',
    properties: {
      action: { type: 'string', enum: ['save', 'list', 'search', 'read', 'delete'] },
      name: { type: 'string', description: '记忆名称（save 时必填）' },
      description: { type: 'string', description: '一句话描述（save 时必填）' },
      type: { type: 'string', enum: ['user', 'feedback', 'project', 'reference'] },
      content: { type: 'string', description: '记忆内容（save 时必填）' },
      query: { type: 'string', description: '搜索关键词（search 时必填）' },
      filename: { type: 'string', description: '文件名（read/delete 时必填）' },
    },
    required: ['action'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async (args: any) => {
    switch (args.action) {
      case 'save': {
        if (!args.name || !args.type || !args.content) {
          return '保存失败：需要 name、type、content 参数';
        }
        const filename = memoryStore.save({
          name: args.name,
          description: args.description || args.name,
          type: args.type,
          content: args.content,
        });
        return `已保存到记忆: ${filename}`;
      }
      case 'list': {
        const entries = memoryStore.list();
        if (entries.length === 0) return '当前没有存储任何记忆。';
        return `记忆列表（共 ${entries.length} 条记忆）：\n` +
          entries.map(e => `  [${e.type}] ${e.name} — ${e.description}`).join('\n');
      }
      case 'search': {
        const results = memoryStore.search(args.query || '');
        if (results.length === 0) return `没有找到与 "${args.query}" 相关的记忆。`;
        return `搜索结果（${results.length} 条匹配）：\n` +
          results.map(e => `  [${e.type}] ${e.name} — ${e.description}`).join('\n');
      }
      // ... read / delete 类似
    }
  },
  };
}


五种操作，一个工具。这是一个常见的设计模式：把相关操作合并成一个工具，用 action 字段区分，而不是拆成 memory_save、memory_list、memory_search 五个独立工具。原因是工具列表的每一项都占用上下文预算——5 个工具就是 5 份 description + parameters。合并成一个，token 开销只有一份。

跑起来试试：

bash
运行
复制
pnpm start


输入 记住我喜欢用 TypeScript，Agent 会调用 memory 工具的 save 操作：

You: 记住我喜欢用 TypeScript
--- Step 1 ---
  [调用: memory({"action":"save","name":"我喜欢 TypeScript","type":"user","content":"我喜欢 TypeScript"})]
  [结果: memory] 已保存到记忆: user_我喜欢-typescript.md
--- Step 2 ---
好的，我已经把这条信息存到记忆里了。下次你重新打开对话，我还会记得这件事。


然后输入 我的记忆，Agent 调用 list：

You: 我的记忆
--- Step 1 ---
  [调用: memory({"action":"list"})]
  [结果: memory] 记忆列表（共 1 条记忆）：
    [user] 我喜欢 TypeScript — 我喜欢 TypeScript
--- Step 2 ---
这是你目前的记忆：
  [user] 我喜欢 TypeScript


关掉终端，重新 pnpm start，启动时你会看到：

  已加载 1 条历史记忆


这说明 Agent 还记得你。

记忆注入：接上 Prompt Pipe

记忆存下来了，但 Agent 怎么"看到"这些记忆？答案是通过 Prompt Pipe 注入到 system prompt 里。

前面做 Prompt Pipe 时讲过"先静后动"原则——不变的 section 放前面利于 cache 命中，变化的放后面。记忆内容在两轮对话之间可能变化（用户可能存了新记忆），但在一轮对话内是稳定的。所以 memoryContext pipe 放在 coreRules 和 toolGuide 之后、sessionContext 之前。

在 src/index.ts 里加一行 pipe 就行——就是个闭包，不值得单独开文件（到下一节加 RAG 时会有第二个 pipe，到时候再拆出去）：

src/index.ts
应用
复制
const builder = new PromptBuilder()
  .pipe('coreRules', coreRules())
  .pipe('toolGuide', toolGuide())
  .pipe('deferredTools', deferredTools())
  .pipe('memoryContext', () => memoryStore.buildPromptSection())   // 记忆注入
  .pipe('sessionContext', sessionContext());


buildPromptSection 把索引内容注入 system prompt，同时附带一条关键提示：

src/memory/store.ts
应用
复制
  buildPromptSection(): string {
    this.init();
    const index = this.loadIndex();
    const entries = this.list();

    if (entries.length === 0) {
      return '[记忆系统] 当前没有存储任何记忆。你可以使用 memory 工具来保存重要信息。';
    }

    const lines = [
      `[记忆系统] 共 ${entries.length} 条记忆`,
      '',
      '记忆索引：',
      index,
      '',
      '使用 memory 工具的 read 操作来读取具体记忆内容。',
      '记忆是线索，不是事实——使用前先验证其准确性。',
    ];
    return lines.join('\n');
  }


注意最后一行——"记忆是线索，不是事实"。这个原则很重要。

比如一条记忆里面说"项目用 MySQL"，但可能上个月已经迁移到 PostgreSQL 了。一条记忆说"formatUser 函数在 utils.ts 第 42 行"，但代码可能重构过了。记忆记录的是写入那一刻的事实，不是当前的事实。 Agent 在使用记忆之前，应该先用工具验证——文件还在不在、函数还叫不叫这个名字。

Claude Code 的实现里，对超过一天的记忆会自动附加一个提醒："claims about code behavior or file:line citations may be outdated."我们也可以借鉴这个做法。

用 /context 命令看看记忆在上下文里占了多少空间：

bash
运行
复制
pnpm start

You: /context
  ● ● ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    Mock Model
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    1.8k/1.0M tokens (0.2%)
  ...
                                       ● System prompt: 1.1k (0.1%)
                                       ● System tools:  600 (0.1%)
                                       ● Memory:        80 (0.0%)
                                       ○ Free space:   948k (94.8%)


记忆索引只占几十个 token。但随着记忆条目增多，这个数字会涨。200 行索引大约 3000-5000 字符，折合 1000-1500 token——并不算多，但也不能无视。这就是为什么要有 200 行上限和 4000 字符/文件上限。

用 system prompt 重建每轮的记忆上下文

有一个容易忽略的细节：每轮对话调 API 时，system prompt 应该包含最新的记忆内容。如果用户在第 3 轮存了一条记忆，第 4 轮的 system prompt 里就应该能看到它。

在 index.ts 的 ask 函数里，每次发消息前调用前面提取的 makePromptCtx() 重新 build system prompt：

src/index.ts
应用
复制
      // Rebuild system prompt with latest memory
      const currentSystem = builder.build(makePromptCtx());

      await agentLoop(model, registry, messages, currentSystem, tracker);


上一篇 PromptContext 只在启动时构建一次（因为没有记忆，system prompt 不会变）。这节加了记忆之后，每轮都要 rebuild——makePromptCtx() 在这里派上了用场。新存的记忆立即可用，删掉的记忆立即消失。Cache 的角度看，因为记忆变化了 system prompt 也变了，这一轮的前缀会 miss。但记忆变化的频率远低于对话频率（大部分轮次记忆不变），所以整体 cache 命中率影响不大。

快捷命令：终端里管理记忆

跟前面几篇一样，给用户几个快捷命令。前面的项目结构调整里我们把命令拆到了 src/commands/ 目录，记忆相关的命令放在 src/commands/memory.ts：

src/commands/memory.ts
应用
复制
import type { CommandHandler } from './index.js';

export const memoryCommands: CommandHandler[] = [
  (cmd, ctx) => {
    if (cmd !== '/memory' && cmd !== 'memory') return false;
    const entries = ctx.memoryStore!.list();
    console.log(`\n[记忆系统] 共 ${entries.length} 条记忆`);
    for (const e of entries) console.log(`  [${e.type}] ${e.name} — ${e.description}`);
    console.log('');
    return true;
  },

  (cmd, ctx) => {
    if (!cmd.startsWith('/memory search ')) return false;
    const query = cmd.slice('/memory search '.length).trim();
    const results = ctx.memoryStore!.search(query);
    console.log(`\n[记忆搜索] "${query}" → ${results.length} 条结果`);
    for (const e of results) console.log(`  [${e.type}] ${e.name} — ${e.description}`);
    console.log('');
    return true;
  },
];


完整的命令列表：

命令	作用
/memory	列出所有记忆
/memory search 关键词	搜索记忆
status	显示消息数、token 数和记忆数
/context	看记忆在 context 里占多少

跑起来试一下完整流程：

bash
运行
复制
pnpm start

You: 记住我喜欢用 TypeScript
  [调用: memory] 已保存到记忆: user_我喜欢-typescript.md

You: 记住这个项目的数据库上周出过事故，改表结构要谨慎
  [调用: memory] 已保存到记忆: project_数据库上周出过事故.md

You: /memory
[记忆系统] 共 2 条记忆
  [user] 我喜欢 TypeScript — 用户偏好 TypeScript
  [project] 数据库事故 — 数据库上周出过事故，改表结构要谨慎

You: /memory search 数据库
[记忆搜索] "数据库" → 1 条结果
  [project] 数据库事故 — 数据库上周出过事故，改表结构要谨慎

You: status
[状态] 4 条消息 (2 条工具结果), ~85 tokens, 2 条记忆


关掉终端，重新 pnpm start：

Super Agent v0.11 — Memory System
  已加载 2 条历史记忆


记忆跨会话生效了。

文件派 vs 数据库派：选型思路

我们选择了文件派（Markdown 文件 + MEMORY.md 索引），这也是 Claude Code 的方案。但这不是唯一的选择。

OpenClaw 走的是数据库派——SQLite + sqlite-vec（向量检索）+ FTS5（全文搜索），三表联合。记忆存储在数据库里，检索用 70% 向量权重 + 30% 关键词权重的混合排序，还有指数时间衰减（30 天半衰期）让旧记忆自动降权。

两种方案的选型很直接：

文件派适合：记忆条目 < 200 条、单人使用、需要 git 版本追溯、希望人类能直接打开文件阅读和编辑。

数据库派适合：记忆条目 500+、多人使用、需要语义搜索能力（"找到关于性能优化的讨论"这种模糊查询）、记忆量会持续增长。

不确定的时候从文件派开始。 文件派的迁移成本几乎为零——每个 .md 文件就是一段文本，将来要迁到数据库派，遍历文件入库就行。反过来不行——数据库里的 embedding 和索引没法直接变成人类可读的文件。

下一篇做 RAG 时，我们会用 SQLite + sqlite-vec 实现混合检索，那时候这两种方案的差异会更直观。

生产级记忆系统的进阶设计

我们实现的版本已经能跑了，但生产级的记忆系统还有几个值得了解的进阶设计。这些特性不需要现在实现，但理解它们能帮你评估自己的 Agent 什么时候需要升级。

精选注入：不是所有记忆都该塞进上下文

我们当前的做法是把 MEMORY.md 索引整个注入 system prompt。记忆少的时候这没问题。但如果积累了 200 条记忆，索引本身就可能有几千 token——其中大部分跟当前对话无关。

Claude Code 的解决方案是用一个轻量模型做异步精选。每轮对话开始前，用 Sonnet（比 Opus 便宜且更快）扫描所有记忆文件的 frontmatter，判断哪些跟当前任务相关，最多选 5 个文件，每个不超过 4KB。这个选择过程是异步的，不阻塞主流程的响应。

这就是为什么 description 字段很重要——精选模型看的就是这个字段，不是记忆的完整内容。Description 写得好，精选就准。

后台自动提取：用户无感知的记忆写入

我们当前的记忆写入是 Agent 主动调用 memory 工具——用户说"记住 XXX"，Agent 调工具存储。但很多值得记忆的信息不是用户明确要求存的。"我上周刚升了 Tech Lead"这句话出现在闲聊里，Agent 应该自己识别出来并存为 user 类型记忆。

Claude Code 的做法是每次对话结束后，fork 一个后台 Agent 来提取记忆。这个后台 Agent 有严格的限制：最多 5 轮对话预算、只能读代码和写记忆文件、跟用户手动写入互斥（防止冲突）。它分析刚才的对话内容，提取值得长期保留的信息，存到记忆目录里。

更关键的是，因为 fork 出来的 Agent 和主 Agent 共享同一段 system prompt + tools，它可以直接复用主 Agent 的 Prompt Cache，成本极低。

AutoDream：记忆的睡眠整理

Anthropic 给 Claude Code 加了一个叫 AutoDream 的功能——在用户不活跃的时候，后台 Agent 会整理记忆：合并重复信息、把相对日期转成绝对日期、删除矛盾的旧记忆、清理过时的条目。类比人类的"睡眠记忆巩固"——白天积累信息，晚上整理归档。

这些进阶特性的共同特点是：利用用户不活跃的间隙做后台工作。用户打字的间隙、对话结束后、长时间没有输入——这些都是 Agent 可以利用的"碎片时间"。

到这里，Agent 已经有了完整的跨会话记忆能力。它能从对话中提取信息、持久化存储、下次启动时自动加载、通过 Prompt Pipe 注入上下文。用 MEMORY.md 做索引，独立 .md 文件存内容，200 行上限和 4000 字符上限控制膨胀。

但目前的搜索只是关键词匹配——"性能优化"搜不到"减少延迟"，因为它不理解语义。下一篇做 RAG，给 Agent 装上向量检索和 BM25 混合搜索，让它真正"理解"记忆的含义而不是只做字符串匹配。

参考链接
How Claude remembers your project — Claude Code Docs
Mem0: State of AI Agent Memory 2026
MINJA: Memory INJection Attack on LLM Agents (NeurIPS 2025)
MemGPT/Letta — OS-inspired Memory for LLM Agents
Claude Code Memory Architecture — Three-Layer Design
Claude Code AutoDream Feature
Dive into Claude Code — Memory System Analysis
上一篇
让对话越来越便宜——Prompt Cache 与成本追踪
下一篇 · 第四章：Memory + RAG
RAG 实战——sqlite-vec + BM25 混合检索
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
import type { ModelMessage } from 'ai';
import type { ToolRegistry } from '../tools/registry.js';
import type { PromptBuilder, PromptContext } from '../context/prompt-builder.js';
import type { UsageTracker } from '../usage/tracker.js';
import type { SessionStore } from '../session/store.js';
import type { MemoryStore } from '../memory/store.js';

export interface CommandContext {
  messages: ModelMessage[];
  timestamps: Map<number, number>;
  registry: ToolRegistry;
  builder: PromptBuilder;
  tracker: UsageTracker;
  sessionStore: SessionStore;
  model: any;
  makePromptCtx: () => PromptContext;
  ask: () => void;
  memoryStore?: MemoryStore;
  [key: string]: any;
}

export type CommandHandler = (cmd: string, ctx: CommandContext) => boolean | 'async';

export function createDispatcher(handlers: CommandHandler[]): CommandHandler {
  return (cmd, ctx) => {
    for (const h of handlers) {
      const result = h(cmd, ctx);
      if (result) return result;
    }
    return false;
  };
}
```


```typescript
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
]);
```


```typescript
function makePromptCtx(): PromptContext {
  return {
    toolCount: registry.getActiveTools().length,
    deferredToolSummary: registry.getDeferredToolSummary(),
    sessionMessageCount: messages.length,
    sessionId: 'default',
  };
}
```


```typescript
import fs from 'node:fs';
import path from 'node:path';

export interface MemoryEntry {
  name: string;
  description: string;
  type: 'user' | 'feedback' | 'project' | 'reference';
  content: string;
  filePath: string;
}

const MEMORY_DIR = '.memory';
const INDEX_FILE = 'MEMORY.md';
const MAX_INDEX_LINES = 200;
const MAX_FILE_CHARS = 4000;

export class MemoryStore {
  private readonly baseDir: string;

  constructor(baseDir: string = '.') {
    this.baseDir = baseDir;
  }

  private get memoryDir(): string {
    return path.join(this.baseDir, MEMORY_DIR);
  }

  private get indexPath(): string {
    return path.join(this.memoryDir, INDEX_FILE);
  }

  init(): void {
    if (!fs.existsSync(this.memoryDir)) {
      fs.mkdirSync(this.memoryDir, { recursive: true });
    }
    if (!fs.existsSync(this.indexPath)) {
      fs.writeFileSync(this.indexPath, '# Memory Index\n', 'utf-8');
    }
  }
}
```


```markdown
---
name: 用户偏好 TypeScript
description: 用户偏好 TypeScript，不喜欢 Python
type: user
---

用户明确表示偏好 TypeScript，在需要写示例代码时优先用 TypeScript。
```


```typescript
save(entry: Omit<MemoryEntry, 'filePath'>): string {
    this.init();
    const slug = entry.name
      .toLowerCase()
      .replace(/[^a-z0-9一-鿿]+/g, '-')
      .replace(/^-|-$/g, '');
    const filename = `${entry.type}_${slug}.md`;
    const filePath = path.join(this.memoryDir, filename);

    const fileContent = [
      '---',
      `name: ${entry.name}`,
      `description: ${entry.description}`,
      `type: ${entry.type}`,
      '---',
      '',
      entry.content,
    ].join('\n');

    fs.writeFileSync(filePath, fileContent, 'utf-8');
    this.updateIndex(entry.name, filename, entry.description);
    return filename;
  }

  list(): MemoryEntry[] {
    this.init();
    const files = fs.readdirSync(this.memoryDir)
      .filter(f => f.endsWith('.md') && f !== INDEX_FILE);
    // ... parse frontmatter for each file
  }

  search(query: string): MemoryEntry[] {
    const all = this.list();
    const keywords = query.toLowerCase().split(/\s+/);
    return all.filter(entry => {
      const text = `${entry.name} ${entry.description} ${entry.content}`.toLowerCase();
      return keywords.some(kw => text.includes(kw));
    });
  }
```


```bash
pnpm start
```


```typescript
export function createMemoryTool(memoryStore: MemoryStore): ToolDefinition {
  return {
    name: 'memory',
  description: '管理跨会话记忆。action: save（保存）| list（列表）| search（搜索）| read（读取）| delete（删除）',
  parameters: {
    type: 'object',
    properties: {
      action: { type: 'string', enum: ['save', 'list', 'search', 'read', 'delete'] },
      name: { type: 'string', description: '记忆名称（save 时必填）' },
      description: { type: 'string', description: '一句话描述（save 时必填）' },
      type: { type: 'string', enum: ['user', 'feedback', 'project', 'reference'] },
      content: { type: 'string', description: '记忆内容（save 时必填）' },
      query: { type: 'string', description: '搜索关键词（search 时必填）' },
      filename: { type: 'string', description: '文件名（read/delete 时必填）' },
    },
    required: ['action'],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async (args: any) => {
    switch (args.action) {
      case 'save': {
        if (!args.name || !args.type || !args.content) {
          return '保存失败：需要 name、type、content 参数';
        }
        const filename = memoryStore.save({
          name: args.name,
          description: args.description || args.name,
          type: args.type,
          content: args.content,
        });
        return `已保存到记忆: ${filename}`;
      }
      case 'list': {
        const entries = memoryStore.list();
        if (entries.length === 0) return '当前没有存储任何记忆。';
        return `记忆列表（共 ${entries.length} 条记忆）：\n` +
          entries.map(e => `  [${e.type}] ${e.name} — ${e.description}`).join('\n');
      }
      case 'search': {
        const results = memoryStore.search(args.query || '');
        if (results.length === 0) return `没有找到与 "${args.query}" 相关的记忆。`;
        return `搜索结果（${results.length} 条匹配）：\n` +
          results.map(e => `  [${e.type}] ${e.name} — ${e.description}`).join('\n');
      }
      // ... read / delete 类似
    }
  },
  };
}
```


```bash
pnpm start
```


```
You: 记住我喜欢用 TypeScript
--- Step 1 ---
  [调用: memory({"action":"save","name":"我喜欢 TypeScript","type":"user","content":"我喜欢 TypeScript"})]
  [结果: memory] 已保存到记忆: user_我喜欢-typescript.md
--- Step 2 ---
好的，我已经把这条信息存到记忆里了。下次你重新打开对话，我还会记得这件事。
```


```
You: 我的记忆
--- Step 1 ---
  [调用: memory({"action":"list"})]
  [结果: memory] 记忆列表（共 1 条记忆）：
    [user] 我喜欢 TypeScript — 我喜欢 TypeScript
--- Step 2 ---
这是你目前的记忆：
  [user] 我喜欢 TypeScript
```


```
已加载 1 条历史记忆
```


```typescript
const builder = new PromptBuilder()
  .pipe('coreRules', coreRules())
  .pipe('toolGuide', toolGuide())
  .pipe('deferredTools', deferredTools())
  .pipe('memoryContext', () => memoryStore.buildPromptSection())   // 记忆注入
  .pipe('sessionContext', sessionContext());
```


```typescript
buildPromptSection(): string {
    this.init();
    const index = this.loadIndex();
    const entries = this.list();

    if (entries.length === 0) {
      return '[记忆系统] 当前没有存储任何记忆。你可以使用 memory 工具来保存重要信息。';
    }

    const lines = [
      `[记忆系统] 共 ${entries.length} 条记忆`,
      '',
      '记忆索引：',
      index,
      '',
      '使用 memory 工具的 read 操作来读取具体记忆内容。',
      '记忆是线索，不是事实——使用前先验证其准确性。',
    ];
    return lines.join('\n');
  }
```


```bash
pnpm start
```


```
You: /context
  ● ● ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    Mock Model
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    1.8k/1.0M tokens (0.2%)
  ...
                                       ● System prompt: 1.1k (0.1%)
                                       ● System tools:  600 (0.1%)
                                       ● Memory:        80 (0.0%)
                                       ○ Free space:   948k (94.8%)
```


```typescript
// Rebuild system prompt with latest memory
      const currentSystem = builder.build(makePromptCtx());

      await agentLoop(model, registry, messages, currentSystem, tracker);
```


```typescript
import type { CommandHandler } from './index.js';

export const memoryCommands: CommandHandler[] = [
  (cmd, ctx) => {
    if (cmd !== '/memory' && cmd !== 'memory') return false;
    const entries = ctx.memoryStore!.list();
    console.log(`\n[记忆系统] 共 ${entries.length} 条记忆`);
    for (const e of entries) console.log(`  [${e.type}] ${e.name} — ${e.description}`);
    console.log('');
    return true;
  },

  (cmd, ctx) => {
    if (!cmd.startsWith('/memory search ')) return false;
    const query = cmd.slice('/memory search '.length).trim();
    const results = ctx.memoryStore!.search(query);
    console.log(`\n[记忆搜索] "${query}" → ${results.length} 条结果`);
    for (const e of results) console.log(`  [${e.type}] ${e.name} — ${e.description}`);
    console.log('');
    return true;
  },
];
```


```bash
pnpm start
```


```
You: 记住我喜欢用 TypeScript
  [调用: memory] 已保存到记忆: user_我喜欢-typescript.md

You: 记住这个项目的数据库上周出过事故，改表结构要谨慎
  [调用: memory] 已保存到记忆: project_数据库上周出过事故.md

You: /memory
[记忆系统] 共 2 条记忆
  [user] 我喜欢 TypeScript — 用户偏好 TypeScript
  [project] 数据库事故 — 数据库上周出过事故，改表结构要谨慎

You: /memory search 数据库
[记忆搜索] "数据库" → 1 条结果
  [project] 数据库事故 — 数据库上周出过事故，改表结构要谨慎

You: status
[状态] 4 条消息 (2 条工具结果), ~85 tokens, 2 条记忆
```


```
Super Agent v0.11 — Memory System
  已加载 2 条历史记忆
```
