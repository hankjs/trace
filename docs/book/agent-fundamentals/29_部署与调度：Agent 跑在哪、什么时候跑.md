# 部署与调度：Agent 跑在哪、什么时候跑

课程
吃透 AI Agent 开发
部署与调度：Agent 跑在哪、什么时候跑
部署与调度：Agent 跑在哪、什么时候跑

约 21 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

设想一个场景，你用 Vercel Functions 部署了一个 Agent API。用户发来一个请求："帮我重构这个模块的测试文件"。Agent 开始干活——读文件、分析代码、写测试、跑测试、发现报错、修改、再跑测试。

3 分钟过去了，Vercel 报 504 Gateway Timeout。Agent 已经读了 15 个文件、写了 3 个测试、还在跑测试呢——但函数超时了，所有工作全白费了。

这不是代码的 bug，是架构层面的根本不匹配。传统 Web 应用是毫秒级响应、无状态的。Agent 是分钟级执行、有状态、有副作用的长任务。 传统 Serverless（Lambda 最长 15 分钟、Vercel Functions 免费版 60 秒）的超时限制、无状态设计、冷启动开销，对 Agent 来说几乎不可用。

那 Agent 该怎么部署？这是一个很多人做完 Agent 原型后才意识到的问题——本地跑得好好的，一上线就各种超时、断连、状态丢失。这一节，我们就来聊这个问题：Agent 到底怎么来部署到生产环境。

最简单的方案：一个长驻进程搞定一切

大部分 Agent 产品的部署方式其实很简单——一个 Node.js 或 Python 服务，同时处理 API 请求和 Agent 执行，部署到一个长驻容器上。

用户发请求，服务端开一个 SSE 连接，Agent 在同一个进程里执行任务，token 流式推给用户。不需要消息队列，不需要 Serverless，架构非常简单。

主要的平台选择包括 Fly.io（全球分布、独立 CPU/内存）、Railway（开发者体验好、自动扩缩容）、Render（后端友好、有自带数据库），月费 $5-20 起步。核心要求就一个：没有超时限制，给 Agent 充分的时间去跑。

typescript
复制
// 一个普通的 Express 服务 + SSE
app.post("/api/agent", async (req, res) => {
  res.setHeader("Content-Type", "text/event-stream")
  res.setHeader("Cache-Control", "no-cache")

  const stream = await runAgent(req.body.prompt)
  for await (const chunk of stream) {
    res.write(`data: ${JSON.stringify(chunk)}\n\n`)
  }
  res.end()
})


前面流式架构那篇讲过 SSE 的原理，这里就是它的直接应用。Agent 一边想一边推 token，用户看到的是"打字机效果"。

有一个坑需要注意：如果你的网络环境有反向代理（Nginx、Cloudflare），它们通常有 60-120 秒的空闲超时。Agent 在执行一个耗时的工具调用时（比如跑测试），SSE 流会沉默一段时间，代理可能会主动断开连接。解决办法是心跳机制——工具执行期间定期发一个空的 SSE 注释（: heartbeat\n\n），告诉代理"我还活着"。

SSE 和 WebSocket 怎么选呢？大部分 Agent 场景用 SSE 就够了——本质上是服务器单向推送。如果涉及到需要用户审批、决策的情况，我们可以配合 SSE 和 HTTP Post 请求来完成，具体内容可以回到之前讲流式架构的小节复习。

什么时候需要升级架构

上面的单进程方案在大部分场景下都够用。但有两种情况你可能需要升级：

第一种：用户不需要等结果。 比如用户提交一个"帮我重构这个模块"的任务，不想盯着屏幕等——他要的是"任务完成后通知我"。这时候需要把 API 响应和 Agent 执行解耦：API 立刻返回"已接收"，Agent 在后台跑，完成后通过 webhook 或推送通知用户。

第二种：并发量大，需要任务调度。 100 个用户同时发请求，你的单进程可能扛不住。需要消息队列来排队，多个 Worker 并行消费。

这两种情况可以用 Inngest（事件驱动，免费层 5 万次/月）或 Trigger.dev（无超时限制）来做任务调度。API 端发事件、Worker 端消费事件，Inngest 还自带重试——Worker 崩溃了任务自动重新启动。

但说实话，大部分 Agent 产品在早期阶段不需要这个——先用单进程把产品跑起来，等真的遇到并发瓶颈再升级。过早引入消息队列和 Worker 分离只会增加运维复杂度，而你这个阶段最该把精力花在 Agent 本身的质量上。

