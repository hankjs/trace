# 工具太多模型选不准——实现 ToolSearch

课程
Super Agent 实战课
工具太多模型选不准——实现 ToolSearch
工具太多模型选不准——实现 ToolSearch

约 27 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

知识体系课分析过一组数据：工具从 10 个涨到 50 个，模型的选择准确率从 90% 掉到不到 50%。工具越多，模型越容易选错——不是因为模型变笨了，是因为选项太多干扰了判断。

更直接的问题是 token 开销。每个工具的名称、描述、参数 Schema 都要塞进 prompt，70 个工具的定义加起来可能就是 30000-50000 token。200K 上下文窗口直接少了四分之一，还没开始干活呢。而且上一篇提过，这些工具定义在 prompt 前面，一变动就让 KV Cache 失效了，成本可能翻好几倍。

这篇我们用 ToolSearch 延迟加载 来解决这个问题——把不常用的工具藏起来，模型需要时按需搜索、按需发现，prompt 里的工具数量可以从几十个压缩到个位数，同时不损失 Agent 的执行能力。

先装依赖：

bash
运行
复制
pnpm install

先把问题造出来

光说"工具太多"没感觉，先在代码里模拟一下真实场景。除了上一篇的 9 个内置工具和 3 个 GitHub MCP 工具，我们再注册一批模拟的 Notion、浏览器、Supabase 工具：

src/index.ts
应用
复制
// ... 基础代码同上一篇 ...

// 模拟额外的 MCP 工具（演示工具膨胀问题）
function registerSimulatedTools() {
  const simulatedTools: ToolDefinition[] = [
    // 这里展示一个完整的定义，其余格式一样
    {
      name: 'mcp__notion__search_pages',
      description: '[MCP:notion] 搜索 Notion 页面',
      parameters: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] },
      shouldDefer: true,
      searchHint: 'notion search pages documents',
      isConcurrencySafe: true,
      isReadOnly: true,
      execute: async ({ query }: any) => JSON.stringify([{ title: `Mock: ${query}`, id: 'page-001' }]),
    },
    // 其余 10 个模拟工具格式相同，都带 shouldDefer: true 和 searchHint
    // Notion: create_page, list_databases
    // Browser: navigate, screenshot, click, fill, get_text
    // Supabase: query, list_tables, describe_table
    // 完整代码见右侧编辑器 src/index.ts
  ];

  registry.register(...simulatedTools);
  return simulatedTools.length;
}


每个模拟工具都有三个关键字段：shouldDefer: true 标记延迟加载、searchHint 给 ToolSearch 用于匹配、execute 返回 mock 数据。

跑起来看看：

bash
运行
复制
pnpm start

已注册 3 个 Mock MCP 工具
已注册 11 个模拟 MCP 工具（Notion/Browser/Supabase）

=== 工具统计 ===
  全部工具: 24 个


24 个工具。如果全部塞进 prompt，Token 估算大约 1200-1500。这还只是模拟了 14 个 MCP 工具，真实场景下 GitHub 一个 Server 就有 26 个。

问题很明显：你在写代码的时候，不需要 Notion 的文档工具，不需要浏览器的点击工具，不需要 Supabase 的查询工具。但它们全部挤在 prompt 里，占空间、干扰模型选择。

知识体系课介绍过 OpenClaw 的 Tool Profile 方案——给工具打标签按场景裁剪。但 Profile 有一个绕不开的问题：你在 coding 场景下突然需要查一个 GitHub Issue，GitHub 工具不在 coding profile 里，这时候要么切 profile，要么手动加白名单。场景的边界在实际使用中往往没那么清晰。

我更倾向另一个思路——延迟加载。不按场景裁剪，而是把所有工具都保留，但高频的直接加载，低频的藏起来按需发现。模型需要什么工具，搜一下就能用。

哪些工具该延迟

Claude Code 的做法是把工具分成两类：

核心工具——几乎每次对话都会用到的，永远加载。Read、Edit、Write、Bash、Grep、Glob 这些，写代码离不开它们。

低频工具——偶尔用一次的，标记 shouldDefer: true。WebSearch、NotebookEdit、LSP、Cron 这些，大部分对话用不上。所有通过 MCP Server 接入的工具也默认全部延迟——MCP 工具是用户自己装的，数量不可控。

