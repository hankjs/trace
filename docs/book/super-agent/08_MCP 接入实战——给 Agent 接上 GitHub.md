# MCP 接入实战——给 Agent 接上 GitHub

课程
Super Agent 实战课
MCP 接入实战——给 Agent 接上 GitHub
MCP 接入实战——给 Agent 接上 GitHub

约 43 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

你现在的 Agent 有 9 个内置工具，能读文件、编辑代码、跑命令。但试试让它干点超出本地文件系统的事，它会输出类似的回答：

You: 帮我看看 vercel/ai 仓库有什么 open issues

--- Step 1 ---
Agent: 我可以帮你查看 vercel/ai 的 issues，但我目前没有访问 GitHub 的工具……


原因很明显，它缺少了对应的 Github 工具。

你当然可以手写一个 list_github_issues 工具。调 GitHub REST API，解析返回，包装成 ToolDefinition，注册到 Registry。但 GitHub API 有几十个端点——Issues、PRs、搜索、文件内容、Releases……全都手写一遍，工作量就上来了。而且不光是 Github，每个平台的 API 格式不同、认证方式不同、错误处理不同，维护成本很高。

这就是 MCP（Model Context Protocol） 要解决的问题。一个 MCP Server 暴露一组工具，任何支持 MCP 的 Agent 都能直接用。GitHub 社区已经有现成的 MCP Server，装上就能给你的 Agent 加 26 个 GitHub 工具——从查 Issues 到合并 PR，非常方便。

当然，这篇不是教你怎么在现有的工具（例如 Claude Code、Cursor）里面接入一个 MCP Server，那个太浮于表面了，这篇真正要教的是作为一个开发 Agent 的人，怎么在自己的 Agent 工程实战里面适配 MCP 协议，这样可以无缝接入任何的 MCP Server 生态。

首先我们来亲手写一个 MCP Client，把 GitHub MCP Server 接入到 ToolRegistry 里。

先装依赖：

bash
运行
复制
pnpm install

MCP 在传输层做了什么

在开始写代码之前，先搞清楚 MCP 在工程实现的层面到底是个什么东西。

如果你学过知识体系课的 MCP 那篇，应该已经了解它的设计理念。这里我们只关注实战需要的部分：MCP 的通信协议是 JSON-RPC 2.0，传输方式支持 stdio 和 Streamable HTTP。 我们这篇用 stdio——启动一个本地进程，通过标准输入输出收发消息，最简单直接。

具体来说，你的 Agent（Client）启动一个 MCP Server 进程，通过 stdin 发 JSON 消息，从 stdout 收 JSON 响应。整个交互就三步：

握手——Client 发 initialize method，Server 回复它支持的能力
发现工具——Client 发 tools/list method，Server 返回所有工具的名称、描述、参数 Schema
调用工具——模型决定调某个 MCP 工具时，Client 发 tools/call method，Server 执行并返回结果

这里的 initialize、tools/list、tools/call 都是 MCP 规范定义的 JSON-RPC method 名，后面写代码时你会看到它们直接出现在 send() 调用里。

来看一个真实的 tools/list 响应（GitHub MCP Server 返回的）：

json
复制
{
  "tools": [
    {
      "name": "list_issues",
      "description": "List issues in a GitHub repository",
      "inputSchema": {
        "type": "object",
        "properties": {
          "owner": { "type": "string" },
          "repo": { "type": "string" }
        },
        "required": ["owner", "repo"]
      }
    }
  ]
}


注意看这个结构——name、description、inputSchema。跟我们 ToolDefinition 的字段几乎一模一样。这不是巧合，Tool Calling 的参数格式本来就是 JSON Schema，MCP 直接复用了这套标准。这意味着 MCP 工具的 Schema 天然就能注册进 ToolRegistry，不需要任何格式转换。

写一个 MCP Client

明白了协议，开始写代码。创建 src/mcp-client.ts：

src/tools/mcp-client.ts
应用
复制
import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface, type Interface } from 'node:readline';

interface MCPTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

interface MCPCallResult {
  content: Array<{ type: string; text?: string }>;
  isError?: boolean;
}

