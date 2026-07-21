# Plugin 架构——让别人给你的 Agent 写功能

课程
Super Agent 实战课
Plugin 架构——让别人给你的 Agent 写功能
Plugin 架构——让别人给你的 Agent 写功能

约 42 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

到目前为止我们的 Agent 所有能力都写死在代码里——工具、Skill、RAG，改一个就得重新部署。这在自己用的时候没问题，但你想想，如果你做了个 Agent 给团队用，产品经理说"接一下 Supabase 查数据"，运维说"加个飞书通知"，——每个需求都合理，但每加一个你就得改核心代码、跑测试、重新发版。

这个问题太经典了。VS Code 如果没有扩展系统就不会有 4 万个插件，Webpack 没有 Plugin 机制社区就没法给它加 HMR。解法也很经典——把核心和能力彻底分离，能力通过 Plugin 动态加载。核心只管推理循环和工具调度，具体功能全交给外部插件，互相隔离，加一个不影响另一个。

这一节我们来搭这套 Plugin 系统。说实话这套设计不只 Agent 能用——你以后做开放平台、内部工具框架、甚至 CLI 工具，需要扩展性的时候都是同一套思路。而且，搭完之后你会对"控制反转"这个抽象概念或者说设计模式有非常具体的体感。

先装依赖：

bash
运行
复制
pnpm install

设计 Plugin 接口

一个 Plugin 需要回答三个问题：你是谁（名称、版本、描述）、你要注册什么（工具、Channel、Cron Job）、你什么时候退出（destroy 清理资源）。

来，新建 src/plugins/types.ts：

src/plugins/types.ts
应用
复制
import type { ToolDefinition } from '../tools/registry.js';

export interface PluginConfig {
  [key: string]: string | number | boolean;
}

export interface PluginApi {
  registerTools(tools: ToolDefinition[]): void;
  getConfig(): PluginConfig;
  log(message: string): void;
}

export interface PluginDefinition {
  name: string;
  version: string;
  description: string;
  config?: PluginConfig;

  activate(api: PluginApi): Promise<void> | void;
  destroy?(): Promise<void> | void;
}


这里的关键设计是 PluginApi——Plugin 不直接操作 Agent 内部，而是通过一个受控的 API来交互。Plugin 能做什么、不能做什么，全由这一个 API 层决定。如果你直接把 ToolRegistry 传给 Plugin，它就能删别人注册的工具、修改核心配置、甚至搞崩整个系统。未来想给 Plugin 更多能力（比如注册 Channel、订阅事件），往 PluginApi 上加方法就行，不用改 Plugin 的接入方式。

PluginDefinition 的 activate 和 destroy 构成了一个最小生命周期：加载时初始化、卸载时清理。数据库连接池、WebSocket 长连接、定时器——这些资源都需要在 Plugin 卸载时释放干净，否则就是内存泄漏。

实现 PluginManager

PluginManager 的职责很明确：管理 Plugin 的加载/卸载、给每个 Plugin 构造隔离的 PluginApi、处理工具名冲突、保证一个 Plugin 挂掉了不影响其它 Plugin。

新建 src/plugins/manager.ts：

src/plugins/manager.ts
应用
复制
import type { ToolRegistry, ToolDefinition } from '../tools/registry.js';
import type { PluginDefinition, PluginConfig, PluginApi } from './types.js';

interface LoadedPlugin {
  definition: PluginDefinition;
  tools: string[];
}

export class PluginManager {
  private plugins = new Map<string, LoadedPlugin>();
  private registry: ToolRegistry;

  constructor(registry: ToolRegistry) {
    this.registry = registry;
  }

