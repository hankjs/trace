# ACP：标准化 Agent 的控制接口

课程
吃透 AI Agent 开发
ACP：标准化 Agent 的控制接口
ACP：标准化 Agent 的控制接口

约 23 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

假设你做了一个 Coding Agent，功能挺好的。Zed 编辑器想接你的 Agent，你写了一套集成代码。然后 JetBrains 也想接，消息格式不一样，又写了一套。Telegram Bot 要接，又一套。Web App 要嵌，再来一套……

每多一个客户端，就多一份集成代码。消息格式不同、权限机制不同、流式输出方式也不同。到最后你会发现，写适配层花掉的时间，比做 Agent 本身还多。

这个问题其实十年前就出现过。那时候每个编辑器（VS Code、Vim、Sublime）都要为每种语言（TypeScript、Python、Go）单独写插件——代码补全、跳转定义、重命名，每个编辑器各写一遍。N 个编辑器 × M 种语言 = N×M 个插件，谁也受不了。

后来微软搞了 LSP（Language Server Protocol），一下子解决了这个问题——每种语言只需要实现一个 Language Server，所有支持 LSP 的编辑器都能用。N×M 变成了 N+M。

现在 Agent 生态碰到了一模一样的问题。ACP（Agent Client Protocol）就是 Agent 世界的 LSP——标准化了"任何客户端"和"任何 Agent"之间的通信协议。写一次 Agent，所有支持 ACP 的客户端都能接。

ACP 由 Zed（做编辑器的公司）发起，JetBrains 协作推动，Apache-2.0 开源。这个出身很重要——发起方是编辑器厂商，跟任何一家模型公司都没有绑定关系。正因为这个中立的出发点，Claude Code、Codex CLI、Gemini CLI、Cursor、Cline、OpenCode 这些互相竞争的产品才愿意同时支持同一个协议。

先排一个坑：有两个 "ACP"

先帮你排个雷——你搜 ACP 大概率会撞到两个完全不同的协议。

一个是本篇讲的 Agent Client Protocol（Zed 发起）。另一个是 IBM 在 2025 年初为 BeeAI 平台做的 Agent Communication Protocol，解决的是 Agent 和 Agent 之间的通信。后者已经在 2025 年 9 月正式并入 A2A 协议，归到 Linux Foundation 旗下，团队停止独立演进。

所以现在的格局很清楚：客户端到 Agent 这个方向，ACP 指的就是 Zed 那个；Agent 到 Agent 这个方向，标准收敛到了 A2A。 这篇我们把两个方向一起讲掉——前半段讲 ACP，后半段讲 A2A。

ACP 和 MCP 的区别

前面 MCP 那篇讲过，MCP（Model Context Protocol）是 Anthropic 做的，解决的是"Agent 怎么连接外部工具"。

ACP 解决的是相反方向的问题——外部系统怎么驱动你的 Agent。

一个完整的 Agent 生态需要两个协议。这跟 Web 开发里 HTTP（前端和后端之间）+ SQL（后端和数据库之间）是同一个道理——一层解决"怎么接收请求"，另一层解决"怎么获取数据"。你不会用 SQL 来接收用户的 HTTP 请求，也不会用 HTTP 去查数据库，它们各自负责一个方向。ACP 和 MCP 也是一样。

一次完整的 ACP 交互

ACP 的底层通信方式跟 MCP 一样——客户端启动 Agent 作为子进程，通过 stdin 发消息、stdout 收消息，消息格式是 JSON-RPC 2.0（每条消息一行 JSON）。如果你已经理解了 MCP 的通信方式，ACP 的传输层你就已经会了。

与其干讲概念，不如走一遍实际流程。假设你在 Zed 里输入"帮我给这个函数加上错误处理"，协议层面发生了什么：

第一步：握手。 客户端和 Agent 交换 initialize 消息，互相声明自己支持哪些能力（capabilities）。这个协商机制是 ACP 演进的关键设计——像 session/fork（复制一个会话出来做实验）这种 unstable 阶段的新功能，都靠 capability 声明渐进上线：双方都声明支持才启用，老客户端碰到新 Agent 也不会坏。

第二步：创建会话。 Zed 发一个 newSession 请求，Agent 返回 session ID。会话之间互相独立，关掉编辑器明天再打开，用 loadSession 就能恢复，不用从头来。