export class MCPClient {
  private process: ChildProcess | null = null;
  private rl: Interface | null = null;
  private requestId = 0;
  private pending = new Map<number, {
    resolve: (v: any) => void;
    reject: (e: Error) => void;
  }>();
  private serverName: string;

  constructor(
    private command: string,
    private args: string[],
    private env?: Record<string, string>,
  ) {
    this.serverName = args[args.length - 1]?.replace(/^@.*\//, '')
      || 'mcp-server';
  }

  async connect(): Promise<void> {
    this.process = spawn(this.command, this.args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, ...this.env },
    });

    this.process.on('error', (err) => {
      console.error(`  [MCP] 进程启动失败: ${err.message}`);
    });
    this.process.stderr?.on('data', () => {});

    this.rl = createInterface({ input: this.process.stdout! });
    this.rl.on('line', (line) => {
      try {
        const msg = JSON.parse(line);
        if (msg.id !== undefined && this.pending.has(msg.id)) {
          const p = this.pending.get(msg.id)!;
          this.pending.delete(msg.id);
          if (msg.error) {
            p.reject(new Error(
              `MCP error ${msg.error.code}: ${msg.error.message}`
            ));
          } else {
            p.resolve(msg.result);
          }
        }
      } catch { /* ignore non-JSON lines */ }
    });

    await this.send('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'super-agent', version: '0.5.0' },
    });

    this.process.stdin!.write(JSON.stringify({
      jsonrpc: '2.0',
      method: 'notifications/initialized',
    }) + '\n');
  }

  private send(method: string, params?: any): Promise<any> {
    return new Promise((resolve, reject) => {
      const id = ++this.requestId;
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request timeout: ${method}`));
      }, 15000);

      this.pending.set(id, {
        resolve: (v: any) => { clearTimeout(timeout); resolve(v); },
        reject: (e: Error) => { clearTimeout(timeout); reject(e); },
      });

      const msg = JSON.stringify({ jsonrpc: '2.0', id, method, params });
      this.process!.stdin!.write(msg + '\n');
    });
  }

  async listTools(): Promise<MCPTool[]> {
    const result = await this.send('tools/list', {});
    return result.tools || [];
  }

  async callTool(
    name: string, args: Record<string, unknown>
  ): Promise<string> {
    const result: MCPCallResult = await this.send(
      'tools/call', { name, arguments: args }
    );
    const texts = (result.content || [])
      .filter(c => c.type === 'text' && c.text)
      .map(c => c.text!);
    return texts.join('\n') || '(无返回内容)';
  }

  async close(): Promise<void> {
    if (this.rl) this.rl.close();
    if (this.process) this.process.kill();
  }
}


代码量不少，但核心逻辑其实就两个：

connect() 用 child_process.spawn 启动 Server 进程，然后逐行读 stdout。每收到一行 JSON，就按 id 字段去 pending Map 里找到对应的 Promise 并 resolve。这是所有 RPC 客户端的标准套路——请求和响应是异步的，靠 id 匹配。

这里有个容易忽略的细节：JSON-RPC 2.0 的请求和响应是异步交错的——可能多个请求的响应乱序到达。所以 pending Map + id 匹配是必须的，不能用"发一个等一个"的同步模式。

send() 往 stdin 写一条 JSON-RPC 消息，同时在 pending Map 里注册一个 Promise 等待响应。加了 15 秒超时——MCP Server 有可能因为网络问题或者内部错误卡住不回复，你不能让整个 Agent 一直挂着等一个永远不会来的响应。

为什么不用官方的 @modelcontextprotocol/sdk？两个原因：一是 SDK 拉一堆依赖，体积不小；二是自己写一个能让你真正理解 MCP 在传输层做了什么——说到底就是 JSON-RPC 协议，没有魔法。生产环境里你可以直接用官方 SDK，但至少你知道它底下做了什么。

Mock 降级：WebContainer 里也能跑

如果你是在本地跑代码，可以忽略这个部分。

WebContainer（浏览器沙箱）没有 child_process，没法启动 MCP Server 进程。所以我们需要一个 MockMCPClient，返回预设的 GitHub 数据，让你在右侧的 WebContainer 里也能跑通完整流程。

在 mcp-client.ts 底部加上：

typescript
复制
export class MockMCPClient {
  async connect(): Promise<void> {}

  async listTools(): Promise<MCPTool[]> {
    return [
      {
        name: 'list_issues',
        description: '列出 GitHub 仓库的 Issues',
        inputSchema: {
          type: 'object',
          properties: {
            owner: { type: 'string', description: '仓库所有者' },
            repo: { type: 'string', description: '仓库名称' },
          },
          required: ['owner', 'repo'],
        },
      },
      {
        name: 'search_repositories',
        description: '搜索 GitHub 仓库',
        inputSchema: {
          type: 'object',
          properties: {
            query: { type: 'string', description: '搜索关键词' },
          },
          required: ['query'],
        },
      },
      {
        name: 'get_file_contents',
        description: '获取仓库中文件的内容',
        inputSchema: {
          type: 'object',
          properties: {
            owner: { type: 'string', description: '仓库所有者' },
            repo: { type: 'string', description: '仓库名称' },
            path: { type: 'string', description: '文件路径' },
          },
          required: ['owner', 'repo', 'path'],
        },
      },
    ];
  }

  async callTool(
    name: string, args: Record<string, unknown>
  ): Promise<string> {
    switch (name) {
      case 'list_issues':
        return JSON.stringify([
          { number: 42, title: '支持 MCP 协议接入', state: 'open' },
          { number: 41, title: '循环检测阈值可配置化', state: 'open' },
          { number: 39, title: 'Token 预算用完后的优雅降级', state: 'closed' },
        ], null, 2);
      case 'search_repositories':
        return JSON.stringify([
          { full_name: 'anthropics/anthropic-sdk-python', stars: 2800 },
          { full_name: 'vercel/ai', stars: 12000 },
          { full_name: 'modelcontextprotocol/servers', stars: 5600 },
        ], null, 2);
      case 'get_file_contents':
        return `# README\n\nMock file: ${args.owner}/${args.repo}/${args.path}`;
      default:
        return `未知工具: ${name}`;
    }
  }

  async close(): Promise<void> {}
}


