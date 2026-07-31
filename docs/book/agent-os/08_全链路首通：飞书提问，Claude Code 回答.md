# 全链路首通：飞书提问，Claude Code 回答

课程
Agent OS 个人生产系统实战
全链路首通：飞书提问，Claude Code 回答
全链路首通：飞书提问，Claude Code 回答

约 17 分钟

下载本节完整代码

前面的任务卡片会一点点走到 100%，看起来已经很像一个正在工作的 Agent。但那段进度是 mock 的，背后还没有真正的执行引擎。

这一轮，我们把 mock 的任务拿掉。

你会在飞书话题里交代一件事，Agent OS 启动真正的 Claude Code，把任务交给它执行，再把最终回答送回原来的话题。此前搭好的消息解析、任务卡片、会话状态和持久化，会在这里第一次连成完整闭环。

先进入项目，确认上一版可以正常编译：

bash
复制
pnpm install
pnpm build

准备真正的 Claude Code

这一节会直接运行 claude 命令。还没有安装的话，先执行：

bash
复制
npm install -g @anthropic-ai/claude-code
claude --version


只要第二条命令能打印版本号，Agent OS 就能从当前终端找到它。模型后端有两种接法，选一种即可。

如果你有 Anthropic 订阅，在终端运行一次 claude，按照提示完成登录。之后仍在同一个终端里启动 Agent OS，它会复用现有登录状态。

如果你没有 Anthropic 订阅，可以让 Claude Code 使用 DeepSeek API。模型后端属于 Claude Code 自己的配置，放进用户级的 ~/.claude/settings.json 后，终端里直接运行的 Claude Code 和 Agent OS 调起的子进程都会使用同一套配置。

先打开配置文件。文件已经存在时，保留里面原有的设置，把下面的 env 字段合并进去；不要直接覆盖整份文件：

bash
复制
mkdir -p ~/.claude
touch ~/.claude/settings.json


然后用你习惯的编辑器打开 ~/.claude/settings.json：

~/.claude/settings.json
复制
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
    "ANTHROPIC_AUTH_TOKEN": "替换为你的_DeepSeek_API_Key",
    "ANTHROPIC_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash",
    "CLAUDE_CODE_SUBAGENT_MODEL": "deepseek-v4-flash",
    "CLAUDE_CODE_EFFORT_LEVEL": "max"
  }
}


经常在 Anthropic、DeepSeek 和其他模型服务之间切换的话，也可以用 CC Switch 管理这些供应商配置，切换时点一下即可。CC Switch 是第三方开源工具，课程代码不依赖它。

保存后重新运行 claude，新进程会读取这份配置。使用 DeepSeek 时，确认 API Key 有效且账户有可用余额。~/.claude/settings.json 只留在你的电脑上，不要复制进项目或提交到仓库。

你自己的 .env 里也要有 CLAUDE_WORKDIR。留空时，Claude Code 在 Agent OS 的启动目录里工作；要让它处理另一个项目，就填写那个项目的绝对路径：

.env
复制
CLAUDE_WORKDIR=/你的/项目/绝对路径


Claude Code 会从这个目录读取代码和项目说明，也会在这里执行工具。工作目录选错了，模型即使正常回答，也是在理解另一个项目。

程序怎样指挥 Claude Code

Claude Code 提供了非交互调用方式。-p 接收任务并在完成后退出，--output-format stream-json 把执行过程变成一行一个 JSON 事件，--verbose 则提供完整事件信息：

bash
复制
claude -p "读取 package.json，告诉我项目名称" \
  --output-format stream-json \
  --verbose


先在终端手动运行这条命令。它应该真的读取当前项目，并陆续输出 system、assistant 和 result 事件。若这里无法完成，先处理安装、认证或 DeepSeek 配置，暂时不要进入飞书链路。

这一轮只关心闭环需要的两项数据：

result.result 是最终回答。
result.session_id 是 Claude Code 会话标识。

我们先把最终回答送回飞书，并在终端打印 session_id。保存它并恢复多轮上下文，是接下来要完成的能力。

把一次执行收进 runClaude()

新建 src/cli/claude-runner.ts。这个模块只负责一件事：给它提示词和工作目录，它返回最终回答与 session_id。

src/cli/claude-runner.ts
复制
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

export interface ClaudeRunResult {
  answer: string;
  sessionId?: string;
}

export interface RunClaudeOptions {
  prompt: string;
  cwd: string;
  signal?: AbortSignal;
}

interface ClaudeResultEvent {
  type: "result";
  is_error?: boolean;
  result?: string;
  session_id?: string;
}

function isResultEvent(value: unknown): value is ClaudeResultEvent {
  if (!value || typeof value !== "object") return false;
  return (value as { type?: unknown }).type === "result";
}

