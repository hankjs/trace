# 解剖 AI CLI：TUI 与 Headless 的两副面孔

课程
Agent OS 个人生产系统实战
解剖 AI CLI：TUI 与 Headless 的两副面孔
解剖 AI CLI：TUI 与 Headless 的两副面孔

约 15 分钟

下载本节完整代码

项目骨架搭好了，现在得研究我们要指挥的主角。

你的第一直觉可能是这样：Claude Code 是个命令行程序，那我用 child_process 把它 spawn 起来，往 stdin 写问题，从 stdout 读回答，搞定。

真去试你会发现完全走不通。交互模式下的 Claude Code 是画给人眼看的：输出里塞满了 ANSI 转义码，比如控制光标跳来跳去、给文字上色、每秒重绘那个转圈的 spinner。

你的程序从 stdout 读到的是一锅颜色代码和光标指令，是一堆混乱的字符，从这些字符要可靠地分辨出「它说了什么」「它说完了没」，难度超乎想象。

不过这条硬啃终端界面的路线确实存在，我们到偏后面的章节再来啃一遍这块硬骨头。但现在有一条更容易的一条路让大家上手，即使用 Headless 模式，这节我们就来讲讲怎么来集成。

第二副面孔

Claude Code 藏着一个比较安静的模式。在终端里直接跑：

bash
复制
claude -p "1加1等于几？只回答数字本身"


输出：

text
复制
2


没有界面、没有颜色、没有 spinner。进去一个问题，出来一个答案，进程退出。-p 是 --print 的缩写，官方叫它 headless 模式——无头，跑完就走。

这个输出对程序来说已经能用了，但信息太少：中间调了什么工具、花了多少钱、会话 ID 是什么，全都看不到。加两个参数，可以把完整的事件流打开：

bash
复制
claude -p "1加1等于几？只回答数字本身" --output-format stream-json --verbose


这次 stdout 吐出来的是一行一个 JSON 事件。我们逐个拆开看。

init：开场自报家门

第一行是 system 事件，subtype 为 init（原始输出很长，这里截取关键字段）：

json
复制
{
  "type": "system",
  "subtype": "init",
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05",
  "model": "claude-opus-4-6",
  "tools": ["Task", "Bash", "Edit", "Read", "Write", "WebFetch", "..."],
  "cwd": "/your/working/dir",
  "permissionMode": "default"
}


别小看这一行，信息量很大。session_id 是这次会话的身份证，后面续接对话全靠它，看到就要存下来。tools 数组值得多看一眼——Bash、Edit、Write、WebFetch……这说明 headless 模式下跑的还是那个满配的 Agent，会读文件、改代码、执行命令，只是把 TUI 界面关了。

assistant：模型开口
json
复制
{
  "type": "assistant",
  "message": {
    "role": "assistant",
    "content": [{ "type": "text", "text": "2" }]
  },
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05"
}


模型每产出一条消息就有一个 assistant 事件，content 里是内容块。这个例子只有文本块；等它干活的时候，这里会出现更有意思的东西，马上就能看到。

result：结束信号加账单
json
复制
{
  "type": "result",
  "subtype": "success",
  "is_error": false,
  "result": "2",
  "num_turns": 1,
  "duration_ms": 5056,
  "total_cost_usd": 0.241,
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05"
}


result 事件只在最后出现一次。它同时回答了三个问题：答案是什么（result 字段，最终回复的完整文本）、结束了没有（这个事件本身就是结束信号）、花了多少钱（total_cost_usd，精确到这一次调用）。

第三点先记下——等系统接进飞书，每个任务花了多少钱都能算得清清楚楚，这在后面做成本卡片时直接就能用。

让它干点真正的活

刚才的例子没调工具。我们换个需要动手的问题：

bash
复制
claude -p "当前目录下有哪些文件？数一下有几个" --output-format stream-json --verbose


事件流里多了新面孔。assistant 事件的内容块出现了 tool_use 类型：

json
复制
{
  "type": "assistant",
  "message": {
    "content": [
      {
        "type": "tool_use",
        "name": "Bash",
        "input": {
          "command": "ls -la",
          "description": "List files in current directory"
        }
      }
    ]
  }
}


