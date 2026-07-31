# 一个人，一个 Agent 团队——你的操作系统从这里开始

课程
Agent OS 个人生产系统实战
一个人，一个 Agent 团队——你的操作系统从这里开始
一个人，一个 Agent 团队——你的操作系统从这里开始

约 20 分钟

下载本节完整代码

模型越来越强，我们每天使用它的方式却没有发生根本变化。打开终端，敲下 claude 或 Codex，聊完关掉；任务一多，再在几个窗口之间复制上下文、追问进度、转交结果。Agent 已经能干很多活，人依然守在电脑前充当调度员。

如果目标只停在“把 AI 编程 CLI 接进飞书”，最后得到的只是一个更方便的聊天机器人。它能在手机上回复消息，体验会好一些，想象力也就到这里了。

这门课要做得更深一层：给分散的模型、CLI 和自动化流程装上一套能够长期工作的操作系统。 我们叫它 Agent OS。

Claude Code 和 Codex 是执行引擎，飞书是随时可用的操作界面，定时任务等自动化流程是系统主动工作的入口。中间的指挥层负责管理任务、会话、Agent、权限和调度。底层引擎以后可以更换，上层的工作方式会一直留下来。

这一篇是整门课的起点。动手之前，我们先把 Agent OS 的两类入口与统一内核聊清楚。这个问题决定了我们接下来是在做一个小工具，还是在搭一套真正能进入日常工作的个人生产系统。

一套 Agent OS 应该长什么样
飞书：随时可以开工的工作空间

飞书是 Agent OS 的第一块操作界面。它天然适合移动端，也已经进入了大多数人的办公场景。你不用专门打开一个新应用，看到消息就能派活，随时随地都能处理一次关键确认。

更重要的是，飞书已经准备好了远比聊天框丰富的交互能力。话题群天然适合组织任务：一个话题承载一个目标，讨论、文件、图片和执行结果都留在同一条线上。飞书卡片可以放按钮、选项和输入框，让审批与需求确认从“来回打字”变成一次点击。

飞书文档也会成为这套系统的内容空间。需求讨论可以沉淀成 Spec，阶段结果可以整理成报告，Agent 之间交接时传递一份稳定的文档，不需要把几十轮聊天记录塞给下一个角色。

这些能力组合起来，飞书就从消息入口变成了一个真正的工作空间。你在里面发起任务、补充上下文、查看进度，也在需要拍板时完成决策。

比如，我在群里只说了一句「@产品经理 加个查看用户详情的功能」。Agent 先检查现有代码，确认基础实现已经存在，再把真正影响产品行为的分歧整理成 5 个问题。

每个问题都给出可选方案、推荐项和自定义输入；我也可以随时按推荐方案继续。已经确认的答案会被保留，Agent 不会反复追问。

Agent 负责读代码、识别未知信息、收敛选项，人只在真正影响产品方向的地方做决定。确认完成后，答案会继续进入 Spec 和后续执行流程，成为稳定的任务上下文。

自动化工作流：让系统在你没有说话时继续运转

飞书解决了人怎么使用系统，Agent OS 还需要接住那些每天重复发生、没人主动发消息的工作。

我自己运营 Sitor 时就有一个很典型的场景：服务端需要持续做健康检测。发现异常以后，系统去拉取线上日志，整理出真正相关的错误，再交给 Agent 定位问题、修改代码、运行验证并准备发布。低风险步骤可以自动向前推进，涉及上线、数据变更等高风险操作时，再把结论和影响范围送到飞书，请我来拍板。

系统会完成能安全完成的部分，带着问题、证据、修复结果和待确认动作来找你。没有异常时，它会保持安静，不会打扰你。

这套结构也能迁移到其他业务。比如内容、运营、其他研发场景等等，需要替换的是业务规则和工具，任务触发、Agent 执行、结果验证、人类拍板这条主线完完全全可以继续复用。

课程后半程会用我实际运营 Sitor 的经验，把这条自动化链路完整跑一遍。我们会看到任务怎样被触发、怎样交给 Agent、怎样验证结果，以及哪些地方值得自动化、哪些地方必须把决定留给人。