export function runClaude(options: RunClaudeOptions): Promise<ClaudeRunResult> {
  const args = [
    "-p",
    options.prompt,
    "--output-format",
    "stream-json",
    "--verbose",
  ];

  return new Promise((resolve, reject) => {
    const child = spawn("claude", args, {
      cwd: options.cwd,
      signal: options.signal,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const lines = createInterface({ input: child.stdout });
    let finalResult: ClaudeRunResult | undefined;
    let resultError: Error | undefined;
    let stderr = "";
    let settled = false;

    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    lines.on("line", (line) => {
      let event: unknown;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (!isResultEvent(event)) return;
      if (event.is_error) {
        resultError = new Error(event.result || "Claude Code 执行失败");
        return;
      }
      if (typeof event.result === "string") {
        finalResult = {
          answer: event.result,
          sessionId: event.session_id,
        };
      }
    });

    child.stderr.on("data", (chunk: Buffer | string) => {
      stderr += chunk.toString();
    });
    child.once("error", (error) => {
      if (options.signal?.aborted) {
        fail(new Error("Claude Code 执行已取消"));
        return;
      }
      fail(error);
    });
    child.once("close", (code) => {
      if (settled) return;
      if (options.signal?.aborted) {
        return fail(new Error("Claude Code 执行已取消"));
      }
      if (resultError) return fail(resultError);
      if (code !== 0) {
        return fail(
          new Error(stderr.trim() || `Claude Code 退出，状态码 ${code}`),
        );
      }
      if (!finalResult) {
        return fail(new Error("Claude Code 没有返回最终结果"));
      }
      settled = true;
      resolve(finalResult);
    });
  });
}


这里用 spawn() 直接传命令和参数数组，没有把用户消息拼进 shell 字符串。飞书里的引号、换行或 $() 只会成为普通参数，不会被 shell 当成另一条命令执行。

stdout 是字节流，一次 data 回调可能拿到半行，也可能同时拿到几行。readline 把它恢复成一条条完整事件，我们再逐行调用 JSON.parse()。日志噪音解析失败时直接跳过，真正的 result 事件才会更新 finalResult。

进程结束也分几种情况。Claude Code 明确返回错误时使用 result 里的信息；异常退出时优先带回 stderr；状态码正常却没有 result，说明协议没有完整走完，也要当成失败。

settled 防止 error 与 close 两个事件重复结束同一个 Promise。Node.js 在 AbortController.abort() 后会产生 AbortError，这里把它收敛成稳定的“Claude Code 执行已取消”，飞书入口不需要理解底层进程细节。

运行类型检查：

bash
复制
pnpm build

用真实执行替换进度演示

回到 src/index.ts。先调整导入，并读取 Claude Code 的工作目录：

src/index.ts
复制
import "dotenv/config";
import { join, resolve } from "node:path";
import { startBot } from "./im/lark.js";
import { buildTaskCard } from "./im/card.js";
import { resolveMentions, extractResourceKeys } from "./im/message-parser.js";
import { parseCommand } from "./core/command-parser.js";
import { SessionManager, type Session } from "./core/session-manager.js";
import { JsonSessionStore } from "./core/session-store.js";
import { runClaude } from "./cli/claude-runner.js";

const appId = process.env.BOT_A_APP_ID;
const appSecret = process.env.BOT_A_APP_SECRET;
const cliWorkdir = resolve(process.env.CLAUDE_WORKDIR ?? process.cwd());


原来的 Bot 类型和 ThrottledCardUpdater 已经用不到，可以从导入中删除。在凭证检查后面打印真正要执行的命令和工作目录：

src/index.ts
复制
console.log("Agent OS 启动，正在建立飞书长连接…");
console.log(`[CLI] command=claude cwd=${cliWorkdir}`);


删除旧的 wait()、DEMO_STEPS 和 runCardDemo()。那一整段只为演示进度服务，现在换成真实执行入口：

src/index.ts
复制
function executeCli(prompt: string, signal: AbortSignal) {
  return runClaude({
    prompt,
    cwd: cliWorkdir,
    signal,
  });
}


找到发送初始卡片的位置，把标题和当前进度改成真实执行语义：

src/index.ts
复制
cardId = await bot.replyCard(
  msg.messageId,
  buildTaskCard({
    title: "Claude Code 任务",
    status: "running",
    progress: 0,
    detail: "正在启动执行引擎",
  }),
  hasThread,
);


找到原来的 void runCardDemo(...)，用下面这段替换：

src/index.ts
复制
void executeCli(resolved, run.signal)
  .then(async (result) => {
    await bot.updateCard(
      cardId,
      buildTaskCard({
        title: "Claude Code 任务",
        status: "success",
        progress: 100,
        detail: "执行完成",
      }),
    );
    await bot.reply(msg.messageId, result.answer, hasThread);
    console.log(`[CLI] 完成 session_id=${result.sessionId ?? "(无)"}`);
  })
  .catch(async (error) => {
    if (run.signal.aborted) {
      console.log("[CLI] 任务已取消");
      return;
    }
    const message = (error as Error).message;
    console.error("[CLI] 执行失败:", message);
    await bot.updateCard(
      cardId,
      buildTaskCard({
        title: "Claude Code 任务",
        status: "failed",
        progress: 0,
        detail: message,
      }),
    );
    await bot.reply(
      msg.messageId,
      `Claude Code 执行失败：${message}`,
      hasThread,
    );
  })
  .finally(async () => {
    if (activeRuns.get(session.id) === run) activeRuns.delete(session.id);
    try {
      await markSessionIdle(session.id);
    } catch (error) {
      console.error("[会话] 保存空闲状态失败:", (error as Error).message);
    }
  })
  .catch((error) => {
    console.error("[任务] 回传或收尾失败:", (error as Error).message);
  });


消息回调不用原地等待 Claude Code。任务进入后台后，飞书长连接仍能接收 /status 和 /close。完成时先把卡片更新到 100%，再把 result.answer 回复到原消息所在的话题。

失败也要回到同一条链路。卡片会显示失败原因，话题里也会收到错误文本。最末尾再接一层 catch()，用于接住卡片更新或状态持久化自身的异常，避免后台出现 Unhandled Promise rejection。

从飞书完成真实验收

确认 package.json 的启动脚本仍然监听源码和 .env：

package.json
复制
"start": "tsx watch --include .env src/index.ts"


保存配置后，watch 模式会自动重启。现在由你启动服务：

bash
复制
pnpm start


终端应该显示真正的命令与工作目录：

text
复制
[CLI] command=claude cwd=/你的/项目/绝对路径


在飞书新开话题，给它一个容易核对的只读任务：

text
复制
@机器人 请读取 package.json，告诉我项目名称和主要依赖，不要修改文件


消息进入后，先出现“Claude Code 任务”卡片。执行完成时卡片变成绿色并到达 100%，随后真实回答会回到同一个话题。终端还会打印：

text
复制
[CLI] 完成 session_id=...


下面是这条链路的真实验收结果。问题从飞书发出，Claude Code 读取项目文件，回答回到同一个话题：

真实任务执行时间较长时，原来的 /close 仍然有效。已有的 AbortController 会通过 signal 传进 spawn()，子进程被终止，会话保持 closed，后台也不会再发送成功结果。

这条链路还没有记住对话

现在，你已经从飞书真正调动了一次 Claude Code。触发器是群里的消息，入口服务是 Agent OS，无头执行体是刚刚启动的 Claude Code 子进程。三部分第一次连成了一条可以工作的自动化链路。

不过，同一话题再发一句“继续”，Claude Code 还不知道上一轮说过什么。我们虽然拿到了 session_id，目前只把它打印在终端，没有保存进 Session，下一次执行也没有带上恢复参数。

接下来要处理的正是这段断开的记忆：把不同 CLI 收进统一的适配器，保存执行引擎会话标识，并让同一飞书话题里的追问继续原来的 Claude Code 对话。

全链路已经通了。下一次升级之后，它会开始真正记得你们聊过什么。让我们下一节继续！

参考资料
Claude Code：Run Claude Code programmatically
Claude Code：CLI reference
DeepSeek：接入 Claude Code
Claude Code：Settings
CC Switch：AI 编程 CLI 统一管理工具
Node.js：Child process
Node.js：Readline
上一篇
持久化与重启恢复
下一篇 · 第四章：接入 CLI 引擎
适配器模式 & 多轮对话支持


---
## 代码块


```bash
pnpm install
pnpm build
```


```bash
npm install -g @anthropic-ai/claude-code
claude --version
```


```bash
mkdir -p ~/.claude
touch ~/.claude/settings.json
```


```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
    "ANTHROPIC_AUTH_TOKEN": "替换为你的_DeepSeek_API_Key",
    "ANTHROPIC_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-pro[1m]",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash",
    "CLAUDE_CODE_SUBAGENT_MODEL": "deepseek-v4-flash",
    "CLAUDE_CODE_EFFORT_LEVEL": "max"
  }
}
```


```dotenv
CLAUDE_WORKDIR=/你的/项目/绝对路径
```


```bash
claude -p "读取 package.json，告诉我项目名称" \
  --output-format stream-json \
  --verbose
```


```typescript
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

export interface ClaudeRunResult {
  answer: string;
  sessionId?: string;
}

export interface RunClaudeOptions {
  prompt: string;
  cwd: string;
  signal?: AbortSignal;
}

interface ClaudeResultEvent {
  type: "result";
  is_error?: boolean;
  result?: string;
  session_id?: string;
}

function isResultEvent(value: unknown): value is ClaudeResultEvent {
  if (!value || typeof value !== "object") return false;
  return (value as { type?: unknown }).type === "result";
}

export function runClaude(options: RunClaudeOptions): Promise<ClaudeRunResult> {
  const args = [
    "-p",
    options.prompt,
    "--output-format",
    "stream-json",
    "--verbose",
  ];

  return new Promise((resolve, reject) => {
    const child = spawn("claude", args, {
      cwd: options.cwd,
      signal: options.signal,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const lines = createInterface({ input: child.stdout });
    let finalResult: ClaudeRunResult | undefined;
    let resultError: Error | undefined;
    let stderr = "";
    let settled = false;

    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    lines.on("line", (line) => {
      let event: unknown;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (!isResultEvent(event)) return;
      if (event.is_error) {
        resultError = new Error(event.result || "Claude Code 执行失败");
        return;
      }
      if (typeof event.result === "string") {
        finalResult = {
          answer: event.result,
          sessionId: event.session_id,
        };
      }
    });

    child.stderr.on("data", (chunk: Buffer | string) => {
      stderr += chunk.toString();
    });
    child.once("error", (error) => {
      if (options.signal?.aborted) {
        fail(new Error("Claude Code 执行已取消"));
        return;
      }
      fail(error);
    });
    child.once("close", (code) => {
      if (settled) return;
      if (options.signal?.aborted) {
        return fail(new Error("Claude Code 执行已取消"));
      }
      if (resultError) return fail(resultError);
      if (code !== 0) {
        return fail(
          new Error(stderr.trim() || `Claude Code 退出，状态码 ${code}`),
        );
      }
      if (!finalResult) {
        return fail(new Error("Claude Code 没有返回最终结果"));
      }
      settled = true;
      resolve(finalResult);
    });
  });
}
```


```bash
pnpm build
```


```typescript
import "dotenv/config";
import { join, resolve } from "node:path";
import { startBot } from "./im/lark.js";
import { buildTaskCard } from "./im/card.js";
import { resolveMentions, extractResourceKeys } from "./im/message-parser.js";
import { parseCommand } from "./core/command-parser.js";
import { SessionManager, type Session } from "./core/session-manager.js";
import { JsonSessionStore } from "./core/session-store.js";
import { runClaude } from "./cli/claude-runner.js";

const appId = process.env.BOT_A_APP_ID;
const appSecret = process.env.BOT_A_APP_SECRET;
const cliWorkdir = resolve(process.env.CLAUDE_WORKDIR ?? process.cwd());
```


```typescript
console.log("Agent OS 启动，正在建立飞书长连接…");
console.log(`[CLI] command=claude cwd=${cliWorkdir}`);
```


```typescript
function executeCli(prompt: string, signal: AbortSignal) {
  return runClaude({
    prompt,
    cwd: cliWorkdir,
    signal,
  });
}
```


```typescript
cardId = await bot.replyCard(
  msg.messageId,
  buildTaskCard({
    title: "Claude Code 任务",
    status: "running",
    progress: 0,
    detail: "正在启动执行引擎",
  }),
  hasThread,
);
```


```typescript
void executeCli(resolved, run.signal)
  .then(async (result) => {
    await bot.updateCard(
      cardId,
      buildTaskCard({
        title: "Claude Code 任务",
        status: "success",
        progress: 100,
        detail: "执行完成",
      }),
    );
    await bot.reply(msg.messageId, result.answer, hasThread);
    console.log(`[CLI] 完成 session_id=${result.sessionId ?? "(无)"}`);
  })
  .catch(async (error) => {
    if (run.signal.aborted) {
      console.log("[CLI] 任务已取消");
      return;
    }
    const message = (error as Error).message;
    console.error("[CLI] 执行失败:", message);
    await bot.updateCard(
      cardId,
      buildTaskCard({
        title: "Claude Code 任务",
        status: "failed",
        progress: 0,
        detail: message,
      }),
    );
    await bot.reply(
      msg.messageId,
      `Claude Code 执行失败：${message}`,
      hasThread,
    );
  })
  .finally(async () => {
    if (activeRuns.get(session.id) === run) activeRuns.delete(session.id);
    try {
      await markSessionIdle(session.id);
    } catch (error) {
      console.error("[会话] 保存空闲状态失败:", (error as Error).message);
    }
  })
  .catch((error) => {
    console.error("[任务] 回传或收尾失败:", (error as Error).message);
  });
```


```json
"start": "tsx watch --include .env src/index.ts"
```


```bash
pnpm start
```


```text
[CLI] command=claude cwd=/你的/项目/绝对路径
```


```text
@机器人 请读取 package.json，告诉我项目名称和主要依赖，不要修改文件
```


```text
[CLI] 完成 session_id=...
```