模型决定跑一条 ls -la。紧接着有一个 type 为 user 的事件，装着命令的执行结果（工具结果在对话协议里以用户消息的身份回传给模型）。然后又一个 assistant 文本事件给出结论，最后 result 收尾。

把这串事件排成时间线，你看到的就是 Agent 的运行过程：什么时候开始、调了什么工具、说了什么、花了多久、多少钱。将来飞书卡片上会渲染这些信息，而数据源就是这条事件流。

记忆复原的钥匙：resume

headless 进程跑完就退出，那多轮对话怎么办？

还记得每个事件都带着的 session_id 吗？它就是我们恢复上下文的钥匙，尝试输入下面的命令

bash
复制
claude -p --resume [填入session_id] "再加1呢？只回答数字本身"


输出：

text
复制
3


上一轮问的 1+1，这一轮它记得结果是 2。新起的进程、同一个会话——Claude Code 把每个会话的完整记录存在本地，--resume 按 ID 把它捞出来接着聊。事件流里的 session_id 保持不变。

飞书话题里的每一句追问，带着存好的 session_id 去 resume，上下文就续上了。

Codex 的 Headless 模式

同一套思路，Codex 也有：

bash
复制
codex exec --json "1加1等于几？只回答数字本身"


Codex 这个命令会要求你跑在 git 仓库下面，如果跑不起来，可以加上 --skip-git-repo-check 参数解决。

json
复制
{"type":"thread.started","thread_id":"019f6e42-0a63-7863-a5f0-ef005cd952fc"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"2"}}
{"type":"turn.completed","usage":{"input_tokens":19162,"cached_input_tokens":9984,"output_tokens":5,"reasoning_output_tokens":0}}


结构对得上，不过参数名字全变了，我们梳理一下，其实都能对应上，该有的信息都有了：

概念	Claude Code	Codex
会话标识	session_id（init 事件里）	thread_id（thread.started 里）
模型说话	assistant 事件	item.completed + agent_message
执行命令	tool_use 内容块	item.completed + command_execution
结束信号	result 事件	turn.completed 事件
成本信息	total_cost_usd 直接给钱	只给 token 用量，钱要自己算
续接会话	--resume <id> 参数	exec resume <id> 子命令

续接会话这一块值得注意一下，Codex 的续接用子命令而非参数——

bash
复制
codex exec resume 019f6e42-0a63-7863-a5f0-ef005cd952fc --json "再加1呢？只回答数字本身"


回答 3，thread_id 不变。

动手做：三十行事件解析器

光看懂还不算数，我们写个工具消化一下。这个解析器从 stdin 读事件行，打出带时间戳的工作时间线：

src/probe-cli.ts
复制
import { createInterface } from "node:readline";

const t0 = Date.now();
const stamp = () => `[${((Date.now() - t0) / 1000).toFixed(1)}s]`;

const rl = createInterface({ input: process.stdin });

rl.on("line", (line) => {
  let ev: any;
  try {
    ev = JSON.parse(line);
  } catch {
    return; // 非 JSON 行（日志噪音）直接跳过
  }

  switch (ev.type) {
    // ── Claude Code ──
    case "system":
      if (ev.subtype === "init")
        console.log(
          `${stamp()} 会话开始 session_id=${ev.session_id} model=${ev.model}`,
        );
      break;
    case "assistant":
      for (const block of ev.message?.content ?? []) {
        if (block.type === "text" && block.text)
          console.log(`${stamp()} 模型说: ${block.text}`);
        if (block.type === "tool_use")
          console.log(`${stamp()} 调用工具: ${block.name}`);
      }
      break;
    case "result":
      console.log(
        `${stamp()} 完成 turns=${ev.num_turns} 耗时=${ev.duration_ms}ms 成本=$${ev.total_cost_usd}`,
      );
      console.log(`${stamp()} 最终回答: ${ev.result}`);
      break;

    // ── Codex ──
    case "thread.started":
      console.log(`${stamp()} 会话开始 thread_id=${ev.thread_id}`);
      break;
    case "item.completed":
      if (ev.item?.type === "agent_message")
        console.log(`${stamp()} 模型说: ${ev.item.text}`);
      if (ev.item?.type === "command_execution")
        console.log(`${stamp()} 执行命令: ${ev.item.command}`);
      break;
    case "turn.completed":
      console.log(`${stamp()} 完成 tokens=${JSON.stringify(ev.usage)}`);
      break;
  }
});