两个类实现了相同的接口——connect、listTools、callTool、close。上层代码不需要关心底层是真连接还是 mock。这跟我们之前 Mock Model 的思路完全一样。

命名空间隔离

在把 MCP 工具注册到 Registry 之前，有一个问题要解决：工具名冲突。

一个 GitHub MCP Server 就暴露了 26 个工具。create_issue 这种通用名字，换个 Jira MCP Server 也叫 create_issue。如果直接注册，两个 Server 的同名工具会互相覆盖。

解决办法是加命名空间前缀：mcp__<serverName>__<toolName>。比如 list_issues 会被转换为 mcp__github__list_issues。

Claude Code 用的也是这个方式——它的所有 MCP 工具名都是 mcp__<server>__<tool> 格式。模型看到前缀就知道这是外部工具，System Prompt 里也方便按前缀引用。

给 ToolRegistry 加 MCP 注册能力

现在 ToolRegistry 需要一个新方法：连接 MCP Server，发现它暴露的工具，然后自动注册到 Registry 里。

在 src/tool-registry.ts 里新增 registerMCPServer 方法和 closeAllMCP 方法：

src/tools/registry.ts
应用
复制
// ... ToolRegistry 已有代码不变 ...
// 新增以下内容：

private mcpClients: Array<MCPClient | MockMCPClient> = [];

async registerMCPServer(
  serverName: string,
  client: MCPClient | MockMCPClient,
): Promise<string[]> {
  await client.connect();
  this.mcpClients.push(client);

  const tools = await client.listTools();
  const registered: string[] = [];

  for (const tool of tools) {
    const prefixedName = `mcp__${serverName}__${tool.name}`;
    if (this.tools.has(prefixedName)) continue;

    const toolClient = client;
    const originalName = tool.name;

    this.register({
      name: prefixedName,
      description: `[MCP:${serverName}] ${tool.description}`,
      parameters: tool.inputSchema as Record<string, unknown>,
      isConcurrencySafe: true,
      isReadOnly: true,
      maxResultChars: 3000,
      execute: async (input: any) => {
        return toolClient.callTool(originalName, input);
      },
    });

    registered.push(prefixedName);
  }

  return registered;
}