  async load(definition: PluginDefinition, config?: PluginConfig): Promise<string[]> {
    if (this.plugins.has(definition.name)) {
      throw new Error(`插件 "${definition.name}" 已加载`);
    }

    const resolvedConfig = this.resolveEnvVars({
      ...definition.config,
      ...config,
    });

    const registeredTools: string[] = [];

    const api: PluginApi = {
      registerTools: (tools: ToolDefinition[]) => {
        for (const tool of tools) {
          const prefixedName = `${definition.name}__${tool.name}`;
          const prefixedTool: ToolDefinition = {
            ...tool,
            name: prefixedName,
            description: `[Plugin:${definition.name}] ${tool.description}`,
          };
          this.registry.register(prefixedTool);
          registeredTools.push(prefixedName);
        }
      },
      getConfig: () => resolvedConfig,
      log: (message: string) => {
        console.log(`  [plugin:${definition.name}] ${message}`);
      },
    };

    try {
      await definition.activate(api);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error(`  [plugin:${definition.name}] 激活失败: ${msg}`);
      throw err;
    }

    this.plugins.set(definition.name, {
      definition,
      tools: registeredTools,
    });

    return registeredTools;
  }

  async unload(name: string): Promise<boolean> {
    const plugin = this.plugins.get(name);
    if (!plugin) return false;

    if (plugin.definition.destroy) {
      try {
        await plugin.definition.destroy();
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [plugin:${name}] destroy 出错: ${msg}`);
      }
    }

    for (const toolName of plugin.tools) {
      this.registry.unregister(toolName);
    }

    this.plugins.delete(name);
    return true;
  }

  async unloadAll(): Promise<void> {
    const names = Array.from(this.plugins.keys());
    for (const name of names) {
      await this.unload(name);
    }
  }

  get(name: string): LoadedPlugin | undefined {
    return this.plugins.get(name);
  }

  list(): Array<{ name: string; version: string; description: string; tools: string[] }> {
    return Array.from(this.plugins.values()).map(p => ({
      name: p.definition.name,
      version: p.definition.version,
      description: p.definition.description,
      tools: p.tools,
    }));
  }

  private resolveEnvVars(config: PluginConfig): PluginConfig {
    const resolved: PluginConfig = {};
    for (const [key, value] of Object.entries(config)) {
      if (typeof value === 'string' && value.startsWith('${') && value.endsWith('}')) {
        const envKey = value.slice(2, -1);
        resolved[key] = process.env[envKey] || '';
      } else {
        resolved[key] = value;
      }
    }
    return resolved;
  }
}


展开说说代码里的几个设计决策。

首先是 工具名前缀防冲突。Plugin 注册的工具名会被自动加上 pluginName__ 前缀。如果 supabase 插件注册了 query，实际注册到 Registry 里的是 supabase__query。这样即使两个 Plugin 都注册了叫 query 的工具也不会冲突。MCP 那节我们讲过同样的设计——MCP Server 的工具名也有 mcp__serverName__toolName 的三段式前缀。命名空间隔离几乎是所有插件系统的标配。

然后是环境变量解析：Plugin 的 config 里可以写 ${SUPABASE_URL} 这种占位符，resolveEnvVars 会自动替换成实际的环境变量值。这样 Plugin 的配置可以写死在代码里（声明"我需要这个变量"），实际值由部署环境提供。

还有错误隔离：activate 如果抛异常，不会影响其他已加载的 Plugin。destroy 里的异常也只是打 log，不会阻断其他 Plugin 的卸载流程。这个保证很重要——生产环境里一个 Plugin 挂了不能把整个 Agent 拉下水。

给 ToolRegistry 加上 unregister 方法，Plugin 卸载时要用：

src/tools/registry.ts
应用
复制
// 在 register 方法后面加上：
unregister(name: string): boolean {
  this.discoveredTools.delete(name);
  return this.tools.delete(name);
}

bash
运行
复制
pnpm start

写一个 Supabase 插件

理论讲完了，来写个真正的 Plugin 感受一下。我们做一个 Supabase 数据库插件，提供三个工具：list_tables（列出所有表）、query（查询数据）、insert（插入数据）。

新建 src/plugins/supabase-plugin.ts：

src/plugins/supabase-plugin.ts
应用
复制
import type { PluginDefinition, PluginApi } from './types.js';

export const supabasePlugin: PluginDefinition = {
  name: 'supabase',
  version: '1.0.0',
  description: '提供 Supabase 数据库操作能力（query / insert / list_tables）',
  config: {
    supabaseUrl: '${SUPABASE_URL}',
    supabaseKey: '${SUPABASE_KEY}',
  },

  activate(api: PluginApi) {
    const config = api.getConfig();
    const url = config.supabaseUrl as string;
    const key = config.supabaseKey as string;

    if (!url || !key) {
      api.log('未配置 SUPABASE_URL / SUPABASE_KEY，使用 Mock 模式');
    }

    api.registerTools([
      {
        name: 'list_tables',
        description: '列出数据库中所有表',
        parameters: { type: 'object', properties: {}, required: [] },
        isConcurrencySafe: true,
        isReadOnly: true,
        execute: async () => {
          if (!url) {
            return JSON.stringify({
              tables: ['users', 'posts', 'comments', 'sessions'],
              note: 'Mock 模式 — 配置 SUPABASE_URL 和 SUPABASE_KEY 连接真实数据库',
            });
          }
          return `连接 ${url} 查询表列表...`;
        },
      },
      {
        name: 'query',
        description: '查询指定表的数据，支持 select / where / limit',
        parameters: {
          type: 'object',
          properties: {
            table: { type: 'string', description: '表名' },
            select: { type: 'string', description: '查询字段，默认 *' },
            where: { type: 'string', description: '过滤条件，如 status=active' },
            limit: { type: 'number', description: '返回条数限制，默认 10' },
          },
          required: ['table'],
        },
        isConcurrencySafe: true,
        isReadOnly: true,
        execute: async (input: { table: string; select?: string; where?: string; limit?: number }) => {
          const { table, select = '*', where, limit = 10 } = input;
          if (!url) {
            const mockData: Record<string, any[]> = {
              users: [
                { id: 1, name: '张三', email: 'zhang@example.com', role: 'admin' },
                { id: 2, name: '李四', email: 'li@example.com', role: 'user' },
                { id: 3, name: '王五', email: 'wang@example.com', role: 'user' },
              ],
              posts: [
                { id: 1, title: 'Agent 开发入门', author_id: 1, status: 'published' },
                { id: 2, title: 'Plugin 架构设计', author_id: 1, status: 'draft' },
              ],
            };
            const rows = mockData[table] || [];
            let filtered = rows;
            if (where) {
              const [field, value] = where.split('=');
              filtered = rows.filter(r => String(r[field]) === value);
            }
            return JSON.stringify({ table, rows: filtered.slice(0, limit), total: filtered.length });
          }
          return `SELECT ${select} FROM ${table}${where ? ` WHERE ${where}` : ''} LIMIT ${limit}`;
        },
      },
      {
        name: 'insert',
        description: '向指定表插入一条记录',
        parameters: {
          type: 'object',
          properties: {
            table: { type: 'string', description: '表名' },
            data: { type: 'object', description: '要插入的数据' },
          },
          required: ['table', 'data'],
        },
        isConcurrencySafe: false,
        isReadOnly: false,
        execute: async (input: { table: string; data: Record<string, unknown> }) => {
          const { table, data } = input;
          if (!url) {
            return JSON.stringify({
              success: true,
              table,
              inserted: { id: Math.floor(Math.random() * 1000), ...data },
              note: 'Mock 模式',
            });
          }
          return `INSERT INTO ${table} — ${JSON.stringify(data)}`;
        },
      },
    ]);

    api.log(`已注册 3 个工具（list_tables / query / insert）`);
  },

  destroy() {
    console.log('  [plugin:supabase] 连接已释放');
  },
};


看一下这个插件里几个值得留意的设计。

config 里写的 '${SUPABASE_URL}' 是声明式的——告诉 PluginManager "我需要这个环境变量"，Manager 启动时自动解析，缺了给空字符串，Plugin 自己决定怎么降级。这里的降级策略是 Mock 模式——没配真实数据库也能跑，开发调试不受影响。

isConcurrencySafe 这个标记配合前面 ToolRegistry 的读写锁一起工作。list_tables 和 query 是纯读操作，标记 true 允许并发执行。insert 是写操作，标记 false 走独占锁。

destroy 在真实场景里会释放连接池（supabase.disconnect()），Mock 里打行日志就够了。

bash
运行
复制
pnpm start


试试跟 Agent 对话：

text
复制
You: 数据库里有哪些表

--- Step 1 ---
  [调用: supabase__list_tables({})]
  [结果: supabase__list_tables] {"tables":["users","posts","comments","sessions"],...}
  → 继续下一步...

--- Step 2 ---
数据库里有这些表：users, posts, comments, sessions

text
复制
You: 查用户数据

--- Step 1 ---
  [调用: supabase__query({"table":"users"})]
  [结果: supabase__query] {"table":"users","rows":[{"id":1,"name":"张三",...}],"total":3}
  → 继续下一步...

--- Step 2 ---
查询结果如下：users 表共 3 条记录...


Agent 直接通过 supabase__query 操作数据库了。从模型的视角看，这些就是普通的工具——它不需要知道这些工具是 Plugin 动态加载的还是内置的，调用方式完全一样。

管理插件的命令

给用户一个管理 Plugin 的入口——/plugin 命令。列出当前状态、手动加载/卸载：

src/commands/plugin.ts
应用
复制
import type { CommandHandler } from './index.js';
import type { PluginManager } from '../plugins/manager.js';
import type { PluginDefinition } from '../plugins/types.js';

export function createPluginCommands(
  pluginManager: PluginManager,
  availablePlugins: Map<string, PluginDefinition>,
): CommandHandler[] {
  return [
    // /plugin 或 /plugin list
    (cmd, _ctx) => {
      if (cmd !== '/plugin' && cmd !== '/plugin list') return false;

      const loaded = pluginManager.list();
      const unloaded = Array.from(availablePlugins.entries())
        .filter(([name]) => !loaded.find(p => p.name === name));

      if (loaded.length === 0 && unloaded.length === 0) {
        console.log('\n[plugins] 没有可用的插件。\n');
        return true;
      }

      console.log('\n[plugins]');
      if (loaded.length > 0) {
        console.log('  已加载：');
        for (const p of loaded) {
          console.log(`    ${p.name} v${p.version} — ${p.description}`);
          console.log(`      工具: ${p.tools.join(', ')}`);
        }
      }
      if (unloaded.length > 0) {
        console.log('  可加载：');
        for (const [name, def] of unloaded) {
          console.log(`    ${name} v${def.version} — ${def.description}`);
        }
      }
      console.log('');
      return true;
    },

    // /plugin load <name>
    (cmd, _ctx) => {
      const match = cmd.match(/^\/plugin\s+load\s+(\S+)$/);
      if (!match) return false;
      const name = match[1];

      const def = availablePlugins.get(name);
      if (!def) {
        console.log(`\n[plugins] 找不到插件: ${name}\n`);
        return true;
      }

      pluginManager.load(def).then(tools => {
        console.log(`\n[plugins] 已加载 ${name}，注册了 ${tools.length} 个工具：`);
        for (const t of tools) console.log(`    ${t}`);
        console.log('');
      });

      return true;
    },

    // /plugin unload <name>
    (cmd, _ctx) => {
      const match = cmd.match(/^\/plugin\s+unload\s+(\S+)$/);
      if (!match) return false;
      const name = match[1];

      pluginManager.unload(name).then(ok => {
        if (ok) {
          console.log(`\n[plugins] 已卸载 ${name}，相关工具已移除\n`);
        } else {
          console.log(`\n[plugins] ${name} 未加载\n`);
        }
      });

      return true;
    },
  ];
}

bash
运行
复制
pnpm start


试试 /plugin 看状态：

text
复制
You: /plugin

[plugins]
  已加载：
    supabase v1.0.0 — 提供 Supabase 数据库操作能力（query / insert / list_tables）
      工具: supabase__list_tables, supabase__query, supabase__insert


卸载再重新加载：

text
复制
You: /plugin unload supabase
[plugins] 已卸载 supabase，相关工具已移除

You: /plugin load supabase
[plugins] 已加载 supabase，注册了 3 个工具


卸载后 Agent 就用不了 supabase__query 了——工具从 Registry 里移除了。重新 load 又能用。这就是动态加载的意义：运行时增减能力，不需要重启。

把 Plugin 接入 Agent 主流程

最后一步，把 PluginManager 接入 index.ts。启动时自动加载已注册的插件，退出时调用 unloadAll 做 Graceful Shutdown：

src/index.ts
应用
复制
// 在 import 区域新增：
import { PluginManager } from './plugins/manager.js';
import { supabasePlugin } from './plugins/supabase-plugin.js';
import { createPluginCommands } from './commands/plugin.js';
import type { PluginDefinition } from './plugins/types.js';

// ── Plugins ────────────────────────────────
const pluginManager = new PluginManager(registry);
const availablePlugins = new Map<string, PluginDefinition>([
  ['supabase', supabasePlugin],
]);

// Commands 里注册 plugin 命令
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
  ...ragCommands,
  ...dreamCommands,
  ...createSkillCommands(skillLoader, activeSkills),
  ...createPluginCommands(pluginManager, availablePlugins),
]);

// main() 里启动时加载插件
console.log('  加载插件...');
for (const [name, def] of availablePlugins) {
  try {
    const tools = await pluginManager.load(def);
    console.log(`  ✓ ${name} — ${tools.length} 个工具`);
  } catch {
    console.log(`  ✗ ${name} — 加载失败`);
  }
}

// 退出时 Graceful Shutdown
if (!trimmed || trimmed === 'exit') {
  console.log('Bye!');
  await pluginManager.unloadAll();  // ← 清理所有插件资源
  rl.close();
  return;
}


启动后你会看到：

text
复制
  加载插件...
  [plugin:supabase] 未配置 SUPABASE_URL / SUPABASE_KEY，使用 Mock 模式
  [plugin:supabase] 已注册 3 个工具（list_tables / query / insert）
  ✓ supabase — 3 个工具
Super Agent v0.15 — Plugins (type "exit" to quit)


退出时：

text
复制
You: exit
Bye!
  [plugin:supabase] 连接已释放

这套架构的可迁移性

小结一下，我们这一节做了五件事，这五件事构成了一个完整的插件系统的骨架，放到任何项目里都能用：

1. 接口契约（PluginDefinition）——定义清楚"一个插件长什么样"。这是所有插件系统的起点。不管你做的是 Agent、编辑器、还是构建工具，第一步都是定义这个接口。

2. API 隔离层（PluginApi）——插件不直接操作内部，只通过一个受控的中间层交互。比如 VS Code 的 vscode API、Webpack 的 compiler 对象、Express 的 app 对象，都是这个思路，相当于是业界的最佳实践了，好处是你随时能改内部实现，但暴露给插件的 API 层保持稳定。

3. 命名空间隔离（pluginName__toolName）——防止不同插件之间的名字冲突。npm 用 scope（@org/pkg），Chrome 扩展用 manifest ID，道理一样。

4. 生命周期管理（activate / destroy）——解决资源泄漏问题。任何需要初始化和清理的资源（连接池、文件句柄、定时器），都必须有显式的生命周期。

5. 错误隔离——一个插件挂了不影响其他插件，保证基本的稳定性。

如果你以后要给自己的项目加扩展性——不管是给内部工具加插件、给产品做开放能力、还是给框架设计扩展点——这五个设计决策都是绕不过去的。

写在最后

回顾一下，Skill 是往 prompt 里注入知识（改变 Agent 怎么想），Plugin 是往运行时注入代码（改变 Agent 能做什么）。两者配合起来，Agent 的能力就不再受限于开发者一个人的想象力了。

现在我们的 Plugin 只支持注册 tools。后面 Channel 那一节会演示怎么通过 Plugin 注册通信通道（飞书 Bot、Telegram Bot），再后面 Cron 那一节会演示怎么通过 Plugin 注册定时任务。同一个 PluginDefinition 接口，未来还能扩展出 registerChannels、registerCrons——Plugin 是能力的分发单元，工具只是它能分发的第一种东西。

下一节我们来做 Channel 抽象——让 Agent 不只活在终端里，还能在飞书群里跟人对话。到时候你会看到，Channel 就是作为 Plugin 的一部分被加载进来的。

参考资料
VS Code Extension API — 最成熟的编辑器插件系统设计
Webpack Plugin 文档 — tapable 事件系统 + 生命周期钩子
上一篇
Skills——给 Agent 注入领域知识
下一篇 · 第五章：Skills + Plugins + Channel
Channel 抽象——让 Agent 活在飞书群里
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
import type { ToolDefinition } from '../tools/registry.js';

export interface PluginConfig {
  [key: string]: string | number | boolean;
}

export interface PluginApi {
  registerTools(tools: ToolDefinition[]): void;
  getConfig(): PluginConfig;
  log(message: string): void;
}

export interface PluginDefinition {
  name: string;
  version: string;
  description: string;
  config?: PluginConfig;

  activate(api: PluginApi): Promise<void> | void;
  destroy?(): Promise<void> | void;
}
```


```ts
import type { ToolRegistry, ToolDefinition } from '../tools/registry.js';
import type { PluginDefinition, PluginConfig, PluginApi } from './types.js';

interface LoadedPlugin {
  definition: PluginDefinition;
  tools: string[];
}

export class PluginManager {
  private plugins = new Map<string, LoadedPlugin>();
  private registry: ToolRegistry;

  constructor(registry: ToolRegistry) {
    this.registry = registry;
  }

  async load(definition: PluginDefinition, config?: PluginConfig): Promise<string[]> {
    if (this.plugins.has(definition.name)) {
      throw new Error(`插件 "${definition.name}" 已加载`);
    }

    const resolvedConfig = this.resolveEnvVars({
      ...definition.config,
      ...config,
    });

    const registeredTools: string[] = [];

    const api: PluginApi = {
      registerTools: (tools: ToolDefinition[]) => {
        for (const tool of tools) {
          const prefixedName = `${definition.name}__${tool.name}`;
          const prefixedTool: ToolDefinition = {
            ...tool,
            name: prefixedName,
            description: `[Plugin:${definition.name}] ${tool.description}`,
          };
          this.registry.register(prefixedTool);
          registeredTools.push(prefixedName);
        }
      },
      getConfig: () => resolvedConfig,
      log: (message: string) => {
        console.log(`  [plugin:${definition.name}] ${message}`);
      },
    };

    try {
      await definition.activate(api);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error(`  [plugin:${definition.name}] 激活失败: ${msg}`);
      throw err;
    }

    this.plugins.set(definition.name, {
      definition,
      tools: registeredTools,
    });

    return registeredTools;
  }

  async unload(name: string): Promise<boolean> {
    const plugin = this.plugins.get(name);
    if (!plugin) return false;

    if (plugin.definition.destroy) {
      try {
        await plugin.definition.destroy();
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [plugin:${name}] destroy 出错: ${msg}`);
      }
    }