有一个细节值得注意：解析失败的行直接跳过。CLI 的 stdout 里偶尔会混进非 JSON 的日志（Codex 网络重连时就会打），所以要记得做好防御性编程，兜住这些异常情况。

package.json 的 scripts 里加一行：

json
复制
"probe:cli": "tsx src/probe-cli.ts"


管道接上，跑：

bash
复制
claude -p "当前目录下有哪些文件？数一下有几个" --output-format stream-json --verbose | pnpm probe:cli


输出（在我机器上）：

text
复制
[3.1s] 会话开始 session_id=aac1046d-3724-4240-bc70-a3dc7987aa1d model=claude-opus-4-6[1m]
[9.7s] 调用工具: Bash
[19.1s] 模型说: 当前目录下共有 **11 个文件/文件夹**（不算 `.` 和 `..`）：

| # | 名称 | 类型 |
|---|------|------|
| 1 | `.env` | 文件 |
| 2 | `.env.example` | 文件 |
| 3 | `.git` | 目录 |
| 4 | `.gitignore` | 文件 |
| 5 | `CLAUDE.md` | 文件 |
| 6 | `node_modules` | 目录 |
| 7 | `package.json` | 文件 |
| 8 | `pnpm-lock.yaml` | 文件 |
| 9 | `pnpm-workspace.yaml` | 文件 |
| 10 | `src` | 目录 |
| 11 | `tsconfig.json` | 文件 |

其中 3 个目录（`.git`、`node_modules`、`src`），8 个普通文件。
[19.2s] 完成 turns=2 耗时=16112ms 成本=$0.149039


一条命令，Agent 的完整工作过程变成了可读的时间线。Codex 那边同样好使：

bash
复制
codex exec --json --skip-git-repo-check "1加1等于几？" < /dev/null | pnpm probe:cli


顺手把 CLAUDE.md 的模块地图补一行 src/probe-cli.ts — 事件流解析器——上一篇立的规矩，模块长出来就登记。

路线定调

现在可以把技术选型说透了。headless 模式给了我们四样东西：

结构化事件（不用解析 TUI 字符）
确定的结束信号（result / turn.completed）
精确的成本数据
可续接的会话

基本需要用到的数据源现在都有了。

下一篇预告

CLI 这边的门道摸清了，我们接下来该把飞书接上了。下一篇我们去飞书开放平台注册机器人，用 WebSocket 长连接把消息收进来，不需要公网 IP，家里的电脑直接跑。到那篇结束，你在群里 @机器人 发送消息，你的程序就能收到了。

参考资料
Claude Code headless 模式官方文档
Codex CLI 非交互模式官方文档
Codex CLI GitHub 仓库
上一篇
一个人，一个 Agent 团队——你的操作系统从这里开始
下一篇 · 第二章：飞书接入
机器人上线：长连接收到第一条消息


---
## 代码块


```bash
claude -p "1加1等于几？只回答数字本身"
```


```bash
claude -p "1加1等于几？只回答数字本身" --output-format stream-json --verbose
```


```json
{
  "type": "system",
  "subtype": "init",
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05",
  "model": "claude-opus-4-6",
  "tools": ["Task", "Bash", "Edit", "Read", "Write", "WebFetch", "..."],
  "cwd": "/your/working/dir",
  "permissionMode": "default"
}
```


```json
{
  "type": "assistant",
  "message": {
    "role": "assistant",
    "content": [{ "type": "text", "text": "2" }]
  },
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05"
}
```


```json
{
  "type": "result",
  "subtype": "success",
  "is_error": false,
  "result": "2",
  "num_turns": 1,
  "duration_ms": 5056,
  "total_cost_usd": 0.241,
  "session_id": "df09e61a-5d5c-4768-9675-795309c8bf05"
}
```


