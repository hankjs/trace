# Channel 抽象——让 Agent 活在飞书群里

课程
Super Agent 实战课
Channel 抽象——让 Agent 活在飞书群里
Channel 抽象——让 Agent 活在飞书群里

约 39 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

到目前为止，我们的 Agent 只活在终端里——你在命令行敲一句话，它回一句话。但实际用起来呢，大部分人跟 Agent 交互的场景不是终端，而是聊天工具。产品经理在飞书群里 @Agent 问"上周的 bug 修了几个"，运维在群里让 Agent 查个日志——这才是 Agent 的主战场。

问题是，如果你把飞书的消息收发逻辑直接写进 Agent Loop 里，那明天想接 Telegram、后天想接钉钉，代码就乱了。Agent 的核心逻辑不应该跟"消息从哪来"绑死。

这一节我们做两件事：一是设计 Channel 抽象层，把"消息从哪来、回复往哪发"这件事跟 Agent 核心解耦；二是实现一个飞书 Bot Channel，让你的 Agent 真正活在飞书群里。本地跑起来不需要飞书账号——用 curl 模拟 webhook 就能测试完整流程。

先装依赖（这次多了 hono 和 @hono/node-server，用来起 HTTP 服务接收 webhook）：

bash
运行
复制
pnpm install

Channel 是什么

Channel 这个概念其实顾名思义——就是 Agent 跟外界通信的通道。终端是一个 Channel，飞书群是一个 Channel，Telegram 是一个 Channel，甚至邮件也可以是一个 Channel。

每个 Channel 做三件事：接收外部消息、转成 Agent 统一格式、把 Agent 回复发回去。Agent Loop 本身不关心消息是从终端来的还是从飞书来的——它只看到 { role: 'user', content: '...' } 这个统一格式。

这就是为什么需要一个抽象层。没有这层抽象，每接一个新的消息源你都得改 Agent 核心代码。有了这层，加一个 Channel 就是加一个适配器，核心零改动。

设计 ChannelDefinition

跟 Plugin 一样，Channel 也需要一个接口契约。一个 Channel 要能启动、停止、发消息，还要能把收到的消息通知给 Gateway：

新建 src/channels/types.ts：

src/channels/types.ts
应用
复制
export interface IncomingMessage {
  channelId: string;
  senderId: string;
  senderName: string;
  text: string;
  raw?: unknown;
}

export interface OutgoingMessage {
  channelId: string;
  recipientId: string;
  text: string;
}

export interface ChannelDefinition {
  name: string;
  description: string;

  start(): Promise<void> | void;
  stop(): Promise<void> | void;
  send(message: OutgoingMessage): Promise<void>;

  onMessage?: (handler: (msg: IncomingMessage) => void) => void;
}


IncomingMessage 和 OutgoingMessage 是统一的消息格式——不管消息从哪来，进到 Agent 之前都得转成这个结构。channelId 标识会话（飞书里是 chat_id，Telegram 里是 chat.id），senderId 标识谁发的。

ChannelDefinition 的 onMessage 是一个回调注册方法——Channel 收到外部消息后调用这个 handler，Gateway 就能统一处理了。start/stop 管生命周期（启动 HTTP 服务器、关闭连接等）。

bash
运行
复制
pnpm start

实现 ChannelGateway

Gateway 是 Channel 系统的中枢——它注册所有 Channel，统一处理消息路由。收到任何 Channel 的消息后，丢给 Agent Loop 处理，拿到回复再通过同一个 Channel 发回去。

新建 src/channels/gateway.ts：

src/channels/gateway.ts
应用
复制
import type { ModelMessage } from 'ai';
import type { ChannelDefinition, IncomingMessage, OutgoingMessage } from './types.js';
import type { ToolRegistry } from '../tools/registry.js';
import { agentLoop } from '../agent/loop.js';

interface GatewayOptions {
  model: any;
  registry: ToolRegistry;
  buildSystem: () => string;
}

export class ChannelGateway {
  private channels = new Map<string, ChannelDefinition>();
  private sessions = new Map<string, ModelMessage[]>();
  private options: GatewayOptions;

  constructor(options: GatewayOptions) {
    this.options = options;
  }

  register(channel: ChannelDefinition): void {
    this.channels.set(channel.name, channel);

    channel.onMessage?.((msg: IncomingMessage) => {
      this.handleIncoming(channel.name, msg);
    });
  }