如果你用 Vercel 且任务不超过 13 分钟，也可以看看 Fluid Compute——允许更长的函数执行时间，不需要自己管容器。

不管选哪种方案，有一个 Agent 特有的资源消耗模式你需要了解——突发型。Agent 大部分时间在等 LLM API 返回（空闲、几乎不消耗 CPU），工具执行的时候突然飙高（比如跑测试、编译项目），执行完又回到空闲。

这种突发型的场景，模式跟传统 Web 应用完全不同，如果你根据峰值来选择部署容器，那你肯定会花不少冤枉钱。所以不要按峰值来配置资源——用支持自动扩缩容的平台，或者干脆用按量付费的容器服务，避免为空闲时间买单。

顺带说一句成本：Agent 的大头开支不在服务器，在 API 调用。 一个 Railway 容器每月 $5-20，但 Agent 如果每天处理 100 个任务、每个任务调 30 次 LLM API，API 费用可能是每月 $500-2000——是服务器费用的 50-100 倍。所以别在服务器选型上纠结太久，把精力花在前面讲过的 Prompt Cache、模型分层、上下文压缩这些真正省钱的地方。

OK，读到这里，你应该明白了知道什么时候要升级、在升级的时候应该找哪些部署渠道。这里放一张图回顾一下：

崩溃了怎么办：状态持久化

不管你选哪种架构，都要面对一个问题：Agent 跑到一半进程挂了怎么办？

网络断了、服务器重启了、内存溢出了——这些在生产环境里都会发生。如果 Agent 跑了 30 分钟、做了 20 次工具调用，中途挂了要从头再来，体验很糟糕。

Claude Code 的做法值得参考——它用 JSONL 格式的 append-only 日志来记录整个对话过程。每一条消息（用户输入、模型回复、工具调用、工具结果）都作为单独的一行 JSON 追加到文件末尾。

这个设计有两个好处。第一，写入是原子的——追加一行文本到文件末尾是操作系统层面的原子操作，即使进程在写入过程中崩溃，最多丢失最后一行，前面的记录都是完整的。第二，恢复很简单——--resume 从文件里反序列化整个对话历史，Agent 从中断的地方继续工作，之前的工具调用结果还在上下文里。

如果你自己做 Agent 服务，类似的思路是：

Postgres 做主存储——所有数据的最终归宿。对话记录、工具调用日志、任务状态、成本统计，全部写进 Postgres。用 JSONB 列存 Agent 的事件数据（灵活、不用频繁改表结构），加上时间戳索引做查询。崩溃恢复的时候从 Postgres 读出最新状态就行。你还可以方便地跑 SQL 查"上个月哪些任务最费钱"、"哪个工具失败率最高"——这些数据分析需求只有关系型数据库能舒服地做。

比如你可以接入 Supabase 这种基于 Postgres 的服务，配合 Agent 来读表信息、生成 SQL 语句，现在数据分析成本已经非常低了。

Redis 做缓存层——不是必须的，但在生产环境里确实能解决几个实际问题。主要缓存这些东西：

活跃会话的上下文：Agent 每轮对话都要读完整的消息历史来构建上下文，如果每次都从 Postgres 查，频繁的 IO 会拖慢响应。把当前活跃的会话缓存在 Redis 里，读取是毫秒级的。
用户级的速率限制和 token 计数：每个 API 请求都要检查"这个用户今天还剩多少额度"，这种高频读写操作放 Postgres 会给数据库带来不必要的压力。
分布式锁：多个 Worker 处理同一个用户的请求时，需要防止并发冲突——比如两个请求同时修改同一个会话的状态。Redis 的 SETNX 天然适合做轻量级的分布式锁。

简单来说，Postgres 负责"不能丢"的数据，Redis 负责"要读快"的数据。如果你的并发量不大、只有单个 Worker，Redis 可以先不上——等真的遇到 Postgres 查询变慢了再加也不迟。

核心原则是：在执行高风险操作之前，先把当前状态持久化。Agent 要执行一个可能失败的工具调用之前，先把"我打算调用 X 工具、参数是 Y"写入日志。这样即使工具调用过程中崩溃了，恢复后能看到"上次打算做什么"，决定是重试还是跳过。

这里还有一个细节——工具调用最好是幂等的。同样的输入执行两次，结果应该一样。比如"创建文件 A 内容为 X"是幂等的（执行两次结果一样），但类似"在文件 A 末尾追加一行"的行为不是幂等的（执行两次会追加两行）。崩溃恢复时你可能需要重新执行上一次的工具调用，如果工具不幂等，重新执行就会产生"副作用"。所以，设计工具的时候尽量往幂等的方向靠。

