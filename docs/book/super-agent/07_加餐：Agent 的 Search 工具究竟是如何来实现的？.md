# 加餐：Agent 的 Search 工具究竟是如何来实现的？

课程
Super Agent 实战课
加餐：Agent 的 Search 工具究竟是如何来实现的？
加餐：Agent 的 Search 工具究竟是如何来实现的？

约 19 分钟

本节示例需要搜索 API Key。推荐 Tavily（免费 1000 次/月）或 Serper（免费 2500 次/月），任选一个注册后把 Key 填到 .env 文件里。

你用过 Claude Code 的 WebSearch、用过 Cursor 的联网搜索，但你有没有想过这些"搜索能力"到底是怎么实现的？说白了就是一个 Tool——调一个搜索 API 拿结果，跟我们之前写的 get_weather 没有本质区别。

但搜索这个场景有意思的地方在于：市面上的搜索 API 五花八门，选哪个、怎么选，直接影响 Agent 的信息质量和使用成本。这一节我们先做个选型对比，然后实现两套方案——Tavily（自动挡）和 Serper（手动挡），通过环境变量一键切换。

先装依赖：

bash
运行
复制
pnpm install

搜索 API 选型

Agent 能用的搜索 API 大致分三类，区别在于搜索方式和结果处理两个维度上各自做了不同的选择：

Google 代理型（Serper、SerpAPI）——本质是帮你调 Google，返回 Google 的搜索结果。搜索用的是 Google 的关键词匹配引擎，结果也是原样返回 Google snippet——每条一两句话的摘要。速度最快（200-500ms），价格最低（Serper $0.30-1/1K 次），但 Agent 想看详情得自己再去抓网页全文。

AI 原生型（Tavily）——搜索本身还是传统的关键词 + 排序，但结果处理做了 AI 增强：自动从网页里提取完整段落文本，还能返回一个 AI 生成的摘要回答。相当于帮你找了资料还做了笔记，LLM 可以直接用。代价是更贵（$5-8/1K 次）更慢（1-2s）。

维度	Serper	Tavily
免费额度	2,500 次/月	1,000 次/月
价格	$0.30-1/1K	$5-8/1K
延迟	200-500ms	1-2s
返回内容	snippet（一两句话）	提取文本（完整段落）
需要 web_fetch	是	否

小结一下：Tavily 是自动挡——搜索 + 内容提取，有点小贵，但比较省心，Serper 是手动挡——搜索拿到链接后自己抓全文，便宜但需要多一个 web_fetch 工具。

实现 web_search（双引擎）

我们把两个引擎都实现，通过环境变量自动选择：配了 TAVILY_API_KEY 就用 Tavily，配了 SERPER_API_KEY 就用 Serper。

相关平台链接：Tavily、Serper。

新建 src/search-tools.ts：

src/search-tools.ts
应用
复制
import type { ToolDefinition } from './tool-registry.js';

// ── Tavily（自动挡）──────────────────────────────

export const tavilySearchTool: ToolDefinition = {
  name: 'web_search',
  description: '搜索互联网获取最新信息。返回相关网页的标题、链接和内容摘要',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索关键词' },
      max_results: { type: 'number', description: '返回结果数量，默认 5' },
    },
    required: ['query'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ query, max_results = 5 }: { query: string; max_results?: number }) => {
    const apiKey = process.env.TAVILY_API_KEY;
    if (!apiKey) return '[web_search] 未配置 TAVILY_API_KEY，请在 .env 中设置';

    const res = await fetch('https://api.tavily.com/search', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        api_key: apiKey,
        query,
        max_results,
        include_answer: true,
      }),
    });

    if (!res.ok) return `[web_search] 请求失败: HTTP ${res.status}`;

    const data = await res.json() as any;
    const lines: string[] = [];

    if (data.answer) {
      lines.push(`## AI 摘要\n${data.answer}\n`);
    }

    for (const r of data.results || []) {
      lines.push(`### ${r.title}`);
      lines.push(r.url);
      lines.push(r.content || r.snippet || '');
      lines.push('');
    }

    return lines.join('\n') || '没有找到相关结果';
  },
};

// ── Serper（手动挡）──────────────────────────────

export const serperSearchTool: ToolDefinition = {
  name: 'web_search',
  description: '搜索互联网获取最新信息。返回 Google 搜索结果的标题、链接和摘要',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索关键词' },
      max_results: { type: 'number', description: '返回结果数量，默认 5' },
    },
    required: ['query'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ query, max_results = 5 }: { query: string; max_results?: number }) => {
    const apiKey = process.env.SERPER_API_KEY;
    if (!apiKey) return '[web_search] 未配置 SERPER_API_KEY，请在 .env 中设置';

    const res = await fetch('https://google.serper.dev/search', {
      method: 'POST',
      headers: {
        'X-API-KEY': apiKey,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ q: query, num: max_results }),
    });

    if (!res.ok) return `[web_search] 请求失败: HTTP ${res.status}`;

    const data = await res.json() as any;
    const lines: string[] = [];

    if (data.knowledgeGraph) {
      const kg = data.knowledgeGraph;
      lines.push(`## ${kg.title}`);
      if (kg.description) lines.push(kg.description);
      lines.push('');
    }

    for (const r of (data.organic || []).slice(0, max_results)) {
      lines.push(`### ${r.title}`);
      lines.push(r.link);
      lines.push(r.snippet || '');
      lines.push('');
    }

    return lines.join('\n') || '没有找到相关结果';
  },
};


