# 收官——配置系统、CLI 入口与部署上线

课程
Super Agent 实战课
收官——配置系统、CLI 入口与部署上线
收官——配置系统、CLI 入口与部署上线

约 40 分钟

好了，之前我们已经做了足够多的 Agent 开发工作，这一节我们完成收尾工作，让我们的这个项目从一个本地项目正式变为能够跑在服务端的产品。

这一节我们做三件事：

把散落在代码各处的硬编码收进一个 super-agent.config.json；
加一个交互式的 init 引导命令；
最后部署到远端服务器跑起来。

做完之后，你的 Agent 就是一个像 OpenClaw 一样的正经 CLI 工具了——pnpm run init 生成配置，pnpm start 启动服务。

先装依赖：

bash
运行
复制
pnpm install

从硬编码到配置文件

翻一下现在的 index.ts，硬编码的地方偏多，模型地址、模型名、子 Agent 参数、飞书端口、插件列表、Cron 数据目录……每改一个都要动代码、重新部署。

我们要做的是定义一个 JSON 配置文件，把所有可调参数集中管理。读取的时候做三件事：

Zod 校验：确保配置合法，写错了直接报错并告诉你哪个字段有问题
环境变量替换：配置里写 ${DASHSCOPE_API_KEY}，运行时自动从 .env 或系统环境变量读取，敏感信息不硬编码
默认值合并：配置文件只需要写"跟默认不一样的部分"，其他全部自动填充
配置 Schema

新建 src/config/schema.ts，用 Zod 定义配置结构：

src/config/schema.ts
应用
复制
import { z } from 'zod';

export const ModelConfigSchema = z.object({
  provider: z.enum(['dashscope', 'openai', 'custom']).default('dashscope'),
  name: z.string().default('qwen-plus-latest'),
  baseURL: z.string().default('https://dashscope.aliyuncs.com/compatible-mode/v1'),
  apiKey: z.string().default(''),
});

export const PluginConfigSchema = z.object({
  name: z.string(),
  enabled: z.boolean().default(true),
  config: z.record(z.string()).default({}),
});

export const FeishuChannelConfigSchema = z.object({
  enabled: z.boolean().default(false),
  appId: z.string().default(''),
  appSecret: z.string().default(''),
  port: z.number().default(3000),
});

export const ChannelConfigSchema = z.object({
  feishu: FeishuChannelConfigSchema.default({}),
});

export const AgentConfigSchema = z.object({
  maxSpawnDepth: z.number().min(0).max(5).default(1),
  maxConcurrent: z.number().min(1).max(10).default(3),
  defaultTimeout: z.number().default(60000),
});

export const SecurityConfigSchema = z.object({
  defaultRole: z.string().default('developer'),
  auditLog: z.boolean().default(true),
  bashTimestamp: z.boolean().default(true),
});

export const MemoryConfigSchema = z.object({
  dataDir: z.string().default('.'),
});

export const RagConfigSchema = z.object({
  enabled: z.boolean().default(true),
  docsDir: z.string().default('docs'),
});

export const CronConfigSchema = z.object({
  enabled: z.boolean().default(true),
  dataDir: z.string().default('.'),
});

export const SessionConfigSchema = z.object({
  id: z.string().default('default'),
});

export const UsageConfigSchema = z.object({
  trackingFile: z.string().default('.usage/today.jsonl'),
});

export const SuperAgentConfigSchema = z.object({
  version: z.string().default('1.0'),
  model: ModelConfigSchema.default({}),
  plugins: z.array(PluginConfigSchema).default([]),
  channels: ChannelConfigSchema.default({}),
  agents: AgentConfigSchema.default({}),
  security: SecurityConfigSchema.default({}),
  memory: MemoryConfigSchema.default({}),
  rag: RagConfigSchema.default({}),
  cron: CronConfigSchema.default({}),
  session: SessionConfigSchema.default({}),
  usage: UsageConfigSchema.default({}),
});

export type SuperAgentConfig = z.infer<typeof SuperAgentConfigSchema>;


注意每个字段都有 .default() —— 这意味着用户可以只写一个空的 {}，Zod 会把所有默认值都填上。想改什么就写什么，其余的自动用默认值。这就是配置最小化原则：配置文件只表达"跟默认不一样的意图"。

OpenClaw 的配置系统也是这个思路，它用了大约 200 个 Zod schema 覆盖所有子系统。我们的配置项规模小得多，但核心模式是一样的。