讲了不少，这里收一下。崩溃恢复这件事，核心就三条：

Postgres 存所有数据（对话、工具调用、状态）
Redis 缓存热数据（活跃会话、速率限制、分布式锁）
工具设计尽量幂等（崩溃重跑不出问题）。

做到这三点，你的 Agent 服务在生产环境里挂了也能优雅恢复，不至于让用户从头再来。

沙箱：Agent 执行代码需要隔离

Agent 会执行代码——跑测试、安装依赖、编译项目。在生产环境里让 Agent 直接在你的服务器上跑命令，就像让一个你不完全信任的人坐在你的电脑前随便操作——万一模型决定跑一条 rm -rf / 或者往外部发送敏感数据，整个服务器都可能被搞坏。

所以需要沙箱隔离——给 Agent 一个受限的环境，让它只能做你允许的事情。

你可能第一反应是"用 Docker 隔离"。理论上可以，但实际的 Agent 产品几乎没人这么干——Docker 和宿主机共享内核，隔离强度不够；而且你要自己处理容器生命周期、资源限制、网络策略，工程量不小。真正在生产环境跑的 Agent 沙箱，主流就两种方案：云端 Agent 用 microVM 服务（如 E2B），本地 Agent 用 OS 级权限控制。

云端 Agent：E2B 微虚拟机

E2B 底层用的是 Firecracker（AWS Lambda 同款技术），给每个 Agent 会话分配一个轻量虚拟机——有自己独立的操作系统内核，不是 Docker 那种共享内核的”隔板间“，而是一个真正的独立房间。启动只要 125ms，内存开销约 5MB。

Manus 用的就是 E2B。实际用起来非常简单——装个 SDK，三行代码就能创建一个沙箱、执行命令、拿到结果：

typescript
复制
import Sandbox from "@e2b/code-interpreter"

// 创建一个沙箱（125ms 启动）
const sandbox = await Sandbox.create()

// 在沙箱里执行代码
const result = await sandbox.runCode("print('Hello from sandbox!')")
console.log(result.text) // "Hello from sandbox!"

// 也可以跑 shell 命令
const proc = await sandbox.commands.run("ls -la /home/user")
console.log(proc.stdout)

// 用完关掉
await sandbox.kill()


在 Agent 的工具调用里集成也很直观：

typescript
复制
const tools = {
  execute_code: async ({ code, language }) => {
    const sandbox = await Sandbox.create()
    try {
      const result = await sandbox.runCode(code, { language })
      return {
        stdout: result.text,
        stderr: result.error,
        exitCode: result.exitCode,
      }
    } finally {
      await sandbox.kill()
    }
  },

  run_tests: async ({ testCommand }) => {
    const sandbox = await Sandbox.create()
    // 先把项目文件写进沙箱
    await sandbox.files.write("/home/user/app/index.ts", projectCode)
    await sandbox.files.write("/home/user/app/test.ts", testCode)
    // 装依赖、跑测试
    await sandbox.commands.run("cd /home/user/app && npm install")
    const result = await sandbox.commands.run(
      `cd /home/user/app && ${testCommand}`
    )
    await sandbox.kill()
    return { output: result.stdout, errors: result.stderr }
  },
}


Agent 在沙箱里怎么折腾都不影响主系统——装了恶意包、删了系统文件、死循环吃满 CPU，都被隔离在虚拟机里。沙箱一关，什么都没了。

E2B 的 Hobby 层免费，注册送一次性 $100 额度（按秒计费，默认 2 vCPU 大约能跑 50 多小时），早期产品完全够用了。

本地 Agent：OS 级权限控制

如果你做的是跑在用户电脑上的本地工具（类似 Claude Code、Cursor 这类），没必要起容器或虚拟机——直接用操作系统自带的安全机制限制 Agent 能做什么。

Claude Code 就是这个思路：文件系统默认只读（只有工作目录可写），除非你加白名单或者加上 --dangerously-skip-permissions 命令行参数。操作系统强制执行这些规则，Agent 想违反也做不到。好处是几乎零开销，Agent 直接在本机跑，只是权限被限制了。

Anthropic 的数据显示，上线沙箱后权限弹窗减少了 84%——大部分正常操作在规则内自动放行，只有真正越界的操作才会被拦住。