两个引擎注册的工具名都叫 web_search——对模型来说没有区别，它只管调 web_search，不需要知道后端是 Tavily 还是 Serper。切换后端只需要改环境变量：

bash
复制
# .env 里二选一
TAVILY_API_KEY=tvly-xxxxx       # 自动挡
# 或
SERPER_API_KEY=xxxxx            # 手动挡


启动时自动检测：

src/search-tools.ts
应用
复制
export function pickSearchTool(): ToolDefinition {
  if (process.env.TAVILY_API_KEY) return tavilySearchTool;
  if (process.env.SERPER_API_KEY) return serperSearchTool;
  return tavilySearchTool;  // 默认（会提示配 Key）
}

bash
运行
复制
pnpm start

实现 web_fetch（手动挡配套）

用 Serper 的时候，搜索结果只有 snippet。Agent 想看详情就需要一个 web_fetch 工具——抓取 URL 全文，把 HTML 转成 Markdown：

src/search-tools.ts
应用
复制
export const webFetchTool: ToolDefinition = {
  name: 'web_fetch',
  description: '抓取指定 URL 的网页内容，转换为 Markdown 格式',
  parameters: {
    type: 'object',
    properties: {
      url: { type: 'string', description: '完整 URL' },
    },
    required: ['url'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ url }: { url: string }) => {
    try {
      const res = await fetch(url, {
        headers: { 'User-Agent': 'Mozilla/5.0 (compatible; SuperAgent/1.0)' },
        signal: AbortSignal.timeout(15000),
      });
      if (!res.ok) return `抓取失败: HTTP ${res.status}`;
      const html = await res.text();
      return htmlToMarkdown(html);
    } catch (err: any) {
      return `抓取失败: ${err.message}`;
    }
  },
};


HTML 转 Markdown 用的是 Turndown——社区标准的转换库，标题、链接、代码块、表格、嵌套列表全都能正确处理。我们配了 remove 过滤掉 <script>、<style>、<nav>、<footer> 这些噪音标签，只保留正文内容。

手写正则做 HTML 转 Markdown 看着简单，但边界情况太多——嵌套标签、自闭合标签、HTML 实体编码、表格——每个都是坑。Turndown 压缩后不到 30KB，没有理由自己造轮子。

web_fetch 替代了上一篇的 fetch_url。fetch_url 粗暴地把 HTML 标签全删了返回纯文本，web_fetch 通过 Turndown 保留了 Markdown 结构——标题层级、链接、代码块、列表都在，LLM 读起来信息密度更高。

bash
运行
复制
pnpm start

自动挡 vs 手动挡的实际差异

用 Tavily 的时候，Agent 搜索一次就够了——结果自带完整内容：

text
复制
You: 搜索一下 Vercel AI SDK 最新版本

--- Step 1 ---
  [调用: web_search({"query":"Vercel AI SDK latest version 2026"})]
  [结果: web_search]
  ## AI 摘要
  Vercel AI SDK 最新版本是 5.0...

  ### Vercel AI SDK Documentation
  https://ai-sdk.dev/docs
  AI SDK 5.0 引入了全新的 LanguageModelV2 接口...


用 Serper 的时候，Agent 通常需要两步——先搜索拿链接，再 fetch 详情：

text
复制
Step 1: web_search → 拿到 5 条 Google 结果（每条只有一两句 snippet）
Step 2: web_fetch → Agent 自己判断哪条最相关，抓取全文
Step 3: 综合全文内容给出回答


多了一步，但每次搜索便宜 10-25 倍。日均 1000 次搜索的话，Tavily 月账单 
150
−
240
，
𝑆
𝑒
𝑟
𝑝
𝑒
𝑟
只要
150−240，Serper只要9-30。这个差距在生产环境跑量大的时候很明显。

写在最后

搜索工具的实现本身不复杂——核心就是调 API、格式化结果。真正值得关注的是选型决策：你的 Agent 是偶尔搜一搜（Tavily 省事），还是高频搜索（Serper 省钱）？ 搜索结果的摘要够不够用，还是需要经常抓全文？这些问题没有标准答案，取决于你的业务场景。

我们这一节实现的双引擎架构给了你灵活性——环境变量一切换就能换后端，代码零改动。以后你要加 Brave Search，只需要再写一个 braveSearchTool，加到 pickSearchTool 的判断里就行。