```bash
claude -p "当前目录下有哪些文件？数一下有几个" --output-format stream-json --verbose
```


```json
{
  "type": "assistant",
  "message": {
    "content": [
      {
        "type": "tool_use",
        "name": "Bash",
        "input": {
          "command": "ls -la",
          "description": "List files in current directory"
        }
      }
    ]
  }
}
```


```bash
claude -p --resume [填入session_id] "再加1呢？只回答数字本身"
```


```bash
codex exec --json "1加1等于几？只回答数字本身"
```


```json
{"type":"thread.started","thread_id":"019f6e42-0a63-7863-a5f0-ef005cd952fc"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"2"}}
{"type":"turn.completed","usage":{"input_tokens":19162,"cached_input_tokens":9984,"output_tokens":5,"reasoning_output_tokens":0}}
```


```bash
codex exec resume 019f6e42-0a63-7863-a5f0-ef005cd952fc --json "再加1呢？只回答数字本身"
```


```typescript
import { createInterface } from "node:readline";

const t0 = Date.now();
const stamp = () => `[${((Date.now() - t0) / 1000).toFixed(1)}s]`;

const rl = createInterface({ input: process.stdin });

rl.on("line", (line) => {
  let ev: any;
  try {
    ev = JSON.parse(line);
  } catch {
    return; // 非 JSON 行（日志噪音）直接跳过
  }

  switch (ev.type) {
    // ── Claude Code ──
    case "system":
      if (ev.subtype === "init")
        console.log(
          `${stamp()} 会话开始 session_id=${ev.session_id} model=${ev.model}`,
        );
      break;
    case "assistant":
      for (const block of ev.message?.content ?? []) {
        if (block.type === "text" && block.text)
          console.log(`${stamp()} 模型说: ${block.text}`);
        if (block.type === "tool_use")
          console.log(`${stamp()} 调用工具: ${block.name}`);
      }
      break;
    case "result":
      console.log(
        `${stamp()} 完成 turns=${ev.num_turns} 耗时=${ev.duration_ms}ms 成本=$${ev.total_cost_usd}`,
      );
      console.log(`${stamp()} 最终回答: ${ev.result}`);
      break;

    // ── Codex ──
    case "thread.started":
      console.log(`${stamp()} 会话开始 thread_id=${ev.thread_id}`);
      break;
    case "item.completed":
      if (ev.item?.type === "agent_message")
        console.log(`${stamp()} 模型说: ${ev.item.text}`);
      if (ev.item?.type === "command_execution")
        console.log(`${stamp()} 执行命令: ${ev.item.command}`);
      break;
    case "turn.completed":
      console.log(`${stamp()} 完成 tokens=${JSON.stringify(ev.usage)}`);
      break;
  }
});
```


```json
"probe:cli": "tsx src/probe-cli.ts"
```


```bash
claude -p "当前目录下有哪些文件？数一下有几个" --output-format stream-json --verbose | pnpm probe:cli
```


```text
[3.1s] 会话开始 session_id=aac1046d-3724-4240-bc70-a3dc7987aa1d model=claude-opus-4-6[1m]
[9.7s] 调用工具: Bash
[19.1s] 模型说: 当前目录下共有 **11 个文件/文件夹**（不算 `.` 和 `..`）：

| # | 名称 | 类型 |
|---|------|------|
| 1 | `.env` | 文件 |
| 2 | `.env.example` | 文件 |
| 3 | `.git` | 目录 |
| 4 | `.gitignore` | 文件 |
| 5 | `CLAUDE.md` | 文件 |
| 6 | `node_modules` | 目录 |
| 7 | `package.json` | 文件 |
| 8 | `pnpm-lock.yaml` | 文件 |
| 9 | `pnpm-workspace.yaml` | 文件 |
| 10 | `src` | 目录 |
| 11 | `tsconfig.json` | 文件 |

其中 3 个目录（`.git`、`node_modules`、`src`），8 个普通文件。
[19.2s] 完成 turns=2 耗时=16112ms 成本=$0.149039
```


```bash
codex exec --json --skip-git-repo-check "1加1等于几？" < /dev/null | pnpm probe:cli
```
