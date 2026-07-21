# 小试牛刀——把工具组装成应用：代码分析、Research Agent、Vibe Coding

课程
Super Agent 实战课
小试牛刀——把工具组装成应用：代码分析、Research Agent、Vibe Coding
小试牛刀——把工具组装成应用：代码分析、Research Agent、Vibe Coding

约 33 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

到这里，你的 Agent 已经有了一套完整的工具系统：read_file、write_file、list_directory、edit_file、glob、grep、bash，加上 ToolRegistry、结果截断、读写锁，零件都齐了。

但你可能心里有点没底——这些零件拼在一起，到底能干什么真东西？

这一篇我们先放一放架构和原理，拿已有的工具组装三个能直接跑的应用：

代码分析 Agent——丢一个项目目录给它，自己探索、找出所有 TODO/FIXME 并归类
Research Agent——给一个 URL，自己抓取、剥 HTML、综合摘要
Vibe Coding 网页生成——一句话让它生成一个能在浏览器跑起来的多文件 React 应用

每个 demo 都不引入新概念，全靠现有的工具系统支撑。唯一的区别是再加两个工具：fetch_url(读网页) 和 start_preview(启动网页预览)。

先装依赖：

bash
运行
复制
pnpm install

加一个新工具：fetch_url

在 src/tools.ts 里加一个能抓网页的工具。逻辑很简单：调 Node 内置的 fetch、剥掉 <script>/<style>/HTML 标签，返回纯文本。教学场景里再加一层 MOCK_PAGES 兜底，保证没联网也能跑：

src/tools.ts
应用
复制
const MOCK_PAGES: Record<string, string> = {
  'https://esm.sh': `esm.sh - 一个免费的 ES module CDN...`,
  'https://ai-sdk.dev/docs/ai-sdk-core/tools-and-tool-calling': `AI SDK Core - Tools and Tool Calling
工具是模型可以决定调用的函数。一个工具由三部分组成：
- description：告诉模型何时使用这个工具
- inputSchema：通过 Zod 或 JSON Schema 定义参数
- execute：实际在服务端运行的函数...`,
  // ... 更多预定义页面
};