参考资料
Tavily API 文档 — AI 原生搜索 API
Serper API 文档 — Google Search API 代理
Turndown — 生产级 HTML 转 Markdown 库
mozilla/readability — Firefox 阅读模式提取库
上一篇
小试牛刀——把工具组装成应用：代码分析、Research Agent、Vibe Coding
下一篇 · 第二章：Tool System
MCP 接入实战——给 Agent 接上 GitHub
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
import type { ToolDefinition } from './tool-registry.js';

// ── Tavily（自动挡）──────────────────────────────

export const tavilySearchTool: ToolDefinition = {
  name: 'web_search',
  description: '搜索互联网获取最新信息。返回相关网页的标题、链接和内容摘要',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索关键词' },
      max_results: { type: 'number', description: '返回结果数量，默认 5' },
    },
    required: ['query'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ query, max_results = 5 }: { query: string; max_results?: number }) => {
    const apiKey = process.env.TAVILY_API_KEY;
    if (!apiKey) return '[web_search] 未配置 TAVILY_API_KEY，请在 .env 中设置';

    const res = await fetch('https://api.tavily.com/search', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        api_key: apiKey,
        query,
        max_results,
        include_answer: true,
      }),
    });

    if (!res.ok) return `[web_search] 请求失败: HTTP ${res.status}`;

    const data = await res.json() as any;
    const lines: string[] = [];

    if (data.answer) {
      lines.push(`## AI 摘要\n${data.answer}\n`);
    }

    for (const r of data.results || []) {
      lines.push(`### ${r.title}`);
      lines.push(r.url);
      lines.push(r.content || r.snippet || '');
      lines.push('');
    }

    return lines.join('\n') || '没有找到相关结果';
  },
};

// ── Serper（手动挡）──────────────────────────────

export const serperSearchTool: ToolDefinition = {
  name: 'web_search',
  description: '搜索互联网获取最新信息。返回 Google 搜索结果的标题、链接和摘要',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: '搜索关键词' },
      max_results: { type: 'number', description: '返回结果数量，默认 5' },
    },
    required: ['query'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ query, max_results = 5 }: { query: string; max_results?: number }) => {
    const apiKey = process.env.SERPER_API_KEY;
    if (!apiKey) return '[web_search] 未配置 SERPER_API_KEY，请在 .env 中设置';

    const res = await fetch('https://google.serper.dev/search', {
      method: 'POST',
      headers: {
        'X-API-KEY': apiKey,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ q: query, num: max_results }),
    });

    if (!res.ok) return `[web_search] 请求失败: HTTP ${res.status}`;

    const data = await res.json() as any;
    const lines: string[] = [];

    if (data.knowledgeGraph) {
      const kg = data.knowledgeGraph;
      lines.push(`## ${kg.title}`);
      if (kg.description) lines.push(kg.description);
      lines.push('');
    }

    for (const r of (data.organic || []).slice(0, max_results)) {
      lines.push(`### ${r.title}`);
      lines.push(r.link);
      lines.push(r.snippet || '');
      lines.push('');
    }

    return lines.join('\n') || '没有找到相关结果';
  },
};
```


```bash
# .env 里二选一
TAVILY_API_KEY=tvly-xxxxx       # 自动挡
# 或
SERPER_API_KEY=xxxxx            # 手动挡
```


```ts
export function pickSearchTool(): ToolDefinition {
  if (process.env.TAVILY_API_KEY) return tavilySearchTool;
  if (process.env.SERPER_API_KEY) return serperSearchTool;
  return tavilySearchTool;  // 默认（会提示配 Key）
}
```


```bash
pnpm start
```


```ts
export const webFetchTool: ToolDefinition = {
  name: 'web_fetch',
  description: '抓取指定 URL 的网页内容，转换为 Markdown 格式',
  parameters: {
    type: 'object',
    properties: {
      url: { type: 'string', description: '完整 URL' },
    },
    required: ['url'],
  },
  isConcurrencySafe: true,
  isReadOnly: true,
  maxResultChars: 3000,
  execute: async ({ url }: { url: string }) => {
    try {
      const res = await fetch(url, {
        headers: { 'User-Agent': 'Mozilla/5.0 (compatible; SuperAgent/1.0)' },
        signal: AbortSignal.timeout(15000),
      });
      if (!res.ok) return `抓取失败: HTTP ${res.status}`;
      const html = await res.text();
      return htmlToMarkdown(html);
    } catch (err: any) {
      return `抓取失败: ${err.message}`;
    }
  },
};
```


```bash
pnpm start
```


```text
You: 搜索一下 Vercel AI SDK 最新版本

--- Step 1 ---
  [调用: web_search({"query":"Vercel AI SDK latest version 2026"})]
  [结果: web_search]
  ## AI 摘要
  Vercel AI SDK 最新版本是 5.0...

  ### Vercel AI SDK Documentation
  https://ai-sdk.dev/docs
  AI SDK 5.0 引入了全新的 LanguageModelV2 接口...
```


```text
Step 1: web_search → 拿到 5 条 Google 结果（每条只有一两句 snippet）
Step 2: web_fetch → Agent 自己判断哪条最相关，抓取全文
Step 3: 综合全文内容给出回答
```