    for (const toolName of plugin.tools) {
      this.registry.unregister(toolName);
    }

    this.plugins.delete(name);
    return true;
  }

  async unloadAll(): Promise<void> {
    const names = Array.from(this.plugins.keys());
    for (const name of names) {
      await this.unload(name);
    }
  }

  get(name: string): LoadedPlugin | undefined {
    return this.plugins.get(name);
  }

  list(): Array<{ name: string; version: string; description: string; tools: string[] }> {
    return Array.from(this.plugins.values()).map(p => ({
      name: p.definition.name,
      version: p.definition.version,
      description: p.definition.description,
      tools: p.tools,
    }));
  }

  private resolveEnvVars(config: PluginConfig): PluginConfig {
    const resolved: PluginConfig = {};
    for (const [key, value] of Object.entries(config)) {
      if (typeof value === 'string' && value.startsWith('${') && value.endsWith('}')) {
        const envKey = value.slice(2, -1);
        resolved[key] = process.env[envKey] || '';
      } else {
        resolved[key] = value;
      }
    }
    return resolved;
  }
}
```


```ts
// 在 register 方法后面加上：
unregister(name: string): boolean {
  this.discoveredTools.delete(name);
  return this.tools.delete(name);
}
```


```bash
pnpm start
```


```ts
import type { PluginDefinition, PluginApi } from './types.js';