async closeAllMCP(): Promise<void> {
  for (const client of this.mcpClients) {
    await client.close();
  }
  this.mcpClients = [];
}


这个方法做的事情很符合直觉：先连接 MCP，再拿工具列表，然后逐个注册。每个 MCP 工具的 execute 函数就是一个闭包，调用时通过 JSON-RPC 转发给 Server。

有两个设计决策值得说一下：

isConcurrencySafe: true——MCP 工具通常是无状态的 API 调用（查 issue、搜仓库），天然可以并发。如果某个 Server 暴露了写操作（比如 create_issue），严格来说应该标记为 false，后续权限系统那篇会做更细的控制。

description 加了 [MCP:github] 前缀——这不是给模型看的，是给你调试看的。当 Agent 调了一个工具但结果不对，日志里一眼就能分辨是内置工具的问题还是 MCP Server 的问题。

MCP 工具注册后，和内置工具共享同一套截断策略和并发控制。Agent Loop 完全不需要区分工具来源——它只看 ToolRegistry 里有什么。这就是 Registry 模式的好处：加一种新的工具来源，不需要改 Agent Loop 的一行代码。

连接起来

入口文件里加上 MCP 的连接逻辑。有 GitHub token + 能 spawn 进程就连真实 MCP Server，否则降级 Mock：

src/index.ts
应用
复制
// ... import 和模型初始化同上一篇 ...
import { MCPClient, MockMCPClient } from './mcp-client.js';

const registry = new ToolRegistry();
registry.register(...allTools);

async function connectMCP() {
  const githubToken = process.env.GITHUB_PERSONAL_ACCESS_TOKEN;

  let canSpawn = true;
  try {
    const { execSync } = await import('node:child_process');
    execSync('echo test', { stdio: 'ignore' });
  } catch {
    canSpawn = false;
  }

  if (githubToken && canSpawn) {
    console.log('\n连接 GitHub MCP Server...');
    try {
      const client = new MCPClient(
        'npx', ['-y', '@modelcontextprotocol/server-github'],
        { GITHUB_PERSONAL_ACCESS_TOKEN: githubToken },
      );
      const tools = await registry.registerMCPServer('github', client);
      console.log(`  已注册 ${tools.length} 个 MCP 工具`);
      return;
    } catch (err) {
      console.log(`  MCP 连接失败: ${err instanceof Error ? err.message : err}`);
      console.log('  降级为 Mock MCP...');
    }
  }

  if (!githubToken) {
    console.log('\n未配置 GITHUB_PERSONAL_ACCESS_TOKEN，使用 Mock MCP');
  }

  const mockClient = new MockMCPClient();
  const tools = await registry.registerMCPServer('github', mockClient);
  console.log(`  已注册 ${tools.length} 个 Mock MCP 工具`);
}

// ... main() 调用 connectMCP 后启动 REPL ...


connectMCP 里做了三层降级：如果有 token 并且环境支持 spawn，就连真实的 GitHub MCP Server；如果连接过程出错了，自动降级到 Mock；如果压根没配 token，直接用 Mock。保证 Agent 没有 MCP 也能正常工作，有了 MCP 只是多了外部能力。

跑起来看看：

bash
运行
复制
pnpm start

未配置 GITHUB_PERSONAL_ACCESS_TOKEN，使用 Mock MCP
  已注册 3 个 Mock MCP 工具

已注册 12 个工具：
  - get_weather（内置, 可并发）
  - calculator（内置, 可并发）
  - read_file（内置, 可并发）
  - write_file（内置, 串行）
  - list_directory（内置, 可并发）
  - edit_file（内置, 串行）
  - glob（内置, 可并发）
  - grep（内置, 可并发）
  - bash（内置, 串行）
  - mcp__github__list_issues（MCP, 可并发）
  - mcp__github__search_repositories（MCP, 可并发）
  - mcp__github__get_file_contents（MCP, 可并发）


9 个内置 + 3 个 MCP，共 12 个工具。MCP 工具带着 mcp__github__ 前缀，一眼就能分辨来源。

试试查 issues：