ConfigLoader：读取、替换、校验

新建 src/config/loader.ts：

src/config/loader.ts
应用
复制
import fs from 'node:fs';
import { SuperAgentConfigSchema, type SuperAgentConfig } from './schema.js';

export const CONFIG_FILE = 'super-agent.config.json';

const ENV_VAR_RE = /\$\{([A-Z_][A-Z0-9_]*)\}/g;

function substituteEnvVars(obj: unknown): unknown {
  if (typeof obj === 'string') {
    return obj.replace(ENV_VAR_RE, (match, name) => {
      const val = process.env[name];
      if (val === undefined) {
        console.warn(`  ⚠ 环境变量 ${name} 未设置，保留原值`);
        return match;
      }
      return val;
    });
  }
  if (Array.isArray(obj)) return obj.map(substituteEnvVars);
  if (obj !== null && typeof obj === 'object') {
    const result: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(obj as Record<string, unknown>)) {
      result[key] = substituteEnvVars(value);
    }
    return result;
  }
  return obj;
}

export function loadConfig(path = CONFIG_FILE): SuperAgentConfig {
  if (!fs.existsSync(path)) {
    console.log(`  未找到 ${path}，使用默认配置`);
    console.log('  运行 pnpm run init 生成配置文件\n');
    return SuperAgentConfigSchema.parse({});
  }

  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(path, 'utf-8'));
  } catch (err) {
    console.error(`  ✗ 解析 ${path} 失败: ${(err as Error).message}`);
    process.exit(1);
  }

  const substituted = substituteEnvVars(raw);

  const result = SuperAgentConfigSchema.safeParse(substituted);
  if (!result.success) {
    console.error('  ✗ 配置文件校验失败:');
    for (const issue of result.error.issues) {
      console.error(`    ${issue.path.join('.')}: ${issue.message}`);
    }
    process.exit(1);
  }

  console.log(`  ✓ 已加载 ${path}`);
  return result.data;
}


loadConfig 做了下面这四件事情，顺序很重要：

读取 JSON → 替换 ${ENV_VAR} → Zod 校验 + 默认值合并

环境变量替换发生在 Zod 校验之前。这意味着你可以在配置文件里写 "apiKey": "${DASHSCOPE_API_KEY}"，运行时自动从环境变量读取实际值。敏感信息（API Key、Secret）不会出现在配置文件里，但配置文件本身依然是完整可读的。

substituteEnvVars 只匹配大写字母+下划线的模式（[A-Z_][A-Z0-9_]*），这是 OpenClaw 也在用的约定——环境变量统一大写，避免误替换正常文本中的 ${} 模式。

如果配置校验失败，safeParse 返回详细的错误路径和原因，直接打印出来告诉用户哪里写错了。

init 引导命令

用户第一次用 Super Agent，不应该让他手动写 JSON。一个交互式的 init 命令能引导用户走完所有关键配置。

新建 src/config/init.ts：

src/config/init.ts
应用
复制
import { createInterface } from 'node:readline';
import fs from 'node:fs';
import { CONFIG_FILE } from './loader.js';