第三步：发指令、流式返回。 prompt 发过去之后，Agent 边干边推送 sessionUpdate——先推 token（你看到 Agent 在"打字"），然后推一条"我要调用 read_file 读取 utils.ts"的状态更新，工具跑完继续推。这就是流式响应那篇讲的事件流思想，只不过这次被写进了协议标准。

第四步：权限请求。 Agent 准备修改 utils.ts 了，写文件是敏感操作——它发一个 requestPermission，Zed 弹确认框，你点允许，Agent 继续。

第五步：返回结果。 修改完成，推送最终结果，这一轮结束。

这整个流程解决了一个普通 REST API 处理不了的问题——Agent 执行过程中需要跟客户端来回交互。HTTP 的请求-响应模式是单向的，中间没有"服务端反过来问客户端一个问题再等回答"的标准方式。但 Agent 恰恰需要这个：跑到一半要请求权限、要用户确认、要汇报中间状态。

还有一个容易被忽略的设计：ACP 允许 Agent 在客户端那边创建终端跑命令。乍一看有点绕——Agent 自己不就能跑命令吗？关键在于执行环境的归属。Agent 可能跑在远程容器里，但用户要改的项目、装好的依赖、配好的凭证都在本地。把终端开在客户端这边，命令就跑在用户真实的环境里，用户还能亲眼看到输出。这跟权限请求是同一个哲学：敏感的东西留在用户的环境里面。

权限代理：ACP 的标准化解法

在 Swarm 那篇里我们讲过 Permission Sync 的问题——Worker Agent 跑在 tmux 后台没有 UI，遇到需要确认的操作，通过 pending/resolved 文件机制把请求转给 Leader。

ACP 的权限代理本质上是同一个问题，但距离更远——Agent 可能跑在远程服务器上，客户端可能是 IDE、Telegram Bot 或者一个网页。中间隔着网络，共享文件系统这条路走不通了，权限请求必须序列化之后在协议层传输。

ACP 的标准化解法：Agent 发 requestPermission（带操作类型、参数、风险说明），客户端呈现给用户，用户选择，结果返回给 Agent。

实际接入：没你想的那么重

一个最小的 ACP Agent 长什么样？我们用 TypeScript SDK 大概是这样：

typescript
复制
import * as acp from "@agentclientprotocol/sdk";
import { Readable, Writable } from "node:stream";

class MyAgent {
  connection: acp.AgentSideConnection;

  constructor(connection: acp.AgentSideConnection) {
    this.connection = connection;
  }

  // 1. 协议握手——告诉客户端你支持什么能力
  async initialize() {
    return { protocolVersion: acp.PROTOCOL_VERSION, agentCapabilities: {} };
  }

  // 2. 创建会话——每个对话一个独立的 session
  async newSession() {
    return { sessionId: crypto.randomUUID() };
  }

  // 3. 处理用户指令——这是核心
  async prompt(params: acp.PromptRequest) {
    // 流式推 token 给客户端
    await this.connection.sessionUpdate({
      sessionId: params.sessionId,
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "正在分析你的代码..." },
      },
    });

    // 遇到敏感操作，请求用户授权
    const result = await this.connection.requestPermission({
      sessionId: params.sessionId,
      toolCall: {
        toolCallId: "call_1",
        title: "修改 config.json",
        kind: "edit",
        status: "pending",
        locations: [{ path: "/project/config.json" }],
      },
      options: [
        { kind: "allow_once", name: "允许", optionId: "allow" },
        { kind: "reject_once", name: "拒绝", optionId: "deny" },
      ],
    });

    if (result.outcome.outcome === "cancelled") {
      return { stopReason: "cancelled" };
    }

    return { stopReason: "end_turn" };
  }
}

// 通过 stdin/stdout 跟客户端通信
const stream = acp.ndJsonStream(
  Writable.toWeb(process.stdout),
  Readable.toWeb(process.stdin)
);
new acp.AgentSideConnection((conn) => new MyAgent(conn), stream);


骨架就是三个方法：initialize（握手）、newSession（建会话）、prompt（干活）。流式推送用 connection.sessionUpdate，请求授权用 connection.requestPermission——前面那五步流程，就靠这几个 API。