分类的依据就是使用频率，没有什么复杂的逻辑。Claude Code 还设了一个自动触发阈值：当延迟工具的 Schema 总量超过上下文窗口的 10% 时才启用延迟加载。低于这个阈值——比如你只接了一个 MCP Server、3 个工具——没必要多此一举，全量加载就行。

在我们的 Agent 里也一样：9 个内置工具（文件操作、搜索、命令执行）是核心，14 个 MCP 工具全部延迟。

在 ToolDefinition 上新增两个字段：

src/tools/registry.ts
应用
复制
export interface ToolDefinition {
  // ... 已有字段 ...
  shouldDefer?: boolean;    // 是否延迟加载
  searchHint?: string;      // 搜索提示词，帮助 ToolSearch 匹配
}


searchHint 是给 ToolSearch 用的匹配线索——一个 3-10 个词的短语，描述这个工具能做什么。比如浏览器导航工具的 hint 是 "browser navigate open url webpage"，Supabase 查询工具的 hint 是 "supabase database sql query select"。模型不会看到这些 hint，它们只在 ToolSearch 内部用于关键词匹配。

上一篇的 registerMCPServer 方法注册 MCP 工具时，自动加上 shouldDefer: true 和 searchHint：

src/tools/registry.ts
应用
复制
this.register({
  name: prefixedName,
  description: `[MCP:${serverName}] ${tool.description}`,
  parameters: tool.inputSchema,
  shouldDefer: true,
  searchHint: `${serverName} ${tool.name} ${tool.description}`,
  execute: async (input) => toolClient.callTool(originalName, input),
});

实现 ToolSearch

ToolSearch 是一个特殊的"元工具"——它不执行任何业务操作，只做一件事：根据关键词搜索已注册的工具，返回匹配工具的完整 Schema。

src/index.ts
应用
复制
const toolSearchTool: ToolDefinition = {
  name: 'tool_search',
  description: '获取延迟工具的完整定义。传入工具名（从系统提示的延迟工具列表中选取），返回该工具的完整参数 Schema',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '工具名，如 "mcp__github__list_issues"。支持逗号分隔多个工具名' },
    },
    required: ['query'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ query }: { query: string }) => {
    const results = registry.searchTools(query);
    if (results.length === 0) return `没有找到匹配 "${query}" 的工具`;
    return results.map(t => ({
      name: t.name,
      description: t.description,
      parameters: t.parameters,
    }));
  },
};

registry.register(toolSearchTool);


tool_search 本身是 isConcurrencySafe: true + isReadOnly: true——它只是在注册表里查找工具定义，不做任何写入操作，可以安全并发。

ToolRegistry 这边需要三个新能力：

搜索——searchTools 方法按精确的工具名匹配。因为 System prompt 里已经列出了所有延迟工具的名字，模型直接选名字传过来就行，不需要搞模糊匹配。支持逗号分隔一次查多个工具。匹配到的工具自动加入 discoveredTools 集合：

src/tools/registry.ts
应用
复制
searchTools(query: string): ToolDefinition[] {
  const q = query.trim();
  const results: ToolDefinition[] = [];

  const names = q.includes(',')
    ? q.split(',').map(n => n.trim()).filter(Boolean)
    : [q];

  for (const name of names) {
    const tool = this.tools.get(name);
    if (tool && tool.name !== 'tool_search') {
      results.push(tool);
      this.discoveredTools.add(tool.name);
    }
  }
  return results;
}


这里用精确匹配而不是模糊搜索，是因为工具名已经全部告诉模型了。模型看到 mcp__github__list_issues 这个名字，自然知道该传什么。精确匹配不会搜出不相关的结果，也更可靠。

过滤——getActiveTools 方法控制哪些工具进入 prompt。延迟工具默认不输出，除非已经被 tool_search 发现过：

src/tools/registry.ts
应用
复制
getActiveTools(): ToolDefinition[] {
  return this.getAll().filter(tool => {
    if (tool.shouldDefer && !this.discoveredTools.has(tool.name)) {
      return false;
    }
    return true;
  });
}


提示——getDeferredToolSummary 方法生成延迟工具的名字列表，附到 System prompt 里。模型看到这个列表就知道有哪些能力可用，需要时调 tool_search 搜索：