export async function runInit() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string): Promise<string> =>
    new Promise((resolve) => {
      console.log(q);
      rl.question('  > ', resolve);
    });

  console.log('\n  Super Agent 初始化向导\n');

  if (fs.existsSync(CONFIG_FILE)) {
    const overwrite = await ask(`  ${CONFIG_FILE} 已存在，覆盖? (y/N): `);
    if (overwrite.toLowerCase() !== 'y') {
      console.log('  已取消\n');
      rl.close();
      return;
    }
  }

  // ── 模型选择 ──────────────────────────
  console.log('  选择模型:\n');
  console.log('    1. qwen-plus-latest   (推荐，均衡)');
  console.log('    2. qwen-turbo-latest  (快速，便宜)');
  console.log('    3. qwen-max-latest    (最强，贵)\n');
  const modelChoice = (await ask('  模型 [1]: ')) || '1';
  const models: Record<string, string> = {
    '1': 'qwen-plus-latest',
    '2': 'qwen-turbo-latest',
    '3': 'qwen-max-latest',
  };
  const modelName = models[modelChoice] || 'qwen-plus-latest';

  // ── API Key ──────────────────────────
  const apiKey = await ask('\n  DashScope API Key (留空则从环境变量 DASHSCOPE_API_KEY 读取): ');

  // ── 飞书 Channel ──────────────────────────
  const enableFeishu = (await ask('\n  启用飞书 Channel? (y/N): ')).toLowerCase() === 'y';
  let feishuAppId = '';
  let feishuAppSecret = '';
  if (enableFeishu) {
    feishuAppId = await ask('  飞书 App ID: ');
    feishuAppSecret = await ask('  飞书 App Secret: ');
  }

  // ── Sub-Agent ──────────────────────────
  const concurrentStr = await ask('\n  子 Agent 最大并发数 [3]: ');
  const maxConcurrent = parseInt(concurrentStr) || 3;

  // ── 生成配置 ──────────────────────────
  const config = {
    version: '1.0',
    model: {
      provider: 'dashscope',
      name: modelName,
      baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
      apiKey: apiKey || '${DASHSCOPE_API_KEY}',
    },
    plugins: [
      { name: 'supabase', enabled: false, config: {} },
    ],
    channels: {
      feishu: {
        enabled: enableFeishu,
        appId: enableFeishu ? feishuAppId : '${FEISHU_APP_ID}',
        appSecret: enableFeishu ? feishuAppSecret : '${FEISHU_APP_SECRET}',
        port: 3000,
      },
    },
    agents: {
      maxSpawnDepth: 1,
      maxConcurrent,
      defaultTimeout: 60000,
    },
    security: {
      defaultRole: 'developer',
      auditLog: true,
      bashTimestamp: true,
    },
    memory: { dataDir: '.' },
    rag: { enabled: true, docsDir: 'docs' },
    cron: { enabled: true, dataDir: '.' },
    session: { id: 'default' },
    usage: { trackingFile: '.usage/today.jsonl' },
  };

  fs.writeFileSync(CONFIG_FILE, JSON.stringify(config, null, 2) + '\n');
  console.log(`\n  ✓ ${CONFIG_FILE} 已生成`);

  // 生成 .env
  const envLines: string[] = [];
  if (apiKey) {
    envLines.push(`DASHSCOPE_API_KEY=${apiKey}`);
  }
  if (enableFeishu && feishuAppId) {
    envLines.push(`FEISHU_APP_ID=${feishuAppId}`);
    envLines.push(`FEISHU_APP_SECRET=${feishuAppSecret}`);
  }
  if (envLines.length > 0) {
    fs.writeFileSync('.env', envLines.join('\n') + '\n');
    console.log('  ✓ .env 已生成');
  }

  console.log('\n  启动 Agent: pnpm start\n');
  rl.close();
}


init 生成两个文件：super-agent.config.json 和 .env。API Key 如果用户直接输入了就写进 .env，配置文件里用 ${DASHSCOPE_API_KEY} 引用。飞书的 App ID 和 App Secret 同理。

CLI 入口

现在 index.ts 需要根据命令行参数决定跑 init 还是 start。

把原来 index.ts 里的所有启动逻辑搬到 src/main.ts，index.ts 只做路由：

src/index.ts
应用
复制
const command = process.argv[2];

if (command === 'init') {
  import('./config/init.js').then(m => m.runInit());
} else {
  import('./main.js').then(m => m.startAgent().catch(console.error));
}


然后 package.json 加上对应的 scripts：

package.json
应用
复制
{
  "name": "super-agent",
  "version": "1.0.0",
  "type": "module",
  "bin": {
    "super-agent": "src/index.ts"
  },
  "scripts": {
    "start": "tsx src/index.ts",
    "init": "tsx src/index.ts init",
    "continue": "tsx src/index.ts --continue"
  }
}


dependencies 和 devDependencies 跟上一节一样，没有新增依赖。bin 字段让这个包可以作为 CLI 工具安装——npm install -g 之后就能直接用 super-agent start 和 super-agent init。跟 OpenClaw 的 openclaw 命令一样的套路。

bash
运行
复制
pnpm start

配置驱动初始化

src/main.ts 是重构后的主入口。最大的变化是：所有硬编码都被 config 对象替代。

看几个关键的对比：

之前模型创建是这样的：

ts
复制
const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});
const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();


现在变成了：

ts
复制
function createModel(cfg: SuperAgentConfig['model']) {
  if (!cfg.apiKey) return createMockModel();
  const provider = createOpenAI({ baseURL: cfg.baseURL, apiKey: cfg.apiKey });
  return provider.chat(cfg.name);
}
const model = createModel(config.model);