  async startAll(): Promise<void> {
    for (const [name, ch] of this.channels) {
      try {
        await ch.start();
        console.log(`  [gateway] ✓ ${name} 已启动`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [gateway] ✗ ${name} 启动失败: ${msg}`);
      }
    }
  }

  async stopAll(): Promise<void> {
    for (const [, ch] of this.channels) {
      await ch.stop();
    }
  }

  private async handleIncoming(channelName: string, msg: IncomingMessage): Promise<void> {
    const sessionKey = `${channelName}:${msg.senderId}`;
    console.log(`\n  [${channelName}] ${msg.senderName}: ${msg.text}`);

    if (!this.sessions.has(sessionKey)) {
      this.sessions.set(sessionKey, []);
    }
    const messages = this.sessions.get(sessionKey)!;

    const userMsg: ModelMessage = { role: 'user', content: msg.text };
    messages.push(userMsg);

    const system = this.options.buildSystem();
    await agentLoop(this.options.model, this.options.registry, messages, system);

    // 从 messages 里取最后一条 assistant 消息作为回复
    const lastMsg = messages[messages.length - 1];
    let replyText = '';
    if (lastMsg && lastMsg.role === 'assistant') {
      const content = lastMsg.content;
      if (typeof content === 'string') {
        replyText = content;
      } else if (Array.isArray(content)) {
        replyText = content
          .filter((c: any) => c.type === 'text')
          .map((c: any) => c.text)
          .join('');
      }
    }

    if (replyText) {
      const channel = this.channels.get(channelName);
      if (channel) {
        await channel.send({
          channelId: msg.channelId,
          recipientId: msg.senderId,
          text: replyText,
        });
        console.log(`  [${channelName}] → ${replyText.slice(0, 80)}${replyText.length > 80 ? '...' : ''}`);
      }
    }
  }

  list(): Array<{ name: string; description: string }> {
    return Array.from(this.channels.values()).map(ch => ({
      name: ch.name,
      description: ch.description,
    }));
  }
}


Gateway 里有几个值得聊聊的设计决策。

会话隔离：sessions 这个 Map 的 key 是 channelName:senderId——每个 Channel + 发送者的组合维护独立的对话历史。飞书群里张三问的问题不会出现在李四的上下文里。这跟我们在终端里只有一个 messages 数组不一样，多 Channel 场景下必须做隔离。

复用 Agent Loop：handleIncoming 调用的就是我们从第二篇就开始用的 agentLoop——同一个推理引擎、同一套工具、同一套 system prompt。Channel 只是换了输入输出的来源和去向，Agent 的"大脑"是共享的。这意味着你在终端里教会 Agent 的能力（Skill、Plugin、记忆），飞书群里也能用。

回复提取：Agent Loop 跑完之后，messages 数组末尾的 assistant 消息就是回复。但 assistant 消息的 content 可能是 string，也可能是 ContentPart 数组（比如包含 tool-call + text），所以提取时需要过滤出 type === 'text' 的部分拼起来。

bash
运行
复制
pnpm start

飞书 Bot 适配器

有了抽象层，接飞书就是实现一个 ChannelDefinition。飞书 SDK 提供了长连接模式——你的服务主动连接飞书的 WebSocket 服务器，消息通过长连接推过来，不需要公网域名。比传统的 Webhook 回调模式省事很多。

新建 src/channels/feishu.ts：

src/channels/feishu.ts
应用
复制
import type { ChannelDefinition, IncomingMessage, OutgoingMessage } from './types.js';

interface FeishuConfig {
  appId: string;
  appSecret: string;
  port: number;
}

export class FeishuChannel implements ChannelDefinition {
  name = 'feishu';
  description = '飞书 Bot 消息通道（长连接模式）';

  private config: FeishuConfig;
  private messageHandler?: (msg: IncomingMessage) => void;
  private httpServer?: any;
  private wsClient?: any;
  private larkClient?: any;

  // ... constructor / onMessage 省略 ...

  async start(): Promise<void> {
    await this.startDashboard(); // 状态面板，不管有没有配飞书都起

    if (!this.config.appId || !this.config.appSecret) {
      console.log('    飞书未配置，仅启动 Dashboard');
      return;
    }

    // 用飞书 SDK 的长连接模式
    const lark = await import('@larksuiteoapi/node-sdk');

    this.larkClient = new lark.Client({
      appId: this.config.appId,
      appSecret: this.config.appSecret,
    });

    const dispatcher = new lark.EventDispatcher({});

    dispatcher.register({
      'im.message.receive_v1': (data) => {
        if (data.message.message_type !== 'text') return;
        const content = JSON.parse(data.message.content);
        let text = content.text || '';
        // 去掉 @Bot 的 mention 标记
        if (data.message.mentions) {
          for (const m of data.message.mentions) {
            text = text.replace(m.key, '').trim();
          }
        }
        if (text && this.messageHandler) {
          this.messageHandler({
            channelId: data.message.chat_id,
            senderId: data.sender.sender_id?.open_id || 'unknown',
            senderName: data.sender.sender_id?.open_id || 'unknown',
            text,
            raw: data,
          });
        }
      },
    });

    this.wsClient = new lark.WSClient({
      appId: this.config.appId,
      appSecret: this.config.appSecret,
    });

    await this.wsClient.start({ eventDispatcher: dispatcher });
    console.log('    飞书长连接已建立（无需 ngrok）');
  }

  async send(message: OutgoingMessage): Promise<void> {
    if (!this.larkClient) {
      console.log(`    [feishu] 未配置飞书，跳过发送`);
      return;
    }
    await this.larkClient.im.message.create({
      params: { receive_id_type: 'chat_id' },
      data: {
        receive_id: message.channelId,
        msg_type: 'text',
        content: JSON.stringify({ text: message.text }),
      },
    });
  }

  // startDashboard() 省略——起一个 Hono HTTP 服务，
  // 根路径返回状态面板页，/webhook/feishu 保留模拟测试能力
}


这个适配器的核心逻辑分两块。

接收消息用的是飞书 SDK 的 WSClient + EventDispatcher。EventDispatcher 注册 im.message.receive_v1 事件处理器——收到消息后解析 content.text，去掉 @Bot 的 mention 标记，包装成 IncomingMessage 交给 Gateway。WSClient 负责跟飞书服务器建立 WebSocket 长连接，消息实时推过来，不需要你暴露任何公网端口。

发送回复用的是飞书 SDK 的 Client，直接调 im.message.create 发消息。SDK 内部自动管理 tenant_access_token 的获取和续期，不用你手动 refresh。

另外我们还起了一个 Hono HTTP 服务作为 Dashboard——根路径 / 返回一个状态面板页，上面有 Channel 状态和"发送测试消息"的表单。没配飞书环境变量时，用这个页面就能测试完整的 Channel 流程。Dashboard 的 /webhook/feishu 端点也保留了模拟 webhook 的能力，方便用 curl 测试。

/channel 命令

跟 Plugin 一样，Channel 也需要一个快捷命令来查看当前注册了哪些通道。新建 src/commands/channel.ts：

src/commands/channel.ts
应用
复制
import type { CommandHandler } from './index.js';
import type { ChannelGateway } from '../channels/gateway.js';

export function createChannelCommands(gateway: ChannelGateway): CommandHandler[] {
  return [
    (cmd, _ctx) => {
      if (cmd !== '/channel' && cmd !== '/channel list') return false;

      const channels = gateway.list();
      if (channels.length === 0) {
        console.log('\n[channels] 没有注册的通道。\n');
        return true;
      }

      console.log('\n[channels]');
      for (const ch of channels) {
        console.log(`  ${ch.name} — ${ch.description}`);
      }
      console.log('');
      return true;
    },
  ];
}


套路跟之前的 /plugin、/skill 命令一模一样——createChannelCommands 接收 gateway 实例，返回一个 CommandHandler，调用 gateway.list() 把所有已注册的 Channel 信息打印出来。

bash
运行
复制
pnpm start

接入主流程

三个新模块写好了，最后把它们串进 index.ts。跟 Plugin 那节一样，改动集中在三个地方：import、初始化、命令注册。

src/index.ts
应用
复制
// 在 import 区域新增：
import { ChannelGateway } from './channels/gateway.js';
import { FeishuChannel } from './channels/feishu.js';
import { createChannelCommands } from './commands/channel.js';

// ── Channel Gateway ────────────────────────────────
const gateway = new ChannelGateway({
  model,
  registry,
  buildSystem: () => builder.build(makePromptCtx()),
});

const FEISHU_PORT = Number(process.env.FEISHU_PORT || '3000');
const feishuChannel = new FeishuChannel({
  appId: process.env.FEISHU_APP_ID || '',
  appSecret: process.env.FEISHU_APP_SECRET || '',
  port: FEISHU_PORT,
});
gateway.register(feishuChannel);

// Commands 里注册 channel 命令
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
  ...ragCommands,
  ...dreamCommands,
  ...createSkillCommands(skillLoader, activeSkills),
  ...createPluginCommands(pluginManager, availablePlugins),
  ...createChannelCommands(gateway),  // ← 新增
]);

// main() 里启动 Channel
console.log('  启动 Channel...');
await gateway.startAll();

// 退出时 Graceful Shutdown
if (!trimmed || trimmed === 'exit') {
  console.log('Bye!');
  await gateway.stopAll();      // ← 新增：关闭所有 Channel
  await pluginManager.unloadAll();
  rl.close();
  return;
}


ChannelGateway 的构造参数跟 Agent Loop 一致——model、registry、buildSystem。这意味着从飞书群来的消息和终端里敲的消息，走的是同一套推理引擎和工具集。FeishuChannel 的配置从环境变量读取，没配飞书的 App ID / App Secret 也能正常启动——Dashboard 测试页不依赖飞书账号。

退出时多了一个 gateway.stopAll()——它会遍历所有已注册的 Channel 调用 stop()，关闭 HTTP 服务器和 WebSocket 连接，做到 Graceful Shutdown。

bash
运行
复制
pnpm start

本地测试

不需要真飞书账号就能测试完整流程。pnpm start 启动后，打开浏览器访问 http://localhost:3000，你会看到一个 Dashboard 页面——在「发送测试消息」输入框里输入消息，点发送就行。

也可以用 curl 测试：

bash
复制
curl -X POST http://localhost:3000/webhook/feishu \
  -H "Content-Type: application/json" \
  -d '{"header":{"event_type":"im.message.receive_v1"},"event":{"message":{"message_type":"text","content":"{\"text\":\"有哪些表\"}","chat_id":"test-chat"},"sender":{"sender_id":{"open_id":"test-user"}}}}'


你会在 Agent 的终端看到：

text
复制
  [feishu] test-user: 有哪些表

--- Step 1 ---
  [调用: supabase__list_tables({})]
  [结果: supabase__list_tables] {"tables":["users","posts","comments","sessions"],...}
  → 继续下一步...

--- Step 2 ---
  [feishu] 未配置 Token，跳过发送: 数据库里有这些表：...
  [feishu] → 数据库里有这些表：users, posts, comments, sessions


整个链路跑通了：curl 模拟飞书 webhook 发到 Hono HTTP 服务，解析成 IncomingMessage 后由 Gateway 路由到 Agent Loop，Agent 调用 supabase 工具查数据、生成回复，再尝试通过飞书 Channel 发回去（因为没配 token 只打了 log）。

注意 Agent 在处理飞书消息时用了 supabase 插件——这说明所有之前积累的能力（Plugin 注册的工具、Skill 的知识、记忆库的内容）在 Channel 场景下都是直接可用的。你不需要为飞书单独配一套工具，Gateway 复用的是同一个 ToolRegistry 和同一个 PromptBuilder。

接入真实飞书 Bot

如果你想让 Agent 真正在飞书群里回消息，需要先在飞书开放平台创建一个 Bot 应用。

打开飞书开放平台，点击右上角的「开发者后台」，然后你会看到一个「创建飞书智能体应用」的快捷入口，点「立即创建」一键搞定——它会自动帮你预置好 Bot 所需的权限和事件配置。

创建完成后，你会拿到 App ID 和 App Secret，然后把这两个值填到 .env 里：

bash
复制
# .env
FEISHU_APP_ID=cli_xxxxx
FEISHU_APP_SECRET=xxxxx


在「企业自建应用」列表里点击你的应用卡片进入管理后台，在左侧边栏找到「事件与回调」。

本地直接 pnpm start 启动服务。

然后在「事件订阅」配置里，选择长连接模式（飞书推荐的方式）。启动 Agent 后点「重新验证」，看到绿色的「连接成功」就说明 WebSocket 长连接建立好了。

因为我们用的是长连接模式，不需要公网域名——SDK 会主动连接飞书的 WebSocket 服务器，消息通过长连接实时推过来。 就能收发消息。

配好之后，群里有人 @你的 Bot 说话，消息就会通过 WebSocket 长连接推到你本地，Agent 处理完再通过飞书 API 回复到群里。

注意：接真实飞书 Bot 需要把项目下载到本地运行。不接飞书的话，用 Dashboard 页面上的「发送测试消息」就能完整体验 Channel 流程。

Channel 作为 Plugin 的扩展点

回顾 Plugin 那篇我们留了个伏笔——PluginDefinition 未来能注册更多类型的能力。现在 Channel 实现了，两者的集成路径就很清晰：往 PluginApi 加一个 registerChannel 方法，Plugin 就能动态注册通道了。

比如未来的 PluginApi 可以扩展成这样：

ts
复制
interface PluginApi {
  registerTools(tools: ToolDefinition[]): void;
  registerChannel(channel: ChannelDefinition): void;  // 新增
  getConfig(): PluginConfig;
  log(message: string): void;
}


那写一个 Telegram Plugin 就是这样：

ts
复制
const telegramPlugin: PluginDefinition = {
  name: 'telegram',
  version: '1.0.0',
  description: 'Telegram Bot 通道',

  activate(api: PluginApi) {
    const channel = new TelegramChannel({
      botToken: api.getConfig().botToken as string,
    });
    api.registerChannel(channel);
    api.log('Telegram 通道已注册');
  },
};


用户装上这个 Plugin，Agent 就自动多了一个 Telegram 通道——不需要改核心代码，不需要重新部署，动态加载完事。这也是为什么我们把 Channel 设计成接口而不是具体实现——ChannelDefinition 就像一份契约，只要适配器满足 start/stop/send/onMessage 这几个方法，Gateway 都能统一调度。

写在最后

这一节做了两层事情。表面上是"接了个飞书 Bot"，但更本质的是把 Agent 的智能和通信彻底分离了。智能在 Agent Loop 里演化（加工具、加 Skill、优化 prompt），通信在 Channel 里演化（接飞书、接 Telegram、接邮件），两边互不干扰。

这个分离带来一个很实用的好处：你在终端里调试好的一切能力——记忆、RAG、Plugin——飞书群里自动就能用。不需要为每个 Channel 单独适配功能，因为所有 Channel 共享同一个 Agent Loop。这就是抽象的力量，也是为什么我们花时间先设计接口再写实现。

到这里，我们的 Agent 已经具备了相当完整的能力矩阵：Agent Loop 引擎、工具系统、MCP 协议、上下文管理、记忆、RAG、Skill、Plugin、Channel。它不再是一个只能在终端里跑的 demo，而是一个能部署到真实场景里的产品雏形了。

下一节我们进入安全领域——权限系统和 Hook 管线。Agent 能做的事情越来越多了，但"能做"不等于"该做"。哪些工具需要用户确认才能执行、哪些操作直接拒绝、执行前后能不能插入自定义检查逻辑——这些是把 Agent 从玩具变成生产级工具的最后几块拼图。

参考资料
飞书开放平台 - 事件订阅 — 事件回调机制
飞书开放平台 - 发送消息 — 消息发送 API
Hono — 轻量 Web 框架
飞书 SDK (@larksuiteoapi/node-sdk) — 官方 Node.js SDK，支持长连接模式
上一篇
Plugin 架构——让别人给你的 Agent 写功能
下一篇 · 第六章：权限 + Cron + Multi-Agent
权限系统 + Hook 管线——让 Agent 安全地被别人使用
编辑器


---
## 代码块


```bash
pnpm install
```


```ts
export interface IncomingMessage {
  channelId: string;
  senderId: string;
  senderName: string;
  text: string;
  raw?: unknown;
}

export interface OutgoingMessage {
  channelId: string;
  recipientId: string;
  text: string;
}

export interface ChannelDefinition {
  name: string;
  description: string;

  start(): Promise<void> | void;
  stop(): Promise<void> | void;
  send(message: OutgoingMessage): Promise<void>;

  onMessage?: (handler: (msg: IncomingMessage) => void) => void;
}
```


```bash
pnpm start
```


```ts
import type { ModelMessage } from 'ai';
import type { ChannelDefinition, IncomingMessage, OutgoingMessage } from './types.js';
import type { ToolRegistry } from '../tools/registry.js';
import { agentLoop } from '../agent/loop.js';

interface GatewayOptions {
  model: any;
  registry: ToolRegistry;
  buildSystem: () => string;
}

export class ChannelGateway {
  private channels = new Map<string, ChannelDefinition>();
  private sessions = new Map<string, ModelMessage[]>();
  private options: GatewayOptions;

  constructor(options: GatewayOptions) {
    this.options = options;
  }

  register(channel: ChannelDefinition): void {
    this.channels.set(channel.name, channel);

    channel.onMessage?.((msg: IncomingMessage) => {
      this.handleIncoming(channel.name, msg);
    });
  }

  async startAll(): Promise<void> {
    for (const [name, ch] of this.channels) {
      try {
        await ch.start();
        console.log(`  [gateway] ✓ ${name} 已启动`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`  [gateway] ✗ ${name} 启动失败: ${msg}`);
      }
    }
  }

  async stopAll(): Promise<void> {
    for (const [, ch] of this.channels) {
      await ch.stop();
    }
  }

  private async handleIncoming(channelName: string, msg: IncomingMessage): Promise<void> {
    const sessionKey = `${channelName}:${msg.senderId}`;
    console.log(`\n  [${channelName}] ${msg.senderName}: ${msg.text}`);

    if (!this.sessions.has(sessionKey)) {
      this.sessions.set(sessionKey, []);
    }
    const messages = this.sessions.get(sessionKey)!;

    const userMsg: ModelMessage = { role: 'user', content: msg.text };
    messages.push(userMsg);

    const system = this.options.buildSystem();
    await agentLoop(this.options.model, this.options.registry, messages, system);

    // 从 messages 里取最后一条 assistant 消息作为回复
    const lastMsg = messages[messages.length - 1];
    let replyText = '';
    if (lastMsg && lastMsg.role === 'assistant') {
      const content = lastMsg.content;
      if (typeof content === 'string') {
        replyText = content;
      } else if (Array.isArray(content)) {
        replyText = content
          .filter((c: any) => c.type === 'text')
          .map((c: any) => c.text)
          .join('');
      }
    }

    if (replyText) {
      const channel = this.channels.get(channelName);
      if (channel) {
        await channel.send({
          channelId: msg.channelId,
          recipientId: msg.senderId,
          text: replyText,
        });
        console.log(`  [${channelName}] → ${replyText.slice(0, 80)}${replyText.length > 80 ? '...' : ''}`);
      }
    }
  }

  list(): Array<{ name: string; description: string }> {
    return Array.from(this.channels.values()).map(ch => ({
      name: ch.name,
      description: ch.description,
    }));
  }
}
```


```bash
pnpm start
```


```ts
import type { ChannelDefinition, IncomingMessage, OutgoingMessage } from './types.js';

interface FeishuConfig {
  appId: string;
  appSecret: string;
  port: number;
}

export class FeishuChannel implements ChannelDefinition {
  name = 'feishu';
  description = '飞书 Bot 消息通道（长连接模式）';

  private config: FeishuConfig;
  private messageHandler?: (msg: IncomingMessage) => void;
  private httpServer?: any;
  private wsClient?: any;
  private larkClient?: any;

  // ... constructor / onMessage 省略 ...

  async start(): Promise<void> {
    await this.startDashboard(); // 状态面板，不管有没有配飞书都起

    if (!this.config.appId || !this.config.appSecret) {
      console.log('    飞书未配置，仅启动 Dashboard');
      return;
    }

    // 用飞书 SDK 的长连接模式
    const lark = await import('@larksuiteoapi/node-sdk');

    this.larkClient = new lark.Client({
      appId: this.config.appId,
      appSecret: this.config.appSecret,
    });

    const dispatcher = new lark.EventDispatcher({});

    dispatcher.register({
      'im.message.receive_v1': (data) => {
        if (data.message.message_type !== 'text') return;
        const content = JSON.parse(data.message.content);
        let text = content.text || '';
        // 去掉 @Bot 的 mention 标记
        if (data.message.mentions) {
          for (const m of data.message.mentions) {
            text = text.replace(m.key, '').trim();
          }
        }
        if (text && this.messageHandler) {
          this.messageHandler({
            channelId: data.message.chat_id,
            senderId: data.sender.sender_id?.open_id || 'unknown',
            senderName: data.sender.sender_id?.open_id || 'unknown',
            text,
            raw: data,
          });
        }
      },
    });

    this.wsClient = new lark.WSClient({
      appId: this.config.appId,
      appSecret: this.config.appSecret,
    });

    await this.wsClient.start({ eventDispatcher: dispatcher });
    console.log('    飞书长连接已建立（无需 ngrok）');
  }

  async send(message: OutgoingMessage): Promise<void> {
    if (!this.larkClient) {
      console.log(`    [feishu] 未配置飞书，跳过发送`);
      return;
    }
    await this.larkClient.im.message.create({
      params: { receive_id_type: 'chat_id' },
      data: {
        receive_id: message.channelId,
        msg_type: 'text',
        content: JSON.stringify({ text: message.text }),
      },
    });
  }

  // startDashboard() 省略——起一个 Hono HTTP 服务，
  // 根路径返回状态面板页，/webhook/feishu 保留模拟测试能力
}
```


```ts
import type { CommandHandler } from './index.js';
import type { ChannelGateway } from '../channels/gateway.js';

export function createChannelCommands(gateway: ChannelGateway): CommandHandler[] {
  return [
    (cmd, _ctx) => {
      if (cmd !== '/channel' && cmd !== '/channel list') return false;

      const channels = gateway.list();
      if (channels.length === 0) {
        console.log('\n[channels] 没有注册的通道。\n');
        return true;
      }

      console.log('\n[channels]');
      for (const ch of channels) {
        console.log(`  ${ch.name} — ${ch.description}`);
      }
      console.log('');
      return true;
    },
  ];
}
```


```bash
pnpm start
```


```ts
// 在 import 区域新增：
import { ChannelGateway } from './channels/gateway.js';
import { FeishuChannel } from './channels/feishu.js';
import { createChannelCommands } from './commands/channel.js';

// ── Channel Gateway ────────────────────────────────
const gateway = new ChannelGateway({
  model,
  registry,
  buildSystem: () => builder.build(makePromptCtx()),
});

const FEISHU_PORT = Number(process.env.FEISHU_PORT || '3000');
const feishuChannel = new FeishuChannel({
  appId: process.env.FEISHU_APP_ID || '',
  appSecret: process.env.FEISHU_APP_SECRET || '',
  port: FEISHU_PORT,
});
gateway.register(feishuChannel);

// Commands 里注册 channel 命令
const dispatch = createDispatcher([
  ...debugCommands,
  ...contextCommands,
  ...memoryCommands,
  ...ragCommands,
  ...dreamCommands,
  ...createSkillCommands(skillLoader, activeSkills),
  ...createPluginCommands(pluginManager, availablePlugins),
  ...createChannelCommands(gateway),  // ← 新增
]);

// main() 里启动 Channel
console.log('  启动 Channel...');
await gateway.startAll();

// 退出时 Graceful Shutdown
if (!trimmed || trimmed === 'exit') {
  console.log('Bye!');
  await gateway.stopAll();      // ← 新增：关闭所有 Channel
  await pluginManager.unloadAll();
  rl.close();
  return;
}
```


```bash
pnpm start
```


```bash
curl -X POST http://localhost:3000/webhook/feishu \
  -H "Content-Type: application/json" \
  -d '{"header":{"event_type":"im.message.receive_v1"},"event":{"message":{"message_type":"text","content":"{\"text\":\"有哪些表\"}","chat_id":"test-chat"},"sender":{"sender_id":{"open_id":"test-user"}}}}'
```


```text
[feishu] test-user: 有哪些表

--- Step 1 ---
  [调用: supabase__list_tables({})]
  [结果: supabase__list_tables] {"tables":["users","posts","comments","sessions"],...}
  → 继续下一步...

--- Step 2 ---
  [feishu] 未配置 Token，跳过发送: 数据库里有这些表：...
  [feishu] → 数据库里有这些表：users, posts, comments, sessions
```


```bash
# .env
FEISHU_APP_ID=cli_xxxxx
FEISHU_APP_SECRET=xxxxx
```


```ts
interface PluginApi {
  registerTools(tools: ToolDefinition[]): void;
  registerChannel(channel: ChannelDefinition): void;  // 新增
  getConfig(): PluginConfig;
  log(message: string): void;
}
```


```ts
const telegramPlugin: PluginDefinition = {
  name: 'telegram',
  version: '1.0.0',
  description: 'Telegram Bot 通道',

  activate(api: PluginApi) {
    const channel = new TelegramChannel({
      botToken: api.getConfig().botToken as string,
    });
    api.registerChannel(channel);
    api.log('Telegram 通道已注册');
  },
};
```