src/tools/registry.ts
应用
复制
getDeferredToolSummary(): string {
  const deferred = this.getAll().filter(tool => {
    return tool.shouldDefer && !this.discoveredTools.has(tool.name);
  });

  if (deferred.length === 0) return '';

  const lines = deferred.map(t => {
    const hint = t.searchHint ? ` — ${t.searchHint}` : '';
    return `  - ${t.name}${hint}`;
  });

  return `\n以下工具可用，但需要先通过 tool_search 搜索获取完整定义：\n${lines.join('\n')}`;
}


生成出来的效果类似：

以下工具可用，但需要先通过 tool_search 搜索获取完整定义：
  - mcp__github__list_issues — github list_issues 列出 GitHub 仓库的 Issues
  - mcp__notion__search_pages — notion search_pages 搜索 Notion 页面
  - mcp__browser__navigate — browser navigate 导航到 URL
  ...


toAISDKFormat() 也改成只输出 getActiveTools() 返回的工具，延迟工具的 Schema 不进 prompt。

为了直观地看到延迟加载省了多少 token，再加一个估算方法：

src/tools/registry.ts
应用
复制
countTokenEstimate(): { active: number; deferred: number; total: number } {
  let active = 0;
  let deferred = 0;

  for (const tool of this.tools.values()) {
    const schemaSize = JSON.stringify({
      name: tool.name,
      description: tool.description,
      parameters: tool.parameters,
    }).length;
    const tokens = Math.ceil(schemaSize / 4);

    if (tool.shouldDefer && !this.discoveredTools.has(tool.name)) {
      deferred += tokens;
    } else {
      active += tokens;
    }
  }

  return { active, deferred, total: active + deferred };
}


把工具定义序列化后除以 4 得到粗略的 token 数。active 是会进 prompt 的，deferred 是省下来的。

main() 函数里加上统计输出，启动时打印工具分布情况：

src/index.ts
应用
复制
const simCount = registerSimulatedTools();
console.log(`  已注册 ${simCount} 个模拟 MCP 工具（Notion/Browser/Supabase）`);

const allCount = registry.getAll().length;
const activeTools = registry.getActiveTools();
const estimate = registry.countTokenEstimate();

console.log(`\n=== 工具统计 ===`);
console.log(`  全部工具: ${allCount} 个`);
console.log(`  活跃工具: ${activeTools.length} 个`);
console.log(`  延迟工具: ${allCount - activeTools.length} 个`);
console.log(`  Token 估算: ~${estimate.active} (活跃) + ~${estimate.deferred} (延迟，不占 prompt)`);

跑起来看效果

Apply 代码后跑一下：

bash
运行
复制
pnpm start

=== 工具统计 ===
  全部工具: 26 个
  活跃工具: 12 个（非延迟）
  延迟工具: 14 个
  Token 估算: ~763 (活跃) + ~641 (延迟)


26 个工具，但 prompt 里只有 12 个。14 个 MCP 工具的 Schema 完全不占空间。

输入"查看 vercel/ai 的 issues"：

You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: tool_search({"query":"mcp__github__list_issues"})]
  [结果: tool_search] [
    { "name": "mcp__github__list_issues", "description": "..." }
  ]
  → 继续下一步...

--- Step 2 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
    { "number": 42, "title": "支持 MCP 协议接入", "state": "open" },
    ...
  ]
  → 继续下一步...

--- Step 3 ---
vercel/ai 仓库目前有以下 issues：
- #42 支持 MCP 协议接入（open）
- #41 循环检测阈值可配置化（open）
- #39 Token 预算用完后的优雅降级（closed）


这个流程有三步：

模型在 System prompt(通过 deferredSummary) 的延迟工具列表里看到了 mcp__github__list_issues，于是调 tool_search 传入这个精确的工具名
tool_search 返回了完整的 Schema 定义。同时这个工具被加入 discoveredTools 集合，下一轮请求它就出现在 tools 参数里了
模型拿到 Schema 后知道需要传 owner 和 repo，正常调用

多了一轮工具调用的开销，但省下来的是所有延迟工具的 Schema 定义不用常驻 prompt。而且一旦工具被发现过，discoveredTools 集合会记住它，后续对话里它就直接出现在 tools 参数里了，不需要再搜索。整个流程对用户完全透明——用户只说了"查看 vercel/ai 的 issues"，Agent 自己判断需要搜索、自己搜索、自己调用。