You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
  { "number": 42, "title": "支持 MCP 协议接入", "state": "open" },
  ...
  → 继续下一步...

--- Step 2 ---
vercel/ai 仓库目前有以下 issues：
- #42 支持 MCP 协议接入（open）
- #41 循环检测阈值可配置化（open）
- #39 Token 预算用完后的优雅降级（closed）


模型正确选择了 mcp__github__list_issues。整个过程和调内置工具一模一样——因为 MCP 工具已经在 ToolRegistry 里了，Agent Loop 不知道也不需要知道它来自外部 Server。

连接真实 GitHub MCP Server

在 .env 里填上 GITHUB_PERSONAL_ACCESS_TOKEN（从 GitHub Settings 生成，给 repo 权限就行），在本地终端 pnpm start 就能连接真实的 GitHub MCP Server：

GitHub 后来推出了官方的 MCP Server，基于 Docker 运行。不过 Docker 启动成本比较高，我们这里用的 @modelcontextprotocol/server-github 这个 npm 包通过 npx 直接跑，GitHub API 本身非常稳定，实际使用完全没问题。

连接 GitHub MCP Server...
  已注册 26 个 MCP 工具

已注册 35 个工具：
  - get_weather（内置, 可并发）
  - ...（省略其他 8 个内置工具）
  - mcp__github__list_issues（MCP, 可并发）
  - mcp__github__search_repositories（MCP, 可并发）
  - mcp__github__create_issue（MCP, 可并发）
  - mcp__github__list_pull_requests（MCP, 可并发）
  - mcp__github__search_code（MCP, 可并发）
  - ...（共 26 个 GitHub 工具）


26 个工具——从 list_issues 到 merge_pull_request。这就是 MCP 的价值：社区写一次 Server，你一行代码接进来就能用。

试试真实数据：

You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
    { "number": 14625, "title": "feat(ai/ui): allow dynamic tools..." },
    { "number": 14476, "title": "docs: add ToolLoopAgent example..." },
    ...

--- Step 2 ---
vercel/ai 仓库的 issues：
- #14625（PR）：feat(ai/ui): allow dynamic tools to pass UI message validation
- #14476（PR）：docs: add ToolLoopAgent example


这是实时的 GitHub 数据。模型自动选了 mcp__github__list_issues，拿到结果后整理成可读的列表。整个过程——从工具发现到工具调用到结果截断——全部走 ToolRegistry 的统一管线。

不过你可能也注意到了另一个现象：启动变慢了。MCP Server 是一个独立进程，npx 要下载包、启动 Node、初始化连接、发现 26 个工具，整个过程可能要 3-5 秒。而且 Server 挂了怎么办？网络断了怎么办？回头看 connectMCP 函数——MCP 连接失败就降级 Mock，Agent 核心功能不受影响。这个三层降级设计不是过度工程，是实际跑起来之后你一定会遇到的问题。

生产环境：用官方 SDK

前面我们手写了 MCPClient，是为了搞清楚 MCP 在传输层到底做了什么。但生产环境里你不会自己维护 JSON-RPC 的请求匹配、超时处理、协议版本协商这些细节。官方提供了 @modelcontextprotocol/sdk，用它替换手写的 Client 非常简单：

typescript
复制
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const transport = new StdioClientTransport({
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-github'],
  env: { GITHUB_PERSONAL_ACCESS_TOKEN: token },
});

const client = new Client({ name: 'super-agent', version: '1.0.0' });
await client.connect(transport);

const { tools } = await client.listTools();
const result = await client.callTool({
  name: 'list_issues',
  arguments: { owner: 'vercel', repo: 'ai' },
});


对比我们手写的版本——StdioClientTransport 替代了 spawn + readline + JSON 行解析，Client 替代了 pending Map + id 匹配 + 超时处理。API 层面几乎一样：listTools()、callTool()，连方法名都没变。

所以 ToolRegistry 的 registerMCPServer 方法几乎不用改——把参数类型从自定义的 MCPClient 换成官方的 Client，listTools 和 callTool 的返回格式稍微适配一下就行。

架构设计是通用的，底层实现随时可以换。有了上面的手写过程之后，相信你对这个 SDK 的理解也比一般人更深了一步。