模型名、API 地址、API Key 全从配置来读取。如果想换成 qwen-max-latest，改配置文件就行，不动代码。

子 Agent 配置也一样：

ts
复制
// 之前
const agentRegistry = new SubAgentRegistry({ maxSpawnDepth: 1, maxConcurrent: 3 });

// 现在
const agentRegistry = new SubAgentRegistry({
  maxSpawnDepth: config.agents.maxSpawnDepth,
  maxConcurrent: config.agents.maxConcurrent,
});


飞书 Channel 变成了条件启用：

ts
复制
if (config.channels.feishu.enabled) {
  const feishuChannel = new FeishuChannel({
    appId: config.channels.feishu.appId,
    appSecret: config.channels.feishu.appSecret,
    port: config.channels.feishu.port,
  });
  gateway.register(feishuChannel);
}


配置文件里 "feishu": { "enabled": false } 就完全跳过飞书初始化——不创建实例、不启动 HTTP 服务。之前不管用不用飞书，都会创建 FeishuChannel 实例并启动一个 HTTP 服务在那占着端口。

之前做过的安全相关的 Hook 也是配置驱动：

ts
复制
if (config.security.auditLog) {
  hookPipeline.registerPre('audit-log', ...);
}
if (config.security.bashTimestamp) {
  hookPipeline.registerPost('bash-timestamp', ...);
}


插件加载也从"全部加载"变成了"按配置启用"：

ts
复制
for (const pluginCfg of config.plugins) {
  const def = availablePlugins.get(pluginCfg.name);
  if (!def) { console.log(`  ✗ ${pluginCfg.name} — 未知插件`); continue; }
  if (!pluginCfg.enabled) { console.log(`  - ${pluginCfg.name} — 已禁用`); continue; }
  const tools = await pluginManager.load(def);
  console.log(`  ✓ ${pluginCfg.name} — ${tools.length} 个工具`);
}


Cron 和 RAG 同理，都有 config.cron.enabled 和 config.rag.enabled 的开关。配置里关掉了，对应的子系统完全不初始化。

main.ts 的完整代码可以在右侧编辑器里查看——本质上就是把上一节的 index.ts 搬过来，所有硬编码替换成 config.xxx 读取，再给各个子系统加上条件开关。

整个 main.ts 的初始化流程就是按配置逐个初始化子系统。回头看一下我们从头到尾造了多少东西：

每一块的参数都从 super-agent.config.json 读取，配置化的真正价值在于让运维和使用者不需要理解代码就能调整行为。

bash
运行
复制
pnpm start


试试修改 super-agent.config.json 里的参数然后重启，观察启动信息的变化。比如把 agents.maxConcurrent 改成 5，或者把 security.bashTimestamp 改成 false。

配置文件长什么样

跑完 init 之后生成的 super-agent.config.json 大概是这样的：

json
复制
{
  "version": "1.0",
  "model": {
    "provider": "dashscope",
    "name": "qwen-plus-latest",
    "baseURL": "https://dashscope.aliyuncs.com/compatible-mode/v1",
    "apiKey": "${DASHSCOPE_API_KEY}"
  },
  "plugins": [
    { "name": "supabase", "enabled": false, "config": {} }
  ],
  "channels": {
    "feishu": {
      "enabled": true,
      "appId": "${FEISHU_APP_ID}",
      "appSecret": "${FEISHU_APP_SECRET}",
      "port": 3000
    }
  },
  "agents": {
    "maxSpawnDepth": 1,
    "maxConcurrent": 3,
    "defaultTimeout": 60000
  },
  "security": {
    "defaultRole": "developer",
    "auditLog": true,
    "bashTimestamp": true
  },
  "cron": { "enabled": true, "dataDir": "." },
  "rag": { "enabled": true, "docsDir": "docs" }
}


所有的 ${DASHSCOPE_API_KEY} 之类的引用，在 loadConfig 的时候会自动从 .env 或系统环境变量里替换成真实值。配置文件本身可以安全地放进 git 仓库。

如果你什么都不配，所有字段都会用默认值——Zod schema 里的 .default() 保证了这一点。只有你想"覆盖默认行为"的时候才需要写对应字段。比如你只想把并发数从 3 改成 5，配置文件可以只有一行：

json
复制
{ "agents": { "maxConcurrent": 5 } }


其他所有字段自动走默认值。