说实话这个设计的本质就是给工具集加了一层"搜索引擎"。你不需要把所有商品摆在货架上，顾客要什么搜一下就行。tool_search 本身的 Schema 只占几百 token，但它能帮你管理任意数量的延迟工具。Claude Code 大概有 20 个工具被标记为延迟加载，按每个工具 600-800 token 算，省下 12000-16000 token，加上 MCP 工具就更可观了。

对 Prompt Cache 的影响

tool_search 发现工具后，下一轮请求会把这个工具加进 tools 参数里——工具列表变了，从工具定义的位置开始 KV Cache 失效。

知识体系课讲过，Claude Code 用 Anthropic API 的 defer_loading beta 特性解决了这个问题——延迟工具的 Schema 出现在对话历史里（tool_result 消息中），不在工具定义区域，所以 cache 前缀完全不受影响。

我们用的是另一种方式：直接动态修改 tools 列表。这意味着发现新工具的那一轮会丢失 cache。但实际上工具发现主要集中在对话前几轮——用户一上来就会说"帮我查个 Issue"、"帮我看看数据库"，前两三轮把需要的工具都搜出来之后，工具列表就稳定了，后面的 cache 不受影响。

知识体系课还介绍了一种更激进的方案：ToolSearch + CallTool 双工具代理模式。tools 列表里永远只有 tool_search 和 call_tool 两个元工具，模型先搜索获取 Schema，再通过 call_tool 转发执行，应用层根据 tool_name 路由到真正的工具实现。这样工具列表从头到尾不变，cache 完全稳定。代价是模型不是通过 tools 参数里的结构化 Schema 来"认识"工具的，而是通过对话历史里的文本描述来理解参数格式，参数复杂的工具准确率会略低一些。

两种做法各有取舍，但我推荐在生产环境还是使用本文实战的方法比较好，因为用的是原生 tools 列表做工具加载，稳定性更有保障。

到这里，Tool System 这一章就结束了。回顾一下这几篇我们做了什么：从最初的两个玩具工具出发，搭了 ToolRegistry 统一注册和管理，加了结果截断和读写锁并发控制，通过 MCP 接入了外部 GitHub 能力，最后用 ToolSearch 延迟加载解决了工具数量膨胀的问题。整个工具系统的骨架搭完了。

下一章我们进入 Context Engineering——Session 持久化、Prompt 组装、上下文压缩、成本控制，每一篇都是实打实的硬核实战。

参考链接
Anthropic - Tool Use Overview
Anthropic - Prompt Caching
Anthropic - Advanced Tool Use
上一篇
MCP 接入实战——给 Agent 接上 GitHub
下一篇 · 第三章：Context Engineering
Session 持久化 + Prompt Pipe——对话存档与模块化 Prompt 组装
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
// ... 基础代码同上一篇 ...

// 模拟额外的 MCP 工具（演示工具膨胀问题）
function registerSimulatedTools() {
  const simulatedTools: ToolDefinition[] = [
    // 这里展示一个完整的定义，其余格式一样
    {
      name: 'mcp__notion__search_pages',
      description: '[MCP:notion] 搜索 Notion 页面',
      parameters: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] },
      shouldDefer: true,
      searchHint: 'notion search pages documents',
      isConcurrencySafe: true,
      isReadOnly: true,
      execute: async ({ query }: any) => JSON.stringify([{ title: `Mock: ${query}`, id: 'page-001' }]),
    },
    // 其余 10 个模拟工具格式相同，都带 shouldDefer: true 和 searchHint
    // Notion: create_page, list_databases
    // Browser: navigate, screenshot, click, fill, get_text
    // Supabase: query, list_tables, describe_table
    // 完整代码见右侧编辑器 src/index.ts
  ];

  registry.register(...simulatedTools);
  return simulatedTools.length;
}
```


```bash
pnpm start
```


```
已注册 3 个 Mock MCP 工具
已注册 11 个模拟 MCP 工具（Notion/Browser/Supabase）

=== 工具统计 ===
  全部工具: 24 个