MCP 的隐性成本：Token 开销

到这里 MCP 看起来很美好，但有一个问题你可能已经注意到了：接上真实 GitHub MCP Server 后，工具从 9 个变成了 35 个。

每个工具的名称、描述、参数 Schema 都要塞进 prompt——这些是模型做工具选择的依据，少了任何一个字段模型就不知道怎么调。实测数据：26 个 GitHub MCP 工具的描述加起来大约 8000-12000 token。

这还只是一个 Server。Playwright MCP 有 20+ 个浏览器操作工具，接进来又是几千 token。如果你同时接了 GitHub + Playwright + 一个数据库 MCP，光工具定义就吃掉 30000+ token——200K 上下文窗口的 15%，还没开始干活就没了。

更微妙的是，如果你学过知识体系课讲 KV Cache 那篇的话，会知道 prompt 前缀的稳定性决定了 cache 命中率。工具列表一变，整个 cache 就废了。所以你也不能简单地"用完就删、下次再加"——频繁增删工具定义等于频繁禁用 cache，每次都全价计费。

这个矛盾引出了下一篇的主题：动态工具集。核心思路是不是所有场景都需要所有工具，按需加载，才能在工具数量膨胀的情况下仍然控制好 Agent 的上下文。我们下一篇接着来实战。

参考链接
MCP 官方文档
MCP Server 列表（官方仓库）
GitHub MCP Server
MCP TypeScript SDK
Anthropic - Advanced Tool Use
上一篇
加餐：Agent 的 Search 工具究竟是如何来实现的？
下一篇 · 第二章：Tool System
工具太多模型选不准——实现 ToolSearch
编辑器


---
## 代码块


```
You: 帮我看看 vercel/ai 仓库有什么 open issues

--- Step 1 ---
Agent: 我可以帮你查看 vercel/ai 的 issues，但我目前没有访问 GitHub 的工具……
```


```bash
pnpm install
```


```json
{
  "tools": [
    {
      "name": "list_issues",
      "description": "List issues in a GitHub repository",
      "inputSchema": {
        "type": "object",
        "properties": {
          "owner": { "type": "string" },
          "repo": { "type": "string" }
        },
        "required": ["owner", "repo"]
      }
    }
  ]
}
```


```typescript
import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface, type Interface } from 'node:readline';

interface MCPTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

interface MCPCallResult {
  content: Array<{ type: string; text?: string }>;
  isError?: boolean;
}

export class MCPClient {
  private process: ChildProcess | null = null;
  private rl: Interface | null = null;
  private requestId = 0;
  private pending = new Map<number, {
    resolve: (v: any) => void;
    reject: (e: Error) => void;
  }>();
  private serverName: string;

  constructor(
    private command: string,
    private args: string[],
    private env?: Record<string, string>,
  ) {
    this.serverName = args[args.length - 1]?.replace(/^@.*\//, '')
      || 'mcp-server';
  }

  async connect(): Promise<void> {
    this.process = spawn(this.command, this.args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, ...this.env },
    });

    this.process.on('error', (err) => {
      console.error(`  [MCP] 进程启动失败: ${err.message}`);
    });
    this.process.stderr?.on('data', () => {});

    this.rl = createInterface({ input: this.process.stdout! });
    this.rl.on('line', (line) => {
      try {
        const msg = JSON.parse(line);
        if (msg.id !== undefined && this.pending.has(msg.id)) {
          const p = this.pending.get(msg.id)!;
          this.pending.delete(msg.id);
          if (msg.error) {
            p.reject(new Error(
              `MCP error ${msg.error.code}: ${msg.error.message}`
            ));
          } else {
            p.resolve(msg.result);
          }
        }
      } catch { /* ignore non-JSON lines */ }
    });

    await this.send('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'super-agent', version: '0.5.0' },
    });

    this.process.stdin!.write(JSON.stringify({
      jsonrpc: '2.0',
      method: 'notifications/initialized',
    }) + '\n');
  }

  private send(method: string, params?: any): Promise<any> {
    return new Promise((resolve, reject) => {
      const id = ++this.requestId;
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request timeout: ${method}`));
      }, 15000);

      this.pending.set(id, {
        resolve: (v: any) => { clearTimeout(timeout); resolve(v); },
        reject: (e: Error) => { clearTimeout(timeout); reject(e); },
      });

      const msg = JSON.stringify({ jsonrpc: '2.0', id, method, params });
      this.process!.stdin!.write(msg + '\n');
    });
  }

  async listTools(): Promise<MCPTool[]> {
    const result = await this.send('tools/list', {});
    return result.tools || [];
  }

  async callTool(
    name: string, args: Record<string, unknown>
  ): Promise<string> {
    const result: MCPCallResult = await this.send(
      'tools/call', { name, arguments: args }
    );
    const texts = (result.content || [])
      .filter(c => c.type === 'text' && c.text)
      .map(c => c.text!);
    return texts.join('\n') || '(无返回内容)';
  }

  async close(): Promise<void> {
    if (this.rl) this.rl.close();
    if (this.process) this.process.kill();
  }
}
```


```typescript
export class MockMCPClient {
  async connect(): Promise<void> {}