两类入口共享一个系统内核

飞书负责接住人的目标、补充信息与关键决策，打造人和 Agent 交互的入口，自动化工作流负责在没人发消息时持续触发和推进任务。两类入口共享同一套任务、会话与 Agent 状态，才能形成一个完整闭环。

这也是“OS”这个名字真正的含义。它统一管理人工派活、主动触发、任务状态、执行资源和人的注意力，把底层模型与工具的复杂度收进系统内部。你只需要提出目标、做关键判断、验收结果，剩下的工作可以稳定地向前推动。

超级个体的两种工作模式

今年超级个体、一人公司（OPC） 之类的概念非常火热，背后指代的趋势是一个人 + 一堆 Agent 可以干很多事情了，成为了一种新型的组织形态。

那「一个人 + 一个 Agent 团队」到底怎么工作？拆开看主要有两种模式。

第一种：单兵作战。 你直接指挥一个 Agent 干活，它做你看，你说它改。这个模式今天人人都在用——打开终端，敲 claude或者打开 Codex，直接开聊。我们理论课的绝大多数篇幅，都在加强你这部分的认知，让你少踩坑、Agent 少返工。

而我想说的是，单兵作战有个容易被忽视的体验问题：入口的便捷度。

Agent 本身的能力，大家用的都是同一个模型；真正拉开差距的，是你能在多少个时刻使唤得动它。终端把你锁在电脑前，合上笔记本，你的 AI 员工就下班了，说实话，这还不够方便。

飞书让这个入口一直待在你的口袋里。移动端、PC 端随时在线，你不再需要守着终端等待一次任务结束。Agent 需要补充信息或等待审批时，系统再把你叫回来。

第二种：团队作战。 任务多了以后，你不可能每件事都亲自盯。这时候需要的是 Agent 之间互相协作：一个 Agent 干完活 @另一个 Agent 审查；巡检 Agent 发现问题，直接派活给修复 Agent。你只跟「助理」对话，团队内部的流转自动发生。

比如 Claude Code 完成开发后，我们希望自动把任务交给 Codex 独立审查；Codex 给出意见后，再把反馈交回开发 bot 继续处理。整次协作留在同一个飞书话题中，并通过轮次上限避免 Agent 无限对话。

这里的人不需要逐条复制代码和审查意见，只需要提出最初的目标，并在流程结束后验收结果。课程后面会从两台 bot 的首次交接开始，逐步实现这条跨模型互审链路，通过我们的 Agent OS 去控制 Bot 协作过程中的上下文管理和协作轮次。

而且类似的自动化流程还有不少，课程里面会逐步分享出来。

单兵作战解决「随时随地指挥一个 Agent」，团队作战解决「一队 Agent 自己转起来」。这门课两种模式都会做出来：前半程把单兵作战的完整链路搭好，后半程让团队转起来。

Agent OS 要解的四个问题

管理一个 Agent 团队，你会撞上四个绕不开的问题，每一个都对应课程的一条主线。

第一，记忆。 Agent 的上下文是容易丢的：会话关了就忘，进程重启就丢。你上午在话题里讨论清楚的方案，下午追问时它必须还记得。所以会话要持久化、CLI 会话要能续接、项目的关键决策要沉淀成文件——会话内核那一章和 CLI 的 resume 机制负责这条线。今天动手写的 CLAUDE.md，也是其中的一部分。

第二，验证。 幻觉没法消除，只能工程化衰减。Agent 干的活不能只靠它自己说「做完了」，需要独立的验证层。理论课把这件事的原理讲透了；实战课我们把它做出来：跨模型互审那一篇会让 Claude 干活、Codex 审查。

第三，权限。 哪些操作可以全自动，哪些必须人拍板？读日志可以随便读，但重启服务、动数据库就必须有人拍板才行。审批那一篇会把三级权限做出来——而飞书恰好是完美的拍板媒介：@真人，弹卡片，审批按钮，体验还是很不错的。