export const supabasePlugin: PluginDefinition = {
  name: 'supabase',
  version: '1.0.0',
  description: '提供 Supabase 数据库操作能力（query / insert / list_tables）',
  config: {
    supabaseUrl: '${SUPABASE_URL}',
    supabaseKey: '${SUPABASE_KEY}',
  },

  activate(api: PluginApi) {
    const config = api.getConfig();
    const url = config.supabaseUrl as string;
    const key = config.supabaseKey as string;

    if (!url || !key) {
      api.log('未配置 SUPABASE_URL / SUPABASE_KEY，使用 Mock 模式');
    }

    api.registerTools([
      {
        name: 'list_tables',
        description: '列出数据库中所有表',
        parameters: { type: 'object', properties: {}, required: [] },
        isConcurrencySafe: true,
        isReadOnly: true,
        execute: async () => {
          if (!url) {
            return JSON.stringify({
              tables: ['users', 'posts', 'comments', 'sessions'],
              note: 'Mock 模式 — 配置 SUPABASE_URL 和 SUPABASE_KEY 连接真实数据库',
            });
          }
          return `连接 ${url} 查询表列表...`;
        },
      },
      {
        name: 'query',
        description: '查询指定表的数据，支持 select / where / limit',
        parameters: {
          type: 'object',
          properties: {
            table: { type: 'string', description: '表名' },
            select: { type: 'string', description: '查询字段，默认 *' },
            where: { type: 'string', description: '过滤条件，如 status=active' },
            limit: { type: 'number', description: '返回条数限制，默认 10' },
          },
          required: ['table'],
        },
        isConcurrencySafe: true,
        isReadOnly: true,
        execute: async (input: { table: string; select?: string; where?: string; limit?: number }) => {
          const { table, select = '*', where, limit = 10 } = input;
          if (!url) {
            const mockData: Record<string, any[]> = {
              users: [
                { id: 1, name: '张三', email: 'zhang@example.com', role: 'admin' },
                { id: 2, name: '李四', email: 'li@example.com', role: 'user' },
                { id: 3, name: '王五', email: 'wang@example.com', role: 'user' },
              ],
              posts: [
                { id: 1, title: 'Agent 开发入门', author_id: 1, status: 'published' },
                { id: 2, title: 'Plugin 架构设计', author_id: 1, status: 'draft' },
              ],
            };
            const rows = mockData[table] || [];
            let filtered = rows;
            if (where) {
              const [field, value] = where.split('=');
              filtered = rows.filter(r => String(r[field]) === value);
            }
            return JSON.stringify({ table, rows: filtered.slice(0, limit), total: filtered.length });
          }
          return `SELECT ${select} FROM ${table}${where ? ` WHERE ${where}` : ''} LIMIT ${limit}`;
        },
      },
      {
        name: 'insert',
        description: '向指定表插入一条记录',
        parameters: {
          type: 'object',
          properties: {
            table: { type: 'string', description: '表名' },
            data: { type: 'object', description: '要插入的数据' },
          },
          required: ['table', 'data'],
        },
        isConcurrencySafe: false,
        isReadOnly: false,
        execute: async (input: { table: string; data: Record<string, unknown> }) => {
          const { table, data } = input;
          if (!url) {
            return JSON.stringify({
              success: true,
              table,
              inserted: { id: Math.floor(Math.random() * 1000), ...data },
              note: 'Mock 模式',
            });
          }
          return `INSERT INTO ${table} — ${JSON.stringify(data)}`;
        },
      },
    ]);

    api.log(`已注册 3 个工具（list_tables / query / insert）`);
  },

  destroy() {
    console.log('  [plugin:supabase] 连接已释放');
  },
};
```


```bash
pnpm start
```


```text
You: 数据库里有哪些表