  async listTools(): Promise<MCPTool[]> {
    return [
      {
        name: 'list_issues',
        description: '列出 GitHub 仓库的 Issues',
        inputSchema: {
          type: 'object',
          properties: {
            owner: { type: 'string', description: '仓库所有者' },
            repo: { type: 'string', description: '仓库名称' },
          },
          required: ['owner', 'repo'],
        },
      },
      {
        name: 'search_repositories',
        description: '搜索 GitHub 仓库',
        inputSchema: {
          type: 'object',
          properties: {
            query: { type: 'string', description: '搜索关键词' },
          },
          required: ['query'],
        },
      },
      {
        name: 'get_file_contents',
        description: '获取仓库中文件的内容',
        inputSchema: {
          type: 'object',
          properties: {
            owner: { type: 'string', description: '仓库所有者' },
            repo: { type: 'string', description: '仓库名称' },
            path: { type: 'string', description: '文件路径' },
          },
          required: ['owner', 'repo', 'path'],
        },
      },
    ];
  }

  async callTool(
    name: string, args: Record<string, unknown>
  ): Promise<string> {
    switch (name) {
      case 'list_issues':
        return JSON.stringify([
          { number: 42, title: '支持 MCP 协议接入', state: 'open' },
          { number: 41, title: '循环检测阈值可配置化', state: 'open' },
          { number: 39, title: 'Token 预算用完后的优雅降级', state: 'closed' },
        ], null, 2);
      case 'search_repositories':
        return JSON.stringify([
          { full_name: 'anthropics/anthropic-sdk-python', stars: 2800 },
          { full_name: 'vercel/ai', stars: 12000 },
          { full_name: 'modelcontextprotocol/servers', stars: 5600 },
        ], null, 2);
      case 'get_file_contents':
        return `# README\n\nMock file: ${args.owner}/${args.repo}/${args.path}`;
      default:
        return `未知工具: ${name}`;
    }
  }

  async close(): Promise<void> {}
}
```


```typescript
// ... ToolRegistry 已有代码不变 ...
// 新增以下内容：

private mcpClients: Array<MCPClient | MockMCPClient> = [];

async registerMCPServer(
  serverName: string,
  client: MCPClient | MockMCPClient,
): Promise<string[]> {
  await client.connect();
  this.mcpClients.push(client);

  const tools = await client.listTools();
  const registered: string[] = [];

  for (const tool of tools) {
    const prefixedName = `mcp__${serverName}__${tool.name}`;
    if (this.tools.has(prefixedName)) continue;

    const toolClient = client;
    const originalName = tool.name;

    this.register({
      name: prefixedName,
      description: `[MCP:${serverName}] ${tool.description}`,
      parameters: tool.inputSchema as Record<string, unknown>,
      isConcurrencySafe: true,
      isReadOnly: true,
      maxResultChars: 3000,
      execute: async (input: any) => {
        return toolClient.callTool(originalName, input);
      },
    });

    registered.push(prefixedName);
  }

  return registered;
}

async closeAllMCP(): Promise<void> {
  for (const client of this.mcpClients) {
    await client.close();
  }
  this.mcpClients = [];
}
```


```typescript
// ... import 和模型初始化同上一篇 ...
import { MCPClient, MockMCPClient } from './mcp-client.js';