export const fetchUrlTool: ToolDefinition = {
  name: 'fetch_url',
  description: '抓取指定 URL 的网页内容并转换为纯文本（自动剥离 HTML 标签）',
  parameters: {
    type: 'object',
    properties: {
      url: { type: 'string', description: '完整 URL，必须以 http:// 或 https:// 开头' },
    },
    required: ['url'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,    // 只读、可并发——抓多个 URL 时直接并行
  isReadOnly: true,
  maxResultChars: 1500,        // 网页通常很长，截断兜底
  execute: async ({ url }: { url: string }) => {
    for (const key of Object.keys(MOCK_PAGES)) {
      if (url.startsWith(key)) return MOCK_PAGES[key];
    }
    try {
      const res = await fetch(url, {
        headers: { 'User-Agent': 'Mozilla/5.0 SuperAgent' },
        signal: AbortSignal.timeout(10000),
      });
      if (!res.ok) return `请求失败：HTTP ${res.status}`;
      const html = await res.text();
      return html
        .replace(/<script[\s\S]*?<\/script>/gi, '')
        .replace(/<style[\s\S]*?<\/style>/gi, '')
        .replace(/<[^>]+>/g, ' ')
        .replace(/\s+/g, ' ')
        .trim() || '页面无文本内容';
    } catch (err: any) {
      return `抓取失败：${err.message}`;
    }
  },
};


三个细节跟大家说明一下。

isConcurrencySafe: true——网页抓取是只读操作，多个 URL 可以并行抓。前一篇做的读写锁会自动让它们同时跑。

maxResultChars: 1500——一个稍微复杂点的网页 strip 完 HTML 标签也有几万字符。不截断的话，一次请求就能把上下文吃掉一大半。具体数字可以根据场景调，但任何会返回大文本的工具，都得设这个值。

MOCK_PAGES 兜底——如果由于网络原因拿不到网页内容，会自动 fallback 到 MOCK_PAGES 内容，避免功能不可用。

再加一个：start_preview

为了让 Vibe Coding 的 demo 真正"开箱即用"——Agent 写完文件后你能立刻看到效果——再加一个 start_preview 工具。它做的事很简单：启动一个 Node 内置 http server，把 app/ 目录暴露到 8080 端口。

src/tools.ts
应用
复制
import { createServer, type Server } from 'node:http';

let previewServer: Server | null = null;

const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.tsx': 'application/javascript; charset=utf-8',  // 让浏览器把 .tsx 当 JS 加载
  '.ts': 'application/javascript; charset=utf-8',
  // ...
};

export const startPreviewTool: ToolDefinition = {
  name: 'start_preview',
  description: '启动 app/ 目录的预览服务器。生成应用文件后必须立即调用此工具',
  parameters: {
    type: 'object',
    properties: { port: { type: 'number' } },
    required: [],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async ({ port = 8080 }: { port?: number } = {}) => {
    if (previewServer) return `预览服务器已在运行 → http://localhost:${port}`;
    const root = resolve('app');
    if (!existsSync(root)) return '错误：app/ 目录不存在';

    previewServer = createServer((req, res) => {
      const urlPath = (req.url?.split('?')[0] || '/').replace(/\/$/, '/index.html');
      const filePath = join(root, urlPath === '/' ? '/index.html' : urlPath);
      try {
        if (!filePath.startsWith(root)) { res.writeHead(403); res.end(); return; }
        res.writeHead(200, {
          'Content-Type': MIME[extname(filePath).toLowerCase()] || 'application/octet-stream',
          'Cache-Control': 'no-cache',
        });
        res.end(readFileSync(filePath));
      } catch { res.writeHead(404); res.end('Not Found'); }
    });

    return new Promise<string>((resolve) => {
      previewServer!.listen(port, () => {
        resolve(`✓ 预览服务器已启动 → http://localhost:${port}`);
      });
    });
  },
};


最后把两个新工具注册到 allTools：

src/tools.ts
应用
复制
export const allTools: ToolDefinition[] = [
  weatherTool, calculatorTool,
  readFileTool, writeFileTool, listDirectoryTool,
  editFileTool, globTool, grepTool, bashTool,
  fetchUrlTool,        // ← 新增 1
  startPreviewTool,    // ← 新增 2
];


跑起来看看：

bash
运行
复制
pnpm start


启动后你会看到注册了 11 个工具，多出来的两个是 fetch_url 和 start_preview。

接下来就是收获成果的时候了。

Demo 1：代码分析 Agent

第一个 demo——丢给 Agent 一个项目目录，让它自己探索代码、找出所有未完成的待办标记。

模板里已经放了一个 sample-project/ 目录，里面是几个故意留了 TODO/FIXME 注释的 TypeScript 文件，模拟一个有"技术债"的真实项目。

输入：

You: 找出项目里所有 TODO


mock 模式下你会看到这样的输出：

--- Step 1 ---
  [并发] list_directory 获取共享锁
  [调用: list_directory({"path":"sample-project"})]
  [结果: list_directory] [FILE] api.ts
  [FILE] auth.ts
  [FILE] utils.ts
  [并发] grep 获取共享锁
  [调用: grep({"pattern":"TODO|FIXME","path":"sample-project"})]
  [结果: grep] api.ts:13:   // TODO: 加上数据库查询，目前只用了内存 Map
  api.ts:18:   // FIXME: id 生成方式应该换成 nanoid，时间戳容易冲突
  api.ts:25:   // TODO: 软删除而不是物理删除
  ... [省略] ...

--- Step 2 ---
项目里一共找到 8 处 TODO/FIXME，分布在 3 个文件：

- api.ts（4 处）
   * TODO: 加上数据库查询，目前只用了内存 Map
   * FIXME: id 生成方式应该换成 nanoid，时间戳容易冲突
   * TODO: 软删除而不是物理删除
- auth.ts（3 处）
   * TODO: 接入真正的 password hash 校验（bcrypt 或 argon2）
   * FIXME: 这里硬编码了 admin 用户用于联调，上线前必须删除
   * TODO: 改成 JWT 校验，目前只是字符串前缀判断
- utils.ts（1 处）
   * TODO: 支持多时区，目前只输出本地时间

建议优先处理 FIXME 标记的几条，那些是上线前必须修的硬伤。


看起来很自然，但里面有几个值得品的地方。

Agent 自己决定了执行计划。 用户只说了"找出项目里所有 TODO"，没告诉它"先列目录、再 grep"。Agent 自己规划成两步：先用 list_directory 看看目录里有啥（建立认知），再用 grep 精准定位 TODO/FIXME（执行任务）。

两个工具是并行执行的。 看到 Step 1 里两个 [并发] 标记了吗？list_directory 和 grep 都标了 isConcurrencySafe: true，前一篇做的读写锁立刻发挥作用——同时拿到共享锁、并行执行，没有任何串行等待。真实项目里，这种并发执行能直接砍掉一半响应时间。

结果被截断了。 grep 结果里能看到"... [省略]..."的截断标记。如果不截断，grep 一次能返回几千行内容，全塞进上下文非常浪费 token。maxResultChars: 3000 让它只返回核心匹配，给模型留出"消化和归纳"的空间。

Step 2 才是真正的 Agent 价值。 模型拿到 grep 的原始输出后，没有直接复述，而是按文件归类、统计数量、还给出了优先级建议——这就是前面讲 Agent Loop 那篇提过的 ReAct 模式里 reasoning 的部分：工具拿数据，模型做判断。

这个模式可以泛化——list_directory + grep/glob + 模型归纳，是所有"代码探索类"任务的通用骨架。

换个需求描述就能让 Agent 找出"所有未捕获的异常"、"所有用了过期 API 的地方"、"所有跨模块的循环依赖"。Agent 不需要为每个任务专门写一个工具，通用工具的不同组合就能覆盖大量场景。这也是为什么 Claude Code 这类产品只内置十来个工具，却能处理几乎所有代码相关的任务。

Demo 2：Research Agent

第二个 demo 更接近"AI 替我读东西"的场景。给 Agent 一个 URL，让它自己抓内容、做摘要。

输入：

You: 去 https://ai-sdk.dev/docs/ai-sdk-core/tools-and-tool-calling 看下文档总结


它会输出两个步骤：第一步查看网页内容，第二步给出总结内容。

表面看简单——抓一个网页、做一个总结。但在生产里，这个流程的的价值不容小觑：

截断在这里帮了大忙。 假设你给的不是一个简单的 API 文档，而是一个完整的 React 中文文档主页——剥掉 HTML 标签后还是几十 K 字符。不截断的话，单次工具调用就能用掉 prompt 80% 的预算，模型剩下没几个 token 来推理了。fetchUrlTool 的 maxResultChars: 1500 不是约束，而是上下文工程的第一道防线——前面 Tool System 那章讨论过这个原理。

多 URL 并发抓取是天然能力。 换成这个输入就能看出来：

You: 同时去 https://esm.sh 和 https://ai-sdk.dev/docs/ai-sdk-core/generating-text 看看，对比一下


Agent 会一次发出两个 fetch_url 调用，并行抓取——因为 fetch_url 标了 isConcurrencySafe: true。两个 URL 几乎同时返回，再综合摘要。如果你的 Agent 要看 5 篇博客做对比研究，这个并发能力有和没有就是完全不一样的体验。

推荐这里用真实模型。只要你在 .env 里配上 DASHSCOPE_API_KEY，切换到真实的 Qwen 模型，立刻就能看到真正的语义级摘要——模型会从全文中提炼核心论点、补充背景、给出比较。这些是 mock 模型做不到的。

再往前走一步，这个 demo 就是最近被广泛讨论的 Deep Research 类产品的雏形（OpenAI 的 deep research、Perplexity 的 Pro Search 等）。它们的核心不是抓取能力本身，而是多轮迭代的研究链路：抓取然后摘要，摘要后提出新问题，再抓取相关资料，最后综合成报告。本质上就是把一次 fetch + summarize 放进 Agent Loop 多跑几轮，每一轮的产出都喂回给模型作为下一轮的依据。

我们现在的架构其实已经具备这个能力——fetch_url 加上 Agent Loop 的多步推理，它就会自动迭代。

如果你感兴趣，可以继续沿着这个方向去完善，最终完成一个 DeepResearch Agent。

Demo 3：Vibe Coding 多文件应用生成

第三个 demo 是我觉得最有意思的——让 Agent 生成一个完整的、能在浏览器里跑起来的多文件 React 应用，类似现在的 v0、Bolt.new、Lovable 的 Agent。我们可以来基于现在的 Agent 系统来实现一个初步的版本。

输入可以很简单：

You: 做一个待办清单的网页应用


动手之前先说一下项目结构。app/index.html 是预置的脚手架，模板自带，Agent 不需要也不应该重新生成它。这个 HTML 里固定了 importmap + Babel Standalone 实时编译 + 一个把相对 .tsx import 转成 Blob URL 的加载器。这套底子是基础设施——每个 vibe coding 项目都一模一样，没必要让模型每次都重新写几十行容易出错的 loader 代码。

v0、Bolt 这些产品也是同样的思路：固定脚手架，模型只生成应用代码。

所以 Agent 实际只需要写 3 个文件：

app/
├── index.html     ← 预置脚手架，固定加载 ./App.tsx 作为入口
├── App.tsx        ← Agent 写：主入口，必须有 createRoot
├── Button.tsx     ← Agent 写：可被 App.tsx import 的组件
└── styles.css     ← Agent 写：样式


输出：

--- Step 1 ---
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/styles.css", ...})]
  [结果: write_file] 已写入 1386 字符到 app/styles.css
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/Button.tsx", ...})]
  [结果: write_file] 已写入 336 字符到 app/Button.tsx
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/App.tsx", ...})]
  [结果: write_file] 已写入 1627 字符到 app/App.tsx
  [串行] start_preview 获取独占锁，等待其他工具完成
  [调用: start_preview({})]
  [结果: start_preview] ✓ 预览服务器已启动 → http://localhost:8080

--- Step 2 ---
搞定！我已经在 app/ 目录下生成了一个待办清单应用：
  - styles.css  样式（紫色渐变背景、卡片式布局）
  - Button.tsx  可复用的按钮组件
  - App.tsx     主应用入口（用 createRoot 渲染到 #root）

模板自带的 app/index.html 是固定的 ESM bootstrap，不需要重新生成。

✓ 预览服务器已启动 → http://localhost:8080


start_preview 自动排到所有 write 之后，保证文件全部就位再启动服务。

整个流程一气呵成——Agent 写完所有应用文件、立刻启动预览服务器、输出可点击的 URL。在 WebContainer 里点 Preview 标签就能看到一个紫色渐变背景、卡片式布局的待办清单，能输入、能勾选、能删除。本地终端跑的话，浏览器直接打开 http://localhost:8080。

浏览器里直接跑 TSX 的原理

预置的 app/index.html 是这个 demo 除了 Agent 之外真正的精髓——没有 webpack、没有 Vite、没有任何 build step，纯静态 HTML 直接把 TSX 在浏览器里跑起来。

原理分三层。

第一层是 importmap——浏览器原生支持的特性，告诉浏览器"react 这个 bare specifier 解析到 https://esm.sh/react@18"。esm.sh 能把 npm 包重新打包成 ES module，相当于一个免构建的 npm 替代。

第二层是 Babel Standalone——Babel 的浏览器版本，能在运行时把 TSX 代码翻译成浏览器能跑的 ES module。

第三层是一个手写的小型加载器：fetch 一个 .tsx 文件，用 Babel 编译成 JS，处理里面的 from './X.tsx' 这类相对路径 import（递归编译被引用的文件，把每个编译结果包成 Blob URL 给浏览器加载），最后用动态 import() 跑入口模块。

app/index.html
应用
复制
<!-- 简化后的核心逻辑 -->
<script type="module">
  const { transform } = await import('https://esm.sh/@babel/standalone@7.25.6');
  const cache = new Map();

  async function loadModule(url) {
    if (cache.has(url)) return cache.get(url);
    const src = await (await fetch(url)).text();
    const compiled = transform(src, {
      presets: [['react'], ['typescript', { allExtensions: true, isTSX: true }]],
      filename: url,
    }).code;
    const resolved = await rewriteRelativeImports(compiled, url);
    const blob = new Blob([resolved], { type: 'application/javascript' });
    const blobUrl = URL.createObjectURL(blob);
    cache.set(url, blobUrl);
    return blobUrl;
  }

  await import(await loadModule(new URL('./App.tsx', location.href).href));
</script>


为什么把这段 loader 预置在模板里、不让 Agent 自己生成？

它是纯基础设施——每个 vibe coding 项目的 bootstrap 都长一样，让模型每次重新生成既慢又不可靠（一个 typo 就让整个 demo 跑不起来）。v0、Bolt.new 这些 vibe coding 产品的实现方式都是同一套思路：固定脚手架加上模型只负责生成应用代码。把基础设施和应用代码分开，让模型专注做它擅长的事——根据需求写 React 组件，而不是去维护脚手架。

整个方案的亮点是零安装、零配置、所见即所得。生成出来的项目用编辑器打开就是几个普通的 .tsx 和 .css 文件，浏览器直接能跑。

虽然 Babel 在浏览器里编译 TSX 性能差、首次加载慢、缺少 tree-shaking。但作为"让 Agent 在沙箱里把想法变成可运行的产品"的原型工具，它非常合适。

用真实模型自由发挥

mock 模型只会生成预设好的待办清单。但配上 DASHSCOPE_API_KEY 切到真实 Qwen 之后，就能让它生成任何东西：

You: 做一个 markdown 编辑器，左边写、右边实时预览
You: 做一个像 Wordle 那样的猜词游戏
You: 做一个简易计算器，键盘也能用


Agent 会根据需求自己决定文件结构——可能 4 个文件，也可能 7 个、8 个。这才是真正的 Vibe Coding：你描述意图，Agent 把它落地成代码。

这三个 demo 说明了什么

回头看，你会发现它们的 Agent 价值都不在工具本身，而在工具的组合方式。

代码分析靠的是 list_directory + grep + 模型的归纳能力。Research Agent 靠的是 fetch_url + 截断 + 模型的摘要能力。Vibe Coding 靠的是 write_file 的多次调用 + 模型的代码生成能力。

单个工具虽然不起眼，组合在一起就涌现出产品形态。 这就是 Tool System 这一章一直在搭的东西——不只是给模型一双手，而是给它一套能拼出各种可能的零件。

而且从 Agent Loop 到现在，你写的每一行代码都没有为某个特定 demo 服务。读写锁不是为代码分析 Agent 设计的，截断也不是为 Research Agent 设计的——它们是通用的基础设施，今天能拼出三个 demo，明天能拼出三十个完全不同的应用。

下一篇我们接 MCP（Model Context Protocol）。前面讲 Tool System 的时候提过一个问题——所有工具都是你自己写的、自己注册的。但 GitHub、数据库、飞书、邮件这些第三方工具都自己写一遍既不现实也不必要。

MCP 解决的就是这个：让 Agent 能直接连接外部的 MCP Server，用别人写好的工具。到时候 ToolRegistry 会多一个 registerMCPTools() 方法——MCP 工具和内置工具共享同一套截断和读写锁策略，Agent Loop 完全感知不到差异。我们下一节再见。

参考链接
esm.sh — 给 npm 包提供 ES module 形式的 CDN
Import maps - MDN
Babel Standalone — Babel 的浏览器运行时版本
Vercel AI SDK - Tool Reference
上一篇
补齐装备——edit_file、grep、glob 与 bash
下一篇 · 第二章：Tool System
加餐：Agent 的 Search 工具究竟是如何来实现的？
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
const MOCK_PAGES: Record<string, string> = {
  'https://esm.sh': `esm.sh - 一个免费的 ES module CDN...`,
  'https://ai-sdk.dev/docs/ai-sdk-core/tools-and-tool-calling': `AI SDK Core - Tools and Tool Calling
工具是模型可以决定调用的函数。一个工具由三部分组成：
- description：告诉模型何时使用这个工具
- inputSchema：通过 Zod 或 JSON Schema 定义参数
- execute：实际在服务端运行的函数...`,
  // ... 更多预定义页面
};

export const fetchUrlTool: ToolDefinition = {
  name: 'fetch_url',
  description: '抓取指定 URL 的网页内容并转换为纯文本（自动剥离 HTML 标签）',
  parameters: {
    type: 'object',
    properties: {
      url: { type: 'string', description: '完整 URL，必须以 http:// 或 https:// 开头' },
    },
    required: ['url'],
    additionalProperties: false,
  },
  isConcurrencySafe: true,    // 只读、可并发——抓多个 URL 时直接并行
  isReadOnly: true,
  maxResultChars: 1500,        // 网页通常很长，截断兜底
  execute: async ({ url }: { url: string }) => {
    for (const key of Object.keys(MOCK_PAGES)) {
      if (url.startsWith(key)) return MOCK_PAGES[key];
    }
    try {
      const res = await fetch(url, {
        headers: { 'User-Agent': 'Mozilla/5.0 SuperAgent' },
        signal: AbortSignal.timeout(10000),
      });
      if (!res.ok) return `请求失败：HTTP ${res.status}`;
      const html = await res.text();
      return html
        .replace(/<script[\s\S]*?<\/script>/gi, '')
        .replace(/<style[\s\S]*?<\/style>/gi, '')
        .replace(/<[^>]+>/g, ' ')
        .replace(/\s+/g, ' ')
        .trim() || '页面无文本内容';
    } catch (err: any) {
      return `抓取失败：${err.message}`;
    }
  },
};
```


```ts
import { createServer, type Server } from 'node:http';

let previewServer: Server | null = null;

const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.tsx': 'application/javascript; charset=utf-8',  // 让浏览器把 .tsx 当 JS 加载
  '.ts': 'application/javascript; charset=utf-8',
  // ...
};

export const startPreviewTool: ToolDefinition = {
  name: 'start_preview',
  description: '启动 app/ 目录的预览服务器。生成应用文件后必须立即调用此工具',
  parameters: {
    type: 'object',
    properties: { port: { type: 'number' } },
    required: [],
    additionalProperties: false,
  },
  isConcurrencySafe: false,
  isReadOnly: false,
  execute: async ({ port = 8080 }: { port?: number } = {}) => {
    if (previewServer) return `预览服务器已在运行 → http://localhost:${port}`;
    const root = resolve('app');
    if (!existsSync(root)) return '错误：app/ 目录不存在';

    previewServer = createServer((req, res) => {
      const urlPath = (req.url?.split('?')[0] || '/').replace(/\/$/, '/index.html');
      const filePath = join(root, urlPath === '/' ? '/index.html' : urlPath);
      try {
        if (!filePath.startsWith(root)) { res.writeHead(403); res.end(); return; }
        res.writeHead(200, {
          'Content-Type': MIME[extname(filePath).toLowerCase()] || 'application/octet-stream',
          'Cache-Control': 'no-cache',
        });
        res.end(readFileSync(filePath));
      } catch { res.writeHead(404); res.end('Not Found'); }
    });

    return new Promise<string>((resolve) => {
      previewServer!.listen(port, () => {
        resolve(`✓ 预览服务器已启动 → http://localhost:${port}`);
      });
    });
  },
};
```


```ts
export const allTools: ToolDefinition[] = [
  weatherTool, calculatorTool,
  readFileTool, writeFileTool, listDirectoryTool,
  editFileTool, globTool, grepTool, bashTool,
  fetchUrlTool,        // ← 新增 1
  startPreviewTool,    // ← 新增 2
];
```


```bash
pnpm start
```


```
You: 找出项目里所有 TODO
```


```
--- Step 1 ---
  [并发] list_directory 获取共享锁
  [调用: list_directory({"path":"sample-project"})]
  [结果: list_directory] [FILE] api.ts
  [FILE] auth.ts
  [FILE] utils.ts
  [并发] grep 获取共享锁
  [调用: grep({"pattern":"TODO|FIXME","path":"sample-project"})]
  [结果: grep] api.ts:13:   // TODO: 加上数据库查询，目前只用了内存 Map
  api.ts:18:   // FIXME: id 生成方式应该换成 nanoid，时间戳容易冲突
  api.ts:25:   // TODO: 软删除而不是物理删除
  ... [省略] ...

--- Step 2 ---
项目里一共找到 8 处 TODO/FIXME，分布在 3 个文件：

- api.ts（4 处）
   * TODO: 加上数据库查询，目前只用了内存 Map
   * FIXME: id 生成方式应该换成 nanoid，时间戳容易冲突
   * TODO: 软删除而不是物理删除
- auth.ts（3 处）
   * TODO: 接入真正的 password hash 校验（bcrypt 或 argon2）
   * FIXME: 这里硬编码了 admin 用户用于联调，上线前必须删除
   * TODO: 改成 JWT 校验，目前只是字符串前缀判断
- utils.ts（1 处）
   * TODO: 支持多时区，目前只输出本地时间

建议优先处理 FIXME 标记的几条，那些是上线前必须修的硬伤。
```


```
You: 去 https://ai-sdk.dev/docs/ai-sdk-core/tools-and-tool-calling 看下文档总结
```


```
You: 同时去 https://esm.sh 和 https://ai-sdk.dev/docs/ai-sdk-core/generating-text 看看，对比一下
```


```
You: 做一个待办清单的网页应用
```


```
app/
├── index.html     ← 预置脚手架，固定加载 ./App.tsx 作为入口
├── App.tsx        ← Agent 写：主入口，必须有 createRoot
├── Button.tsx     ← Agent 写：可被 App.tsx import 的组件
└── styles.css     ← Agent 写：样式
```


```
--- Step 1 ---
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/styles.css", ...})]
  [结果: write_file] 已写入 1386 字符到 app/styles.css
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/Button.tsx", ...})]
  [结果: write_file] 已写入 336 字符到 app/Button.tsx
  [串行] write_file 获取独占锁，等待其他工具完成
  [调用: write_file({"path":"app/App.tsx", ...})]
  [结果: write_file] 已写入 1627 字符到 app/App.tsx
  [串行] start_preview 获取独占锁，等待其他工具完成
  [调用: start_preview({})]
  [结果: start_preview] ✓ 预览服务器已启动 → http://localhost:8080

--- Step 2 ---
搞定！我已经在 app/ 目录下生成了一个待办清单应用：
  - styles.css  样式（紫色渐变背景、卡片式布局）
  - Button.tsx  可复用的按钮组件
  - App.tsx     主应用入口（用 createRoot 渲染到 #root）

模板自带的 app/index.html 是固定的 ESM bootstrap，不需要重新生成。

✓ 预览服务器已启动 → http://localhost:8080
```


```html
<!-- 简化后的核心逻辑 -->
<script type="module">
  const { transform } = await import('https://esm.sh/@babel/standalone@7.25.6');
  const cache = new Map();

  async function loadModule(url) {
    if (cache.has(url)) return cache.get(url);
    const src = await (await fetch(url)).text();
    const compiled = transform(src, {
      presets: [['react'], ['typescript', { allExtensions: true, isTSX: true }]],
      filename: url,
    }).code;
    const resolved = await rewriteRelativeImports(compiled, url);
    const blob = new Blob([resolved], { type: 'application/javascript' });
    const blobUrl = URL.createObjectURL(blob);
    cache.set(url, blobUrl);
    return blobUrl;
  }

  await import(await loadModule(new URL('./App.tsx', location.href).href));
</script>
```


```
You: 做一个 markdown 编辑器，左边写、右边实时预览
You: 做一个像 Wordle 那样的猜词游戏
You: 做一个简易计算器，键盘也能用
```