--- Step 1 ---
  [调用: supabase__list_tables({})]
  [结果: supabase__list_tables] {"tables":["users","posts","comments","sessions"],...}
  → 继续下一步...

--- Step 2 ---
数据库里有这些表：users, posts, comments, sessions
```


```text
You: 查用户数据

--- Step 1 ---
  [调用: supabase__query({"table":"users"})]
  [结果: supabase__query] {"table":"users","rows":[{"id":1,"name":"张三",...}],"total":3}
  → 继续下一步...

--- Step 2 ---
查询结果如下：users 表共 3 条记录...
```


```ts
import type { CommandHandler } from './index.js';
import type { PluginManager } from '../plugins/manager.js';
import type { PluginDefinition } from '../plugins/types.js';

export function createPluginCommands(
  pluginManager: PluginManager,
  availablePlugins: Map<string, PluginDefinition>,
): CommandHandler[] {
  return [
    // /plugin 或 /plugin list
    (cmd, _ctx) => {
      if (cmd !== '/plugin' && cmd !== '/plugin list') return false;

      const loaded = pluginManager.list();
      const unloaded = Array.from(availablePlugins.entries())
        .filter(([name]) => !loaded.find(p => p.name === name));

      if (loaded.length === 0 && unloaded.length === 0) {
        console.log('\n[plugins] 没有可用的插件。\n');
        return true;
      }

      console.log('\n[plugins]');
      if (loaded.length > 0) {
        console.log('  已加载：');
        for (const p of loaded) {
          console.log(`    ${p.name} v${p.version} — ${p.description}`);
          console.log(`      工具: ${p.tools.join(', ')}`);
        }
      }
      if (unloaded.length > 0) {
        console.log('  可加载：');
        for (const [name, def] of unloaded) {
          console.log(`    ${name} v${def.version} — ${def.description}`);
        }
      }
      console.log('');
      return true;
    },

    // /plugin load <name>
    (cmd, _ctx) => {
      const match = cmd.match(/^\/plugin\s+load\s+(\S+)$/);
      if (!match) return false;
      const name = match[1];

      const def = availablePlugins.get(name);
      if (!def) {
        console.log(`\n[plugins] 找不到插件: ${name}\n`);
        return true;
      }

      pluginManager.load(def).then(tools => {
        console.log(`\n[plugins] 已加载 ${name}，注册了 ${tools.length} 个工具：`);
        for (const t of tools) console.log(`    ${t}`);
        console.log('');
      });

      return true;
    },

    // /plugin unload <name>
    (cmd, _ctx) => {
      const match = cmd.match(/^\/plugin\s+unload\s+(\S+)$/);
      if (!match) return false;
      const name = match[1];

      pluginManager.unload(name).then(ok => {
        if (ok) {
          console.log(`\n[plugins] 已卸载 ${name}，相关工具已移除\n`);
        } else {
          console.log(`\n[plugins] ${name} 未加载\n`);
        }
      });

      return true;
    },
  ];
}
```


```bash
pnpm start
```


```text
You: /plugin