部署到远端

如果你仅仅想要部署在本地，那么现在这一部分你可以跳过了，但如果你希望能有台服务器，24 小时不间断运行，接下来的内容你可以跟着一起做。

第一步：准备服务器

在云厂商（比如阿里云、腾讯云）购买一台 Linux VPS（Ubuntu/Debian），装好 Node.js 20+：

国内服务器网络问题会比较坑，按照我下面的命令走完即可

bash
复制
sudo dnf install -y git
git clone https://gitee.com/mirrors/nvm.git ~/.nvm && cd ~/.nvm && git checkout `git describe --abbrev=0 --tags`
sudo sh -c 'echo ". ~/.nvm/nvm.sh" >> /etc/profile && echo "export NVM_NODEJS_ORG_MIRROR=https://npmmirror.com/mirrors/node" >> /etc/profile'
# 解压到 /usr/local                                                             
source /etc/profile
nvm install 20
npm install -g pnpm pm2


第二步：上传代码和配置

先确保项目根目录有 .gitignore，避免把敏感信息和运行时数据提交到仓库：

node_modules/
.env
.sessions/
.memory/
.usage/
.cron/
super-agent.config.json


.env 里有 API Key，super-agent.config.json 可能包含替换后的真实密钥——这两个文件只应该存在于运行环境，不进 git。

把项目代码传到服务器：

bash
复制
# 在服务器上
git clone <你的仓库地址> super-agent
cd super-agent
pnpm install


然后运行 init 生成配置：

bash
复制
pnpm run init


按提示填入 DashScope API Key、选择模型。如果要接飞书 bot，选 y 并填入 App ID 和 App Secret。

这一步生成了 super-agent.config.json 和 .env。检查一下内容没问题：

bash
复制
cat super-agent.config.json


第三步：启动

bash
复制
pnpm start


Agent 就跑起来了。如果启用了飞书 Channel，现在就可以在飞书群里给 bot 发消息测试了。

用 pm2 守护进程

直接 pnpm start 会占住终端，关掉终端进程就挂了。我们用 pm2 来做进程管理——自动重启、日志管理、开机自启，一套搞定：

bash
复制
npm install -g pm2


启动 Agent：

bash
复制
pm2 start "pnpm start" --name super-agent


搞定。pm2 会在后台守护这个进程，挂了自动拉起。

常用命令：

bash
复制
pm2 logs super-agent       # 看日志
pm2 restart super-agent    # 重启
pm2 stop super-agent       # 停止
pm2 status                 # 查看所有进程状态


设置开机自启（服务器重启后 pm2 自动拉起所有进程）：

bash
复制
pm2 save                   # 保存当前进程列表
pm2 startup                # 生成开机自启脚本，按提示执行输出的命令即可

更新部署

后续更新流程：

bash
复制
git pull
pnpm install
pm2 restart super-agent

回头看：我们已经走了很远了

从第一节的 streamText 到现在，我们从零搭了一个完整的 AI Agent 框架：

Agent Loop：streamText 循环 + 工具调用 + 循环检测 + 容错重试
Tool System：注册表 + 并发锁 + 延迟加载 + MCP 协议接入
Context Engineering：Prompt Builder 管线 + Token 预算 + 压缩策略
记忆与知识库：Memory Store 持久化 + RAG 向量检索
Skill & Plugin：技能热加载 + 插件命名空间隔离
Channel：飞书 bot 长连接 + 会话隔离 + Dashboard
安全：Hook 管线 + Bash 命令分级 + 权限角色
Cron：定时任务 + 持久化 + 执行器
Sub-Agent：独立上下文 + 并行调度 + 深度/并发控制
配置系统：JSON Schema + 环境变量替换 + CLI init

每一层都是为了解决一个真实的工程问题。这些模块加在一起，就是一个 production-ready 的 Agent 框架的核心。

课程到这里就结束了。你手里有一份可以跑起来、能部署、能接飞书、能定时执行、能派子 Agent 的完整实现。

但作为 Agent Builder，你的 Agent 开发之旅并没有结束，关于 Agent 的核心认知你已经了然于胸，甚至已经超过了绝大部分的开发者，接下来就是拿它去解决你真实世界的各种业务问题了。如果有机会，我在后续也会持续推出不同业务场景、不同垂直领域的 Agent 实战课程，欢迎关注，我们下一门课程，再见！