怎么选？

一句话：云端 Agent 用 E2B，本地 Agent 用 OS 权限控制。 Docker 理论上能用，但隔离不够强、工程量又大，不推荐作为 Agent 沙箱方案。

定时 Agent：到点自己干

到目前为止讲的都是"用户触发、Agent 响应"。但还有一类场景：Agent 不需要人触发，到了时间自己开始干活。

比如每天早上自动检查代码仓库有没有过期的依赖，每周五生成项目进度报告，每次有新 PR 自动跑代码审查。

这跟传统的 cron 定时脚本有什么区别？区别很大。cron 脚本是你预定义好每一步操作——"检查 package.json 的过时依赖，如果有就发飞书消息"。定时 Agent 是你给它一个目标——"检查代码库的安全状况"——它自己决定读哪些文件、用什么工具、怎么分析。同一个定时任务，上次它可能重点看了依赖漏洞，这次可能发现了一个 SQL 注入风险。

重点在于目标驱动而非步骤驱动，这是 Agent 自动化跟传统自动化的本质区别。

落地的话，现成的方案有 Claude Code 的 Routines（支持定时、HTTP 调用、GitHub webhook 三种触发方式，跑在 Anthropic 的云上）。自己搭也不难，核心就是一个 cron 调度器（实战课会逐步实现）加上结果推送——Agent 跑完了主动把结果推出去，比如发给飞书群。

下一篇我们进入这一章的最后一个技术专题——Agent 的协议标准化。前面讲的 Permission Sync、流式推送、工具调用这些机制，每个产品都自己发明一套。ACP（Agent Client Protocol）和 A2A（Agent2Agent Protocol）试图把它们统一起来——一个解决"客户端怎么接入你的 Agent"，一个解决"Agent 之间怎么协作"。

参考资料
Vercel Agentic Infrastructure: https://vercel.com/blog/agentic-infrastructure
Inngest (事件驱动后台任务): https://www.inngest.com/
Trigger.dev (无超时后台任务): https://trigger.dev/
E2B (AI Agent 云沙箱): https://e2b.dev/
Claude Code Sandboxing: https://www.anthropic.com/engineering/claude-code-sandboxing
Claude Code Routines: https://code.claude.com/docs/en/web-scheduled-tasks
Railway: https://railway.app/
Fly.io: https://fly.io/
Anthropic: Building Effective Agents: https://www.anthropic.com/research/building-effective-agents
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
Hook 与可观测性：怎么知道你的 Agent 在干什么
下一篇 · 第六章：Agent 与 Harness 进阶 —— 编排、观测与部署
ACP：标准化 Agent 的控制接口


---
## 代码块


```typescript
// 一个普通的 Express 服务 + SSE
app.post("/api/agent", async (req, res) => {
  res.setHeader("Content-Type", "text/event-stream")
  res.setHeader("Cache-Control", "no-cache")

  const stream = await runAgent(req.body.prompt)
  for await (const chunk of stream) {
    res.write(`data: ${JSON.stringify(chunk)}\n\n`)
  }
  res.end()
})
```


```typescript
import Sandbox from "@e2b/code-interpreter"

// 创建一个沙箱（125ms 启动）
const sandbox = await Sandbox.create()

// 在沙箱里执行代码
const result = await sandbox.runCode("print('Hello from sandbox!')")
console.log(result.text) // "Hello from sandbox!"

// 也可以跑 shell 命令
const proc = await sandbox.commands.run("ls -la /home/user")
console.log(proc.stdout)

// 用完关掉
await sandbox.kill()
```


```typescript
const tools = {
  execute_code: async ({ code, language }) => {
    const sandbox = await Sandbox.create()
    try {
      const result = await sandbox.runCode(code, { language })
      return {
        stdout: result.text,
        stderr: result.error,
        exitCode: result.exitCode,
      }
    } finally {
      await sandbox.kill()
    }
  },

  run_tests: async ({ testCommand }) => {
    const sandbox = await Sandbox.create()
    // 先把项目文件写进沙箱
    await sandbox.files.write("/home/user/app/index.ts", projectCode)
    await sandbox.files.write("/home/user/app/test.ts", testCode)
    // 装依赖、跑测试
    await sandbox.commands.run("cd /home/user/app && npm install")
    const result = await sandbox.commands.run(
      `cd /home/user/app && ${testCommand}`
    )
    await sandbox.kill()
    return { output: result.stdout, errors: result.stderr }
  },
}
```