从协议到生产：协议不管的事

实现协议只是及格线。ACP 规定了消息格式和交互流程，但真正上生产时，有一堆问题协议一概不管，全要你自己补：

并发怎么排队：同一个会话内的请求必须串行（两条 prompt 同时改一个会话状态，必然乱），不同会话之间可以并行。这就是上下文拆分那篇讲的 OpenClaw 用的 Lane 队列思想，在协议接入层又用了一遍。
怎么防被打挂：ACP 把你的 Agent 开放给了外部系统，攻击面比纯本地大了一圈。限频、payload 大小上限、幂等去重、参数校验——这些前面 API 容错那篇讲过的防线，到了协议层一个都不能少。
长连接出现问题怎么办：即 SSE 超时处理、弱网环境下断连后的状态怎么恢复，协议没有规定这些，但生产环境里不处理就是定时炸弹。

总而言之，协议只管消息格式，Agent Loop 里面生产环境的问题都得自己来解决，而这些问题我们已经在前面的课程中一一拆解过了。

ACP 的生态走到哪了

截至 2026 年 6 月，官方 Agent 列表已经有 40+ 个实现——Claude Code、Codex CLI、GitHub Copilot CLI、Gemini CLI、OpenCode、Cline、Goose、Factory Droid，连 Docker 的 cagent 都在里面。

SDK 方面，官方维护 TypeScript 和 Rust，社区补上了 Go（Coder 出品）和 Python。协议本身还在快速演进，session/fork、会话元信息更新、elicitation 这些能力还标着 unstable——好在前面说过，capability 协商机制保证了新旧实现可以共存。

这里顺便划一下边界：同样做"Agent 连接用户界面"这层，还有一个叫 AG-UI 的协议（CopilotKit 发起，已经拿到 Microsoft Agent Framework 的官方集成）。两者乍看重叠，区别在于连接方式和侧重点：

ACP 源自编辑器场景，客户端用 stdio 拉起本地子进程，侧重本地 Agent 的控制；
AG-UI 是基于事件流的协议（支持 SSE、WebSocket、Webhook 等传输），侧重把远程 Agent 接入任何面向用户的应用——Web 是最常见的场景，但它的 SDK 覆盖了 Kotlin、Go、Dart 等语言，所以移动端和桌面端同样适用。
A2A：当对面也是一个 Agent

ACP 讲完了，我们换一个场景——如果接入你 Agent 的客户端，本身也是一个 Agent 呢？

这个场景正在变得很现实：比如你的研究 Agent 需要订机票，而航空公司有自己的官方 Agent；你的编码 Agent 想把法务审查委托给律所的合规 Agent。对方的 Agent 跑在别人家的服务器上，用的什么模型、什么框架你不知道，也不该知道。

这就是 A2A（Agent2Agent Protocol）管的事。Google 在 2025 年 4 月发起，现在由 Linux Foundation 托管，150+ 组织参与——前面提到的 IBM ACP 团队，并入的就是这里。

A2A 的核心主要分为三层：

Agent Card——Agent 的名片。 每个 A2A Agent 在固定路径（/.well-known/ 下的 JSON 文件）发布一份自描述：我是谁、服务端点在哪、有哪些技能（skills）、需要什么认证。别人的 Agent 先抓你的名片，确认你能干这活、谈妥认证方式，然后才开始对话。

Task——一次有状态的委托。 客户端 Agent 发起一个 Task，远端 Agent 接单后，Task 沿着状态机走：提交、执行中、需要补充输入、完成或失败。注意"需要补充输入"这个状态——远端 Agent 干到一半可以停下来反问，等客户端 Agent 补完信息再继续。

Artifact——只交结果，不交过程。 任务完成后，远端 Agent 交付的是结构化的产出物（文本、文件、数据），它的内部推理、上下文、工具调用记录全程不暴露。A2A 把这个设计叫 opaque agents（不透明 Agent）——每个 Agent 对外都是黑盒。

Multi-Agent 那篇我们花了一整篇论证：拆 Agent 的本质是隔离上下文，子 Agent 烧几万 token 探索，只把 1-2K 的结论压缩回传。A2A 的 opaque 设计，就是把这条原则标准化了——你家的 Agent 和别人家的 Agent 协作，凭什么共享上下文？商业机密、用户隐私、提示词资产，全都藏在黑盒里，接口上只有 Task 进、Artifact 出。