第四，任务的管理与分发。 任务从哪来、派给谁、状态如何、历史在哪查。飞书里能派活，定时器也能自动派活，这些入口怎样共用一套任务状态？还有，服务端半夜报错，怎么让 Agent 自动领走修掉？能解决这些管理问题，这才算真正的系统。课程中的会话管理、团队调度和主动式 Agent 章节负责这条线。

记忆、验证、权限、分发——四个问题各自都不难懂，但组合起来才算是 OS。这也是为什么聊天机器人教程遍地都是，能日用的 Agent 生产系统却很少见：大部分只是去零散地用工具，但并不能解决 OS 本身的核心问题，并串联起自动化的工作流程。

系统内核把两类入口串起来

飞书消息和定时巡检都会进入同一个任务内核。任务内核找到对应的 Agent、工作目录和会话，再调起 Claude Code 或 Codex 执行。过程状态持续更新到飞书话题与任务卡片，真正需要人关注的结果再触发通知或审批。

把这条链路完整摊开，一句话从飞书出发到本机 CLI 再回到飞书，中间要经过四道处理：

这张图里的每一层，后面都有专门的篇目把它做出来：接入层在接入飞书那一章，会话与工作目录在会话内核那一章，headless 命令和事件翻译在 CLI 引擎那几章，审批卡片留到案例实战那一章。现在只需要记住这条主线的架构就可以。

入口可以不同，底层始终在管理同一批任务。 这一点很重要。否则人工派出的任务有一套状态，自动化任务又有另一套状态，出了问题还要四处追踪，系统很快就会失控。

里面有两个设计决策现在需要说清楚。

一是接入方式走 WebSocket 长连接。飞书支持机器人主动连到开放平台收事件，你家里的电脑就能跑，没有公网 IP、没有域名、没有 webhook 配置。这一点对个人开发者极其友好。

二是 CLI 跑 headless 模式。Claude Code 除了你熟悉的终端交互界面，还有一种打开模式：claude -p 加上 --output-format stream-json，进来一个 prompt，出去一串结构化 JSON 事件。Codex 同样有 codex exec --json。我们的 daemon 模块指挥的就是这种模式。下一篇会把它彻底拆开看。

整条路线是这样走的：

先打通飞书收发，再建会话内核，接入 Claude Code 和 Codex，到这里单兵作战模式就完整了；
接着让两个 bot 学会互相 @ 协作，再装上定时巡检和审批门，团队作战模式初步成型；
再把定时巡检、审批和工程化交付接进同一个任务内核。每一篇结束，你手里都是一个能跑的系统，只是能力会逐篇变强。而且每一节都会附上这节所有的代码，方便你逐步对照学习。

还有一个心态上的转变，也是理论课强调过的：用 Agent 干活，你的角色就从执行者变成了管理者。你真正要练的是派活、验收、拍板这三件事，具体的命令和 API 跟着课敲一遍自然就会。理论课开篇讲的就是这个道理，实战课我们继续在实践中加强。

动手：搭项目骨架

说了这么多，写代码。这篇的产出就是一个规范的项目骨架加一个环境验证。按照下面的命令搭起来脚手架：

脚手架
bash
复制
mkdir agent-os && cd agent-os
pnpm init
pnpm add dotenv zod
pnpm add -D typescript tsx @types/node


简单解释一下：

dotenv：飞书凭证这类敏感信息放 .env 文件，一般不会写进代码。

zod：后面解析飞书消息体和 CLI 事件流时做运行时校验。

tsx：直接运行 TypeScript，改完即跑，不用编译。

你可能注意到了，这里没有装任何「AI SDK」，因为这门课不调模型 API。我们指挥的是完整的 AI 编程 CLI，模型调用、工具执行、上下文管理都是 Claude Code 和 Codex 自己的事。我们造的是上面的指挥层。

修改 package.json，加上模块类型和启动脚本：

package.json
复制
{
  "name": "agent-os",
  "type": "module",
  "scripts": {
    "dev": "pnpm start",
    "start": "tsx watch --include .env src/index.ts",
    "start:once": "tsx src/index.ts",
    "build": "tsc"
  },
  "engines": { "node": ">=22" }
}