```


```typescript
export interface ToolDefinition {
  // ... 已有字段 ...
  shouldDefer?: boolean;    // 是否延迟加载
  searchHint?: string;      // 搜索提示词，帮助 ToolSearch 匹配
}
```


```typescript
this.register({
  name: prefixedName,
  description: `[MCP:${serverName}] ${tool.description}`,
  parameters: tool.inputSchema,
  shouldDefer: true,
  searchHint: `${serverName} ${tool.name} ${tool.description}`,
  execute: async (input) => toolClient.callTool(originalName, input),
});
```


```typescript
const toolSearchTool: ToolDefinition = {
  name: 'tool_search',
  description: '获取延迟工具的完整定义。传入工具名（从系统提示的延迟工具列表中选取），返回该工具的完整参数 Schema',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '工具名，如 "mcp__github__list_issues"。支持逗号分隔多个工具名' },
    },
    required: ['query'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  execute: async ({ query }: { query: string }) => {
    const results = registry.searchTools(query);
    if (results.length === 0) return `没有找到匹配 "${query}" 的工具`;
    return results.map(t => ({
      name: t.name,
      description: t.description,
      parameters: t.parameters,
    }));
  },
};

registry.register(toolSearchTool);
```


```typescript
searchTools(query: string): ToolDefinition[] {
  const q = query.trim();
  const results: ToolDefinition[] = [];

  const names = q.includes(',')
    ? q.split(',').map(n => n.trim()).filter(Boolean)
    : [q];

  for (const name of names) {
    const tool = this.tools.get(name);
    if (tool && tool.name !== 'tool_search') {
      results.push(tool);
      this.discoveredTools.add(tool.name);
    }
  }
  return results;
}
```


```typescript
getActiveTools(): ToolDefinition[] {
  return this.getAll().filter(tool => {
    if (tool.shouldDefer && !this.discoveredTools.has(tool.name)) {
      return false;
    }
    return true;
  });
}
```


```typescript
getDeferredToolSummary(): string {
  const deferred = this.getAll().filter(tool => {
    return tool.shouldDefer && !this.discoveredTools.has(tool.name);
  });

  if (deferred.length === 0) return '';

  const lines = deferred.map(t => {
    const hint = t.searchHint ? ` — ${t.searchHint}` : '';
    return `  - ${t.name}${hint}`;
  });

  return `\n以下工具可用，但需要先通过 tool_search 搜索获取完整定义：\n${lines.join('\n')}`;
}
```


```
以下工具可用，但需要先通过 tool_search 搜索获取完整定义：
  - mcp__github__list_issues — github list_issues 列出 GitHub 仓库的 Issues
  - mcp__notion__search_pages — notion search_pages 搜索 Notion 页面
  - mcp__browser__navigate — browser navigate 导航到 URL
  ...
```


```typescript
countTokenEstimate(): { active: number; deferred: number; total: number } {
  let active = 0;
  let deferred = 0;

  for (const tool of this.tools.values()) {
    const schemaSize = JSON.stringify({
      name: tool.name,
      description: tool.description,
      parameters: tool.parameters,
    }).length;
    const tokens = Math.ceil(schemaSize / 4);

    if (tool.shouldDefer && !this.discoveredTools.has(tool.name)) {
      deferred += tokens;
    } else {
      active += tokens;
    }
  }

  return { active, deferred, total: active + deferred };
}
```


```typescript
const simCount = registerSimulatedTools();
console.log(`  已注册 ${simCount} 个模拟 MCP 工具（Notion/Browser/Supabase）`);

const allCount = registry.getAll().length;
const activeTools = registry.getActiveTools();
const estimate = registry.countTokenEstimate();

console.log(`\n=== 工具统计 ===`);
console.log(`  全部工具: ${allCount} 个`);
console.log(`  活跃工具: ${activeTools.length} 个`);
console.log(`  延迟工具: ${allCount - activeTools.length} 个`);
console.log(`  Token 估算: ~${estimate.active} (活跃) + ~${estimate.deferred} (延迟，不占 prompt)`);
```


```bash
pnpm start
```


```
=== 工具统计 ===
  全部工具: 26 个
  活跃工具: 12 个（非延迟）
  延迟工具: 14 个
  Token 估算: ~763 (活跃) + ~641 (延迟)
```


```
You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: tool_search({"query":"mcp__github__list_issues"})]
  [结果: tool_search] [
    { "name": "mcp__github__list_issues", "description": "..." }
  ]
  → 继续下一步...

--- Step 2 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
    { "number": 42, "title": "支持 MCP 协议接入", "state": "open" },
    ...
  ]
  → 继续下一步...

--- Step 3 ---
vercel/ai 仓库目前有以下 issues：
- #42 支持 MCP 协议接入（open）
- #41 循环检测阈值可配置化（open）
- #39 Token 预算用完后的优雅降级（closed）
```