上一篇
一个不够就拆成多个——实现 Sub-Agent 机制
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
import { z } from 'zod';

export const ModelConfigSchema = z.object({
  provider: z.enum(['dashscope', 'openai', 'custom']).default('dashscope'),
  name: z.string().default('qwen-plus-latest'),
  baseURL: z.string().default('https://dashscope.aliyuncs.com/compatible-mode/v1'),
  apiKey: z.string().default(''),
});

export const PluginConfigSchema = z.object({
  name: z.string(),
  enabled: z.boolean().default(true),
  config: z.record(z.string()).default({}),
});

export const FeishuChannelConfigSchema = z.object({
  enabled: z.boolean().default(false),
  appId: z.string().default(''),
  appSecret: z.string().default(''),
  port: z.number().default(3000),
});

export const ChannelConfigSchema = z.object({
  feishu: FeishuChannelConfigSchema.default({}),
});

export const AgentConfigSchema = z.object({
  maxSpawnDepth: z.number().min(0).max(5).default(1),
  maxConcurrent: z.number().min(1).max(10).default(3),
  defaultTimeout: z.number().default(60000),
});

export const SecurityConfigSchema = z.object({
  defaultRole: z.string().default('developer'),
  auditLog: z.boolean().default(true),
  bashTimestamp: z.boolean().default(true),
});

export const MemoryConfigSchema = z.object({
  dataDir: z.string().default('.'),
});

export const RagConfigSchema = z.object({
  enabled: z.boolean().default(true),
  docsDir: z.string().default('docs'),
});

export const CronConfigSchema = z.object({
  enabled: z.boolean().default(true),
  dataDir: z.string().default('.'),
});

export const SessionConfigSchema = z.object({
  id: z.string().default('default'),
});

export const UsageConfigSchema = z.object({
  trackingFile: z.string().default('.usage/today.jsonl'),
});

export const SuperAgentConfigSchema = z.object({
  version: z.string().default('1.0'),
  model: ModelConfigSchema.default({}),
  plugins: z.array(PluginConfigSchema).default([]),
  channels: ChannelConfigSchema.default({}),
  agents: AgentConfigSchema.default({}),
  security: SecurityConfigSchema.default({}),
  memory: MemoryConfigSchema.default({}),
  rag: RagConfigSchema.default({}),
  cron: CronConfigSchema.default({}),
  session: SessionConfigSchema.default({}),
  usage: UsageConfigSchema.default({}),
});

export type SuperAgentConfig = z.infer<typeof SuperAgentConfigSchema>;
```


```ts
import fs from 'node:fs';
import { SuperAgentConfigSchema, type SuperAgentConfig } from './schema.js';

export const CONFIG_FILE = 'super-agent.config.json';

const ENV_VAR_RE = /\$\{([A-Z_][A-Z0-9_]*)\}/g;

function substituteEnvVars(obj: unknown): unknown {
  if (typeof obj === 'string') {
    return obj.replace(ENV_VAR_RE, (match, name) => {
      const val = process.env[name];
      if (val === undefined) {
        console.warn(`  ⚠ 环境变量 ${name} 未设置，保留原值`);
        return match;
      }
      return val;
    });
  }
  if (Array.isArray(obj)) return obj.map(substituteEnvVars);
  if (obj !== null && typeof obj === 'object') {
    const result: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(obj as Record<string, unknown>)) {
      result[key] = substituteEnvVars(value);
    }
    return result;
  }
  return obj;
}

export function loadConfig(path = CONFIG_FILE): SuperAgentConfig {
  if (!fs.existsSync(path)) {
    console.log(`  未找到 ${path}，使用默认配置`);
    console.log('  运行 pnpm run init 生成配置文件\n');
    return SuperAgentConfigSchema.parse({});
  }

  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(path, 'utf-8'));
  } catch (err) {
    console.error(`  ✗ 解析 ${path} 失败: ${(err as Error).message}`);
    process.exit(1);
  }

  const substituted = substituteEnvVars(raw);

  const result = SuperAgentConfigSchema.safeParse(substituted);
  if (!result.success) {
    console.error('  ✗ 配置文件校验失败:');
    for (const issue of result.error.issues) {
      console.error(`    ${issue.path.join('.')}: ${issue.message}`);
    }
    process.exit(1);
  }

  console.log(`  ✓ 已加载 ${path}`);
  return result.data;
}
```


```ts
import { createInterface } from 'node:readline';
import fs from 'node:fs';
import { CONFIG_FILE } from './loader.js';