type: "module" 统一走 ESM——飞书 SDK 和现代生态都是 ESM，就不用 CJS 了。engines 锁到 Node 22 以上，因为我们会用到较新的 fetch 和 readline 特性。

这里让 pnpm start 直接进入 watch 模式。命令保持运行时，修改 src 里的代码或 .env 配置都会自动重启程序，不需要每次手动按 Ctrl+C。如果只想运行一次，可以改用 pnpm start:once。

tsconfig.json：

tsconfig.json
复制
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}


再加上 .gitignore：

.gitignore
复制
node_modules/
dist/
.env
data/
*.log


.env 必须在第一行代码写出来之前就进 ignore。data/ 是将来会话持久化的目录，同样进 ignore。

.env.example（提交这个，.env 永远只在你自己的机器上）：

.env.example
复制
# 飞书自建应用凭证（接入飞书那一篇手把手教你创建）
BOT_A_APP_ID=cli_xxx
BOT_A_APP_SECRET=xxx

入口与环境自检

创建 src/index.ts。现在用它来检查环境是不是正常的：

src/index.ts
复制
import "dotenv/config";
import { execSync } from "node:child_process";
import { existsSync } from "node:fs";

const VERSION = "0.1.0";

function hasCommand(cmd: string): boolean {
  try {
    execSync(`command -v ${cmd}`, { stdio: "ignore", shell: "/bin/sh" });
    return true;
  } catch {
    return false;
  }
}

function check(label: string, ok: boolean, hint: string): void {
  console.log(`  ${ok ? "✅" : "⚠️ "} ${label}${ok ? "" : `  → ${hint}`}`);
}

console.log(`\nAgent OS v${VERSION} — 一个人，一队 Agent\n`);
console.log("环境自检：");

const nodeMajor = Number(process.versions.node.split(".")[0]);
check(`Node.js ${process.versions.node}`, nodeMajor >= 22, "需要 Node 22+");
check(
  ".env 配置文件",
  existsSync(".env"),
  "复制 .env.example 为 .env 并填入飞书凭证",
);
check(
  "Claude Code CLI",
  hasCommand("claude"),
  "接入 CLI 前需要安装；无 Anthropic 订阅可使用 DeepSeek",
);
check("Codex CLI", hasCommand("codex"), "后续接入 Codex 前再安装");

console.log("\n骨架就绪。下一步：解剖 AI CLI 的两副面孔。\n");


hasCommand 用 command -v 探测命令是否存在，比 which 更符合 POSIX 标准，跨 shell 表现稳定。

跑一下：

bash
复制
pnpm start


输出：

text
复制
Agent OS v0.1.0 — 一个人，一队 Agent

环境自检：
  ✅ Node.js 22.22.3
  ⚠️  .env 配置文件  → 复制 .env.example 为 .env 并填入飞书凭证
  ✅ Claude Code CLI
  ✅ Codex CLI

骨架就绪。下一步：解剖 AI CLI 的两副面孔。


.env 有 warning 是正常的，凭证要到接入飞书那一篇才会配。Claude Code 和 Codex 在这里也只做提示，不会阻塞骨架启动。进入 CLI 引擎章节前需要安装 Claude Code；没有 Anthropic 订阅时，可以按课程里的 DeepSeek 官方接入方式继续完成真实链路，后续我会教大家怎么方便地切换模型。

给项目写 CLAUDE.md

还差一个文件 CLAUDE.md，也就是 AI 进入项目时自动读取的说明书。前面说的四个问题里「记忆」这条线，最直接的起点就是它。

我们在理论课里面已经把 CLAUDE.md 的注意事项都介绍过了，这里不再赘述。我们先只写三类东西：这是什么项目、怎么跑起来、有哪些预设的工程约定。

claude.md
复制
# agent-os

把飞书变成 AI 编程 CLI（Claude Code / Codex）的指挥台。
一个话题 = 一个任务；bot 之间可互相 @ 协作；cron 定时巡检。