[plugins]
  已加载：
    supabase v1.0.0 — 提供 Supabase 数据库操作能力（query / insert / list_tables）
      工具: supabase__list_tables, supabase__query, supabase__insert
```


```text
You: /plugin unload supabase
[plugins] 已卸载 supabase，相关工具已移除

You: /plugin load supabase
[plugins] 已加载 supabase，注册了 3 个工具
```


```ts
// 在 import 区域新增：
import { PluginManager } from './plugins/manager.js';
import { supabasePlugin } from './plugins/supabase-plugin.js';
import { createPluginCommands } from './commands/plugin.js';
import type { PluginDefinition } from './plugins/types.js';

// ── Plugins ────────────────────────────────
const pluginManager = new PluginManager(registry);
const availablePlugins = new Map<string, PluginDefinition>([
  ['supabase', supabasePlugin],
]);

// Commands 里注册 plugin 命令
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
  ...ragCommands,
  ...dreamCommands,
  ...createSkillCommands(skillLoader, activeSkills),
  ...createPluginCommands(pluginManager, availablePlugins),
]);

// main() 里启动时加载插件
console.log('  加载插件...');
for (const [name, def] of availablePlugins) {
  try {
    const tools = await pluginManager.load(def);
    console.log(`  ✓ ${name} — ${tools.length} 个工具`);
  } catch {
    console.log(`  ✗ ${name} — 加载失败`);
  }
}

// 退出时 Graceful Shutdown
if (!trimmed || trimmed === 'exit') {
  console.log('Bye!');
  await pluginManager.unloadAll();  // ← 清理所有插件资源
  rl.close();
  return;
}
```


```text
加载插件...
  [plugin:supabase] 未配置 SUPABASE_URL / SUPABASE_KEY，使用 Mock 模式
  [plugin:supabase] 已注册 3 个工具（list_tables / query / insert）
  ✓ supabase — 3 个工具
Super Agent v0.15 — Plugins (type "exit" to quit)
```


```text
You: exit
Bye!
  [plugin:supabase] 连接已释放
```