const registry = new ToolRegistry();
registry.register(...allTools);

async function connectMCP() {
  const githubToken = process.env.GITHUB_PERSONAL_ACCESS_TOKEN;

  let canSpawn = true;
  try {
    const { execSync } = await import('node:child_process');
    execSync('echo test', { stdio: 'ignore' });
  } catch {
    canSpawn = false;
  }

  if (githubToken && canSpawn) {
    console.log('\n连接 GitHub MCP Server...');
    try {
      const client = new MCPClient(
        'npx', ['-y', '@modelcontextprotocol/server-github'],
        { GITHUB_PERSONAL_ACCESS_TOKEN: githubToken },
      );
      const tools = await registry.registerMCPServer('github', client);
      console.log(`  已注册 ${tools.length} 个 MCP 工具`);
      return;
    } catch (err) {
      console.log(`  MCP 连接失败: ${err instanceof Error ? err.message : err}`);
      console.log('  降级为 Mock MCP...');
    }
  }

  if (!githubToken) {
    console.log('\n未配置 GITHUB_PERSONAL_ACCESS_TOKEN，使用 Mock MCP');
  }

  const mockClient = new MockMCPClient();
  const tools = await registry.registerMCPServer('github', mockClient);
  console.log(`  已注册 ${tools.length} 个 Mock MCP 工具`);
}

// ... main() 调用 connectMCP 后启动 REPL ...
```


```bash
pnpm start
```


```
未配置 GITHUB_PERSONAL_ACCESS_TOKEN，使用 Mock MCP
  已注册 3 个 Mock MCP 工具

已注册 12 个工具：
  - get_weather（内置, 可并发）
  - calculator（内置, 可并发）
  - read_file（内置, 可并发）
  - write_file（内置, 串行）
  - list_directory（内置, 可并发）
  - edit_file（内置, 串行）
  - glob（内置, 可并发）
  - grep（内置, 可并发）
  - bash（内置, 串行）
  - mcp__github__list_issues（MCP, 可并发）
  - mcp__github__search_repositories（MCP, 可并发）
  - mcp__github__get_file_contents（MCP, 可并发）
```


```
You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
  { "number": 42, "title": "支持 MCP 协议接入", "state": "open" },
  ...
  → 继续下一步...

--- Step 2 ---
vercel/ai 仓库目前有以下 issues：
- #42 支持 MCP 协议接入（open）
- #41 循环检测阈值可配置化（open）
- #39 Token 预算用完后的优雅降级（closed）
```


```
连接 GitHub MCP Server...
  已注册 26 个 MCP 工具

已注册 35 个工具：
  - get_weather（内置, 可并发）
  - ...（省略其他 8 个内置工具）
  - mcp__github__list_issues（MCP, 可并发）
  - mcp__github__search_repositories（MCP, 可并发）
  - mcp__github__create_issue（MCP, 可并发）
  - mcp__github__list_pull_requests（MCP, 可并发）
  - mcp__github__search_code（MCP, 可并发）
  - ...（共 26 个 GitHub 工具）
```


```
You: 查看 vercel/ai 的 issues

--- Step 1 ---
  [调用: mcp__github__list_issues({"owner":"vercel","repo":"ai"})]
  [结果: mcp__github__list_issues] [
    { "number": 14625, "title": "feat(ai/ui): allow dynamic tools..." },
    { "number": 14476, "title": "docs: add ToolLoopAgent example..." },
    ...

--- Step 2 ---
vercel/ai 仓库的 issues：
- #14625（PR）：feat(ai/ui): allow dynamic tools to pass UI message validation
- #14476（PR）：docs: add ToolLoopAgent example
```


```typescript
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const transport = new StdioClientTransport({
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-github'],
  env: { GITHUB_PERSONAL_ACCESS_TOKEN: token },
});

const client = new Client({ name: 'super-agent', version: '1.0.0' });
await client.connect(transport);

const { tools } = await client.listTools();
const result = await client.callTool({
  name: 'list_issues',
  arguments: { owner: 'vercel', repo: 'ai' },
});
```