## 运行

pnpm dev（tsx watch）/ pnpm start / pnpm build

## 约定

- ESM only，Node 22+，pnpm
- 凭证只放 .env（已 gitignore），绝不硬编码、绝不提交

## 错题本

> 踩坑后追加一行：现象 → 原因 → 正确做法。给未来的 AI 和人看。

下一篇预告

骨架有了，但还没碰真正的主角。你平时用的 Claude Code 是那个丰富的终端交互界面，它还有另一种用法：claude -p --output-format stream-json——一个只吐结构化 JSON 的 headless 进程。Codex 也一样，只是命令参数不一样。

下一篇我们把这两个 CLI 拆开：headless 的事件流长什么样、session-id 藏在哪、断掉的对话怎么续上。看懂这些，你就明白整门课的技术路线为什么这么选。

参考资料
飞书开放平台
Claude Code 官方文档
Codex CLI 官方文档
下一篇 · 第一章：起步
解剖 AI CLI：TUI 与 Headless 的两副面孔


---
## 代码块


```bash
mkdir agent-os && cd agent-os
pnpm init
pnpm add dotenv zod
pnpm add -D typescript tsx @types/node
```


```json
{
  "name": "agent-os",
  "type": "module",
  "scripts": {
    "dev": "pnpm start",
    "start": "tsx watch --include .env src/index.ts",
    "start:once": "tsx src/index.ts",
    "build": "tsc"
  },
  "engines": { "node": ">=22" }
}
```


```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```


```text
node_modules/
dist/
.env
data/
*.log
```


```text
# 飞书自建应用凭证（接入飞书那一篇手把手教你创建）
BOT_A_APP_ID=cli_xxx
BOT_A_APP_SECRET=xxx
```


```typescript
import "dotenv/config";
import { execSync } from "node:child_process";
import { existsSync } from "node:fs";

const VERSION = "0.1.0";

function hasCommand(cmd: string): boolean {
  try {
    execSync(`command -v ${cmd}`, { stdio: "ignore", shell: "/bin/sh" });
    return true;
  } catch {
    return false;
  }
}

function check(label: string, ok: boolean, hint: string): void {
  console.log(`  ${ok ? "✅" : "⚠️ "} ${label}${ok ? "" : `  → ${hint}`}`);
}

console.log(`\nAgent OS v${VERSION} — 一个人，一队 Agent\n`);
console.log("环境自检：");

const nodeMajor = Number(process.versions.node.split(".")[0]);
check(`Node.js ${process.versions.node}`, nodeMajor >= 22, "需要 Node 22+");
check(
  ".env 配置文件",
  existsSync(".env"),
  "复制 .env.example 为 .env 并填入飞书凭证",
);
check(
  "Claude Code CLI",
  hasCommand("claude"),
  "接入 CLI 前需要安装；无 Anthropic 订阅可使用 DeepSeek",
);
check("Codex CLI", hasCommand("codex"), "后续接入 Codex 前再安装");

console.log("\n骨架就绪。下一步：解剖 AI CLI 的两副面孔。\n");
```


```bash
pnpm start
```


```text
Agent OS v0.1.0 — 一个人，一队 Agent

环境自检：
  ✅ Node.js 22.22.3
  ⚠️  .env 配置文件  → 复制 .env.example 为 .env 并填入飞书凭证
  ✅ Claude Code CLI
  ✅ Codex CLI

骨架就绪。下一步：解剖 AI CLI 的两副面孔。
```


```markdown
# agent-os

把飞书变成 AI 编程 CLI（Claude Code / Codex）的指挥台。
一个话题 = 一个任务；bot 之间可互相 @ 协作；cron 定时巡检。

## 运行

pnpm dev（tsx watch）/ pnpm start / pnpm build

## 约定

- ESM only，Node 22+，pnpm
- 凭证只放 .env（已 gitignore），绝不硬编码、绝不提交

## 错题本

> 踩坑后追加一行：现象 → 原因 → 正确做法。给未来的 AI 和人看。
```