export async function runInit() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (q: string): Promise<string> =>
    new Promise((resolve) => {
      console.log(q);
      rl.question('  > ', resolve);
    });

  console.log('\n  Super Agent 初始化向导\n');

  if (fs.existsSync(CONFIG_FILE)) {
    const overwrite = await ask(`  ${CONFIG_FILE} 已存在，覆盖? (y/N): `);
    if (overwrite.toLowerCase() !== 'y') {
      console.log('  已取消\n');
      rl.close();
      return;
    }
  }

  // ── 模型选择 ──────────────────────────
  console.log('  选择模型:\n');
  console.log('    1. qwen-plus-latest   (推荐，均衡)');
  console.log('    2. qwen-turbo-latest  (快速，便宜)');
  console.log('    3. qwen-max-latest    (最强，贵)\n');
  const modelChoice = (await ask('  模型 [1]: ')) || '1';
  const models: Record<string, string> = {
    '1': 'qwen-plus-latest',
    '2': 'qwen-turbo-latest',
    '3': 'qwen-max-latest',
  };
  const modelName = models[modelChoice] || 'qwen-plus-latest';

  // ── API Key ──────────────────────────
  const apiKey = await ask('\n  DashScope API Key (留空则从环境变量 DASHSCOPE_API_KEY 读取): ');

  // ── 飞书 Channel ──────────────────────────
  const enableFeishu = (await ask('\n  启用飞书 Channel? (y/N): ')).toLowerCase() === 'y';
  let feishuAppId = '';
  let feishuAppSecret = '';
  if (enableFeishu) {
    feishuAppId = await ask('  飞书 App ID: ');
    feishuAppSecret = await ask('  飞书 App Secret: ');
  }

  // ── Sub-Agent ──────────────────────────
  const concurrentStr = await ask('\n  子 Agent 最大并发数 [3]: ');
  const maxConcurrent = parseInt(concurrentStr) || 3;

  // ── 生成配置 ──────────────────────────
  const config = {
    version: '1.0',
    model: {
      provider: 'dashscope',
      name: modelName,
      baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
      apiKey: apiKey || '${DASHSCOPE_API_KEY}',
    },
    plugins: [
      { name: 'supabase', enabled: false, config: {} },
    ],
    channels: {
      feishu: {
        enabled: enableFeishu,
        appId: enableFeishu ? feishuAppId : '${FEISHU_APP_ID}',
        appSecret: enableFeishu ? feishuAppSecret : '${FEISHU_APP_SECRET}',
        port: 3000,
      },
    },
    agents: {
      maxSpawnDepth: 1,
      maxConcurrent,
      defaultTimeout: 60000,
    },
    security: {
      defaultRole: 'developer',
      auditLog: true,
      bashTimestamp: true,
    },
    memory: { dataDir: '.' },
    rag: { enabled: true, docsDir: 'docs' },
    cron: { enabled: true, dataDir: '.' },
    session: { id: 'default' },
    usage: { trackingFile: '.usage/today.jsonl' },
  };

  fs.writeFileSync(CONFIG_FILE, JSON.stringify(config, null, 2) + '\n');
  console.log(`\n  ✓ ${CONFIG_FILE} 已生成`);

  // 生成 .env
  const envLines: string[] = [];
  if (apiKey) {
    envLines.push(`DASHSCOPE_API_KEY=${apiKey}`);
  }
  if (enableFeishu && feishuAppId) {
    envLines.push(`FEISHU_APP_ID=${feishuAppId}`);
    envLines.push(`FEISHU_APP_SECRET=${feishuAppSecret}`);
  }
  if (envLines.length > 0) {
    fs.writeFileSync('.env', envLines.join('\n') + '\n');
    console.log('  ✓ .env 已生成');
  }

  console.log('\n  启动 Agent: pnpm start\n');
  rl.close();
}
```


```ts
const command = process.argv[2];

if (command === 'init') {
  import('./config/init.js').then(m => m.runInit());
} else {
  import('./main.js').then(m => m.startAgent().catch(console.error));
}
```


```json
{
  "name": "super-agent",
  "version": "1.0.0",
  "type": "module",
  "bin": {
    "super-agent": "src/index.ts"
  },
  "scripts": {
    "start": "tsx src/index.ts",
    "init": "tsx src/index.ts init",
    "continue": "tsx src/index.ts --continue"
  }
}
```


```bash
pnpm start
```


```ts
const qwen = createOpenAI({
  baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
  apiKey: process.env.DASHSCOPE_API_KEY,
});
const model = process.env.DASHSCOPE_API_KEY
  ? qwen.chat('qwen-plus-latest')
  : createMockModel();