可能你会问：现在要不要用上 A2A 协议呢？我的结论是暂时不用着急。因为它解决的是跨组织协作，你得先有"别人家的 Agent 想跟你的 Agent 做生意"这个前提，说白了，Agent 生态得先做起来，A2A 才有充分的用武之地。

但经过这一节的学习，它的三个抽象层次——Agent Card、Task、Artifact 交付——值得你去学习和理解，因为这就是 Multi-Agent 协作走向规模化之后的一个标准形态。

三个协议拼成一张图

到这里，Agent 的对外接口就拼完整了：

MCP——Agent 向外伸手，连接工具和数据。
ACP——客户端向内驱动，把你的 Agent 接进编辑器、终端、消息平台等等各个客户端本身。
A2A——Agent 之间横向协作，跨组织地发现、委托、交付。

这门课到这里，Agent 的核心技术专题就全部讲完了——从 Agent Loop 的心跳，到工具系统的手脚，到上下文工程的血液，到记忆系统，到 Multi-Agent 协作，到 Harness，再到部署和协议标准化。下一章我们回到框架，用 LangGraph 重新实现前面手写的 Agent，对比两种写法的优劣——当你真正理解了底层原理之后再看框架，视角会完全不一样。

参考资料
ACP 官方文档: https://agentclientprotocol.com/
ACP 官方规范仓库: https://github.com/agentclientprotocol/agent-client-protocol
ACP 支持的 Agents 列表: https://agentclientprotocol.com/get-started/agents
Zed: The ACP Registry is Live: https://zed.dev/blog/acp-registry
Zed: How the Community is Driving ACP Forward: https://zed.dev/blog/acp-progress-report
OpenClaw ACP 文档: https://docs.openclaw.ai/cli/acp
A2A 官方文档: https://a2a-protocol.org/
A2A 发布博客 (Google): https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/
IBM ACP 并入 A2A 的公告 (Linux Foundation): https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/
AG-UI 协议: https://github.com/ag-ui-protocol/ag-ui
Kiro CLI 的 ACP 支持: https://kiro.dev/docs/cli/acp/
MCP 规范: https://modelcontextprotocol.io/specification
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
部署与调度：Agent 跑在哪、什么时候跑
下一篇 · 第七章：回到框架
LangGraph 实战：用图的方式重新理解 Agent Loop


---
## 代码块


```typescript
import * as acp from "@agentclientprotocol/sdk";
import { Readable, Writable } from "node:stream";

class MyAgent {
  connection: acp.AgentSideConnection;

  constructor(connection: acp.AgentSideConnection) {
    this.connection = connection;
  }

  // 1. 协议握手——告诉客户端你支持什么能力
  async initialize() {
    return { protocolVersion: acp.PROTOCOL_VERSION, agentCapabilities: {} };
  }

  // 2. 创建会话——每个对话一个独立的 session
  async newSession() {
    return { sessionId: crypto.randomUUID() };
  }

  // 3. 处理用户指令——这是核心
  async prompt(params: acp.PromptRequest) {
    // 流式推 token 给客户端
    await this.connection.sessionUpdate({
      sessionId: params.sessionId,
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "正在分析你的代码..." },
      },
    });

    // 遇到敏感操作，请求用户授权
    const result = await this.connection.requestPermission({
      sessionId: params.sessionId,
      toolCall: {
        toolCallId: "call_1",
        title: "修改 config.json",
        kind: "edit",
        status: "pending",
        locations: [{ path: "/project/config.json" }],
      },
      options: [
        { kind: "allow_once", name: "允许", optionId: "allow" },
        { kind: "reject_once", name: "拒绝", optionId: "deny" },
      ],
    });

    if (result.outcome.outcome === "cancelled") {
      return { stopReason: "cancelled" };
    }

    return { stopReason: "end_turn" };
  }
}

// 通过 stdin/stdout 跟客户端通信
const stream = acp.ndJsonStream(
  Writable.toWeb(process.stdout),
  Readable.toWeb(process.stdin)
);
new acp.AgentSideConnection((conn) => new MyAgent(conn), stream);
```