```


```ts
function createModel(cfg: SuperAgentConfig['model']) {
  if (!cfg.apiKey) return createMockModel();
  const provider = createOpenAI({ baseURL: cfg.baseURL, apiKey: cfg.apiKey });
  return provider.chat(cfg.name);
}
const model = createModel(config.model);
```


```ts
// 之前
const agentRegistry = new SubAgentRegistry({ maxSpawnDepth: 1, maxConcurrent: 3 });

// 现在
const agentRegistry = new SubAgentRegistry({
  maxSpawnDepth: config.agents.maxSpawnDepth,
  maxConcurrent: config.agents.maxConcurrent,
});
```


```ts
if (config.channels.feishu.enabled) {
  const feishuChannel = new FeishuChannel({
    appId: config.channels.feishu.appId,
    appSecret: config.channels.feishu.appSecret,
    port: config.channels.feishu.port,
  });
  gateway.register(feishuChannel);
}
```


```ts
if (config.security.auditLog) {
  hookPipeline.registerPre('audit-log', ...);
}
if (config.security.bashTimestamp) {
  hookPipeline.registerPost('bash-timestamp', ...);
}
```


```ts
for (const pluginCfg of config.plugins) {
  const def = availablePlugins.get(pluginCfg.name);
  if (!def) { console.log(`  ✗ ${pluginCfg.name} — 未知插件`); continue; }
  if (!pluginCfg.enabled) { console.log(`  - ${pluginCfg.name} — 已禁用`); continue; }
  const tools = await pluginManager.load(def);
  console.log(`  ✓ ${pluginCfg.name} — ${tools.length} 个工具`);
}
```


```bash
pnpm start
```


```json
{
  "version": "1.0",
  "model": {
    "provider": "dashscope",
    "name": "qwen-plus-latest",
    "baseURL": "https://dashscope.aliyuncs.com/compatible-mode/v1",
    "apiKey": "${DASHSCOPE_API_KEY}"
  },
  "plugins": [
    { "name": "supabase", "enabled": false, "config": {} }
  ],
  "channels": {
    "feishu": {
      "enabled": true,
      "appId": "${FEISHU_APP_ID}",
      "appSecret": "${FEISHU_APP_SECRET}",
      "port": 3000
    }
  },
  "agents": {
    "maxSpawnDepth": 1,
    "maxConcurrent": 3,
    "defaultTimeout": 60000
  },
  "security": {
    "defaultRole": "developer",
    "auditLog": true,
    "bashTimestamp": true
  },
  "cron": { "enabled": true, "dataDir": "." },
  "rag": { "enabled": true, "docsDir": "docs" }
}
```


```json
{ "agents": { "maxConcurrent": 5 } }
```


```bash
sudo dnf install -y git
git clone https://gitee.com/mirrors/nvm.git ~/.nvm && cd ~/.nvm && git checkout `git describe --abbrev=0 --tags`
sudo sh -c 'echo ". ~/.nvm/nvm.sh" >> /etc/profile && echo "export NVM_NODEJS_ORG_MIRROR=https://npmmirror.com/mirrors/node" >> /etc/profile'
# 解压到 /usr/local                                                             
source /etc/profile
nvm install 20
npm install -g pnpm pm2
```


```
node_modules/
.env
.sessions/
.memory/
.usage/
.cron/
super-agent.config.json
```


```bash
# 在服务器上
git clone <你的仓库地址> super-agent
cd super-agent
pnpm install
```


```bash
pnpm run init
```


```bash
cat super-agent.config.json
```


```bash
pnpm start
```


```bash
npm install -g pm2
```


```bash
pm2 start "pnpm start" --name super-agent
```


```bash
pm2 logs super-agent       # 看日志
pm2 restart super-agent    # 重启
pm2 stop super-agent       # 停止
pm2 status                 # 查看所有进程状态
```


```bash
pm2 save                   # 保存当前进程列表
pm2 startup                # 生成开机自启脚本，按提示执行输出的命令即可
```


```bash
git pull
pnpm install
pm2 restart super-agent
```
