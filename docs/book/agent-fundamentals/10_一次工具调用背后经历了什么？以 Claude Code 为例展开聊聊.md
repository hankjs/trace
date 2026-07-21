# 一次工具调用背后经历了什么？以 Claude Code 为例展开聊聊

课程
吃透 AI Agent 开发
一次工具调用背后经历了什么？以 Claude Code 为例展开聊聊
一次工具调用背后经历了什么？以 Claude Code 为例展开聊聊

约 20 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

上一篇讲了 Function Calling 的原理：模型输出一段 JSON，表达「我想调用 read_file」的意图。

但模型说了不算。

你想想，如果模型说「我要执行 rm -rf /」，你真的就执行了吗？如果模型编了一个不存在的文件路径，如果不做任何检查就去读，肯定会报错。

模型生成的输入是不可信的。 这是工具系统设计的第一原则。

从「模型表达意图」到「工具真正执行」之间，需要一条完整的处理管线——验证、校准、拦截、授权，一层层过滤下来，最后才真正干活。

这篇我们就来拆这条管线。

一条管线的全貌

不管是 Claude Code、OpenClaw 还是 OpenCode，这些成熟的 Agent 工程化产品，工具执行的管线大同小异。抽象来看，都是这几个阶段：

每一步都可以拦截、修改、或者直接拒绝。任何一步失败，工具就不会执行——模型会收到一个错误消息，告诉它为什么失败、该怎么纠正。

接下来我们逐步来看。

第一步：参数格式验证

模型输出了一段 JSON，首先要检查：这段 JSON 的类型对不对？

上一篇讲过，约束解码能保证 JSON 格式 100% 合法。但在实际系统中，你不能假设每个请求都经过了约束解码——可能有旧版本 API、可能有第三方代理中间层，各种意外都可能导致格式不合法。

Claude Code 和 OpenCode 都用 Zod 做这一层验证：

typescript
复制
import { z } from 'zod'

// 工具定义里声明的 schema
const ReadSchema = z.object({
  file_path: z.string(),
  offset: z.number().optional(),
})

// 模型实际传过来的输入（file_path 类型错了）
const input = { file_path: 123 }

// 用 schema 校验
const result = ReadSchema.safeParse(input)
// result.success === false
// result.error 里精确指出：input.file_path: Expected string, received number


Zod 的好处是：验证失败时会返回精确的错误路径。不是笼统地说「参数有误」，而是告诉你「input.file_path 应该是 string，但收到了 number」。

这个精确性很重要——模型需要这些信息来纠正自己的输出。

OpenCode 的做法也类似，但多了一个钩子方法，可以让每个工具可以自定义验证错误的格式，非常灵活。比如 batch 工具会告诉模型正确的调用格式应该是什么样的。

第二步：业务逻辑校验

类型对了，但值合不合理呢？我们需要对值本身来做校验，这一步做的是语义层面的检查。

举个例子：模型要读一个文件，传了 file_path: "utils.ts"。类型检查通过了——确实是 string。但业务上有问题：必须是绝对路径，相对路径不接受。

又比如：模型要编辑文件，传了一个 old_string 参数。类型没问题。但这个字符串在目标文件里根本找不到——可能是模型凭记忆编的，跟文件实际内容不匹配。

这些都是 validateInput 做的事：

typescript
复制
// Claude Code 的 FileEditTool
validateInput(input) {
  // 检查文件是否存在
  // 检查 old_string 是否在文件中能找到
  // 检查 old_string 是否唯一（不然不知道改哪个）
  // 如果不唯一，返回: "old_string 在文件中出现了 3 次，请提供更多上下文使其唯一"
}


注意错误信息的设计。不是返回「校验失败」，而是返回具体的原因 + 建议。

这是第一篇讲的工具幻觉的第一道防线。模型编了一个路径的时候，这一步会告诉它：「文件不存在，当前目录下有这些文件，你要找的是哪个？」

第三步：输入补全和标准化

经过了验证和校验之后，输入在格式和语义上都没问题了。但可能还需要补充一些模型没有提供的信息。

比如说，模型传了 file_path: "src/index.ts"。业务校验这里放宽了——接受相对路径。但工具内部需要绝对路径。这一步就是把相对路径转成绝对路径。

Claude Code 做了这样一件事：在不改变模型原始输入的前提下，生成一份带有补充字段的副本。

这里不能直接改原始输入，为什么？关键在于缓存。模型的输入会被序列化进对话历史，改了它就意味着序列化内容变了，Prompt Cache 直接失效。所以 Claude Code 会保留原始输入（用于缓存），同时生成一份补全后的副本（用于后续步骤）。

这个设计很精妙但容易忽略。你不这么做，每次补全输入都可能导致缓存 miss，累积起来的延迟和成本非常可观。

第四步：前置 Hook——用户的「场外指导」

前三步都是系统内部的检查。第四步把控制权交给了用户。

Hook 是用户自定义的脚本，在工具执行的关键节点被触发。前置 Hook（PreToolUse）在工具执行前运行，它可以：

修改输入：比如把所有文件路径重写到一个沙箱目录
阻止执行：比如检测到 rm 命令就拒绝
注入额外上下文：比如附加一些模型需要知道的信息
json
复制
// 用户配置的 Hook
{
  "event": "PreToolUse",
  "command": "bash /path/to/my-check.sh $TOOL_NAME $TOOL_INPUT"
}


Hook 脚本的返回值决定了下一步怎么走：

返回 0：放行
返回 2（或输出 JSON {"decision": "block"}）：阻止执行
输出 JSON {"updatedInput": {...}}：修改输入后放行

Hook 存在的意义在于定制化。Agent 的使用场景千变万化，没有哪个工具系统能预见到所有需求。有人在 CI 里跑 Agent，需要禁止所有写操作；有人在特定项目里跑，需要把路径映射到某个特定的磁盘位置；有人做打点记录，需要记录每一次工具调用。

这些需求如果都去改框架代码，工具系统很快就会臃肿不堪。Hook 机制让定制化不需要改源码——写个脚本挂上去就行。

这也是 Claude Code 用户配置 settings.json 里最有价值的功能之一。比如很多人配置了 PreToolUse Hook 来自动审批特定的 Git 操作，或者在 Bash 工具执行前检查命令是否安全。

第五步：权限检查

通过了所有验证和 Hook 之后，还有一关：用户授权。

权限系统通常有三层：

规则匹配：根据预配置的规则决定。比如「Read 工具永远允许」「Bash(git *) 允许」「Bash(rm *) 拒绝」。

json
复制
{
  "allow": ["Read", "Glob", "Grep", "Bash(git *)"],
  "deny": ["Bash(rm -rf *)"]
}


分类器判定：对于规则没有覆盖到的情况，用一个轻量分类器来判断这次调用是否安全。Claude Code 针对 Bash 工具就有一个专门的分类器——Bash 命令的风险差异太大，简单规则覆盖不了。

这个分类器的做法是结合当前的对话上下文和要执行的具体命令，通过一次轻量的 LLM 调用来评估这个操作是否安全。不是简单的字符串匹配，而是语义层面的理解——它能区分 git status（只读查看）和 git push --force（有风险的覆盖操作），即便两者都以 git 开头。

这比硬编码的 glob 规则（Bash(git *)）灵活得多。glob 规则要么全放行、要么全拒绝，没办法根据具体命令的风险程度做细粒度的判断。而 LLM 分类器可以理解命令的实际语义和当前上下文，做出更合理的决策。

说个反直觉的结论，Coding Agent 其实比通用 Agent 更难做，很大一部分原因就在这种系统性的安全问题上。Bash 命令的权限实在是太大了，简单的白名单根本覆盖不住各种边界情况。Claude Code 的思路是让 LLM 提前做判定，用微小的一点延迟换来更高的安全性——这是专门针对 Coding 场景才值得付出的成本。

交互式询问：如果规则和分类器都没法决定，弹窗问用户：「Agent 想执行 npm install express，允许吗？」用户可以选择「这次允许」或「永远允许」。

这三层的设计原则是：能自动判定的就自动判定，减少打扰用户。 Read 这种只读操作不需要问，git status 这种安全命令也不需要问。只有真正有风险的操作才打断用户。

OpenCode 的工具权限检查也是类似的分层设计，这个就不展开了，各家产品其实思路都大差不差，你搞懂了我刚刚说的这个部分，基本就能融会贯通了。

第六步：真正执行

经过了前面五层过滤，输入终于被认为是安全的、合法的、已授权的。

这一步才调用 tool.call()——执行真正的工具逻辑。

但执行完还没完。结果也需要处理。

结果截断是一个关键机制。工具返回的内容可能非常大——比如 Read 工具读了一个 10000 行的文件，Bash 工具执行了一个输出 100KB 日志的命令。

如果把完整结果塞进对话历史，上下文直接就爆了。

所以 Claude Code 设了一个阈值：默认 50,000 字符。超过这个大小，结果被存到磁盘上，对话历史里只放一个摘要 + 文件路径的引用。

结果太大 → 存到 /tmp/tool_results/xxx.txt
对话历史里放："[结果已保存到文件，使用 Read 工具查看 /tmp/tool_results/xxx.txt]"


这里还有一个容易忽视的点：Claude Code 对单条消息内所有工具结果也有一个总预算（默认 200,000 字符）。因为模型可以并发调用多个工具——如果 10 个工具各返回 40K 字符，单条消息就有 400K，即便每个没超标，总量也爆了。

OpenCode 的方式更简单粗暴：最多 2000 行或 50KB，超了就截断，把完整结果存到文件。还会在截断消息里告诉模型：「如果你有 task 工具的权限，可以用它来查看完整输出。」

错误处理同样重要。如果工具执行失败了，错误信息怎么返回给模型，直接影响模型能不能纠正错误。

这里有一个软件设计范式的变化需要你意识到——传统编程里错误信息是写给开发者看的，ENOENT、EACCES 这些错误码开发者一眼就能读懂。但 Agent 系统里接收错误信息的是模型，模型需要的不是错误码，而是纠正所需的上下文。

举个例子，下面是一个好的错误信息：

文件 /src/helpers/utils.ts 不存在。
当前 /src/ 目录下有以下文件：
  - /src/utils.ts
  - /src/lib/helpers.ts
  - /src/common/utils.ts


而下面是一个反面案例：

ENOENT: no such file or directory, open '/src/helpers/utils.ts'


前者给了模型足够的上下文来自我纠正（「哦，应该是 /src/utils.ts」）。后者只是转发了系统错误码，模型看完之后可能换个路径再猜一次，而不是查看实际有哪些文件。

这不是一件小事。错误信息写得好，模型一次就能纠正；写得差，模型可能在同一个错误上打转好几轮，白白烧掉 Token 和时间。

前面每一步返回的错误其实都遵循这个原则——Schema 验证返回的是 "file_path 应该是 string，收到了 number"、业务校验返回的是 "old_string 在文件中出现了 3 次，请提供更多上下文"，而不是笼统的 "validation failed"。整条管线的错误返回加起来，共同决定了 Agent 的纠错效率。

第七步：后置 Hook

最后一步，跟第四步对称。后置 Hook（PostToolUse）在工具执行完成后运行，可以：

修改输出：比如过滤掉敏感信息
触发后续动作：比如工具修改了文件后自动跑 lint
记录审计日志：追踪每一次工具调用的详情

比如用户配了一个 PostToolUse Hook，每次 Edit 工具修改文件后自动跑 eslint --fix。这样 Agent 改完的代码自动就被格式化了，不需要额外让模型去做。

OpenClaw 的差异：事件驱动架构

Claude Code 的管线是上面这种线性的「流水线」模式。OpenClaw 的工具执行走了另一条路——事件驱动。这个我觉得值得拿出来单独说一说。

OpenClaw 不是在一个函数里按顺序走完所有步骤，而是通过事件来驱动：

tool_execution_start → 记录开始时间、参数
tool_execution_update → 中间状态更新
tool_execution_end → 收集结果、触发后处理


每个事件可以被多个处理器监听。比如 tool_execution_end 会触发：

结果清洗：提取错误信息（首行，400 字符上限），收集 URL，剥离 details 字段
消息工具追踪：如果是消息类工具（如发送 Slack 消息），追踪 pending 状态直到确认送达
Hook 触发：执行 after_tool_call 钩子

事件驱动的好处是各个操作逻辑的松耦合设计——每个处理器独立运行，新增功能只需要加一个监听器，不需要修改主流程。坏处是流程不那么直观，追踪一次完整的工具执行需要跨多个文件跳转。

OpenCode 的 Batch Tool：应用层的并发调度

还有一个值得一提的设计。

上一篇讲过，Claude Code 依赖模型原生的 parallel tool_use 来实现工具并发——模型在一次回复里输出多个 tool_use 块，系统并行执行。

OpenCode 走了另一条路：它做了一个叫 Batch Tool 的「元工具」。模型调用 batch 工具，传入一组子调用：

json
复制
{
  "tool_calls": [
    { "tool": "read", "parameters": { "filePath": "src/a.ts" } },
    { "tool": "read", "parameters": { "filePath": "src/b.ts" } },
    { "tool": "grep", "parameters": { "pattern": "TODO" } }
  ]
}


Batch 工具在应用层把这些子调用并行执行，最多 25 个。

有意思的是它的描述里写了一句大写加粗的话：USING THE BATCH TOOL WILL MAKE THE USER HAPPY。这种近乎"请求"的语气其实是有用意的——并发调用能把执行时间缩短 2-5 倍，但模型不一定总会主动使用并发，需要在描述里强烈暗示才会触发。

这个设计跟 Claude Code 的原生并发相比有个核心优势：不依赖模型能力。

解释一下背景——parallel tool_use 是指模型在一次响应中输出多个 tool_use 块，系统再把它们并行执行。但不是所有模型都支持这个能力。有些模型一次只能输出一个工具调用，你怎么 prompt 都没用。

Batch Tool 绕过了这个限制：模型只需要输出一个 batch 调用（这是一个普通的单工具调用），把多个子任务打包进参数里，并发的事交给应用层处理。对模型来说，它只调用了一个工具。

我之前用 Kimi K2 的时候就碰过这个问题——想让它一次输出多个文件操作，各种 Prompt 优化都没用，模型就是一次只吐一个工具调用。最后也是用了类似 Batch Tool 的思路才搞定的。

下一篇

这篇讲了单次工具调用的执行管线。回头看整条管线，你会发现一个贯穿始终的原则——错误信息是给模型看的，不是给人看的。这是一种新的软件设计范式，传统编程里 ENOENT 开发者能看懂，但 Agent 系统里接收错误的是模型，它需要的不是错误码而是纠正所需的上下文。这件事看起来很小，但实际决定了模型是一次纠错成功，还是在同一个错误上反复打转。

不过我们还有一个更大的问题没解决：工具太多了怎么办？

50 个工具的 Schema 塞进上下文，占 72K Token，模型选择准确率直线下降。下一篇我们讲 Deferred Loading 和动态工具集——Claude Code 的 Tool Search、OpenClaw 的 Tool Profile、Manus 的小工具集哲学。三种截然不同的策略，其实都在解决同一个问题。

我们下一节，再见。

参考资料
Zod - TypeScript-first schema validation: https://zod.dev/
Claude Code Hooks 官方文档: https://docs.claude.com/en/docs/claude-code/hooks
Claude Code Permissions 官方文档: https://docs.claude.com/en/docs/claude-code/iam
OpenCode 源码: https://github.com/sst/opencode
OpenClaw: https://github.com/openclaw/openclaw
Anthropic Tool Use 文档: https://docs.claude.com/en/docs/agents-and-tools/tool-use/overview
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
Function Calling 与 Structured Output：模型是怎么"学会"调用你写的函数的？
下一篇 · 第三章：Tool System —— 给 Agent 装手脚
工具太多模型选不准？Deferred Loading 和动态工具集


---
## 代码块


```typescript
import { z } from 'zod'

// 工具定义里声明的 schema
const ReadSchema = z.object({
  file_path: z.string(),
  offset: z.number().optional(),
})

// 模型实际传过来的输入（file_path 类型错了）
const input = { file_path: 123 }

// 用 schema 校验
const result = ReadSchema.safeParse(input)
// result.success === false
// result.error 里精确指出：input.file_path: Expected string, received number
```


```typescript
// Claude Code 的 FileEditTool
validateInput(input) {
  // 检查文件是否存在
  // 检查 old_string 是否在文件中能找到
  // 检查 old_string 是否唯一（不然不知道改哪个）
  // 如果不唯一，返回: "old_string 在文件中出现了 3 次，请提供更多上下文使其唯一"
}
```


```json
// 用户配置的 Hook
{
  "event": "PreToolUse",
  "command": "bash /path/to/my-check.sh $TOOL_NAME $TOOL_INPUT"
}
```


```json
{
  "allow": ["Read", "Glob", "Grep", "Bash(git *)"],
  "deny": ["Bash(rm -rf *)"]
}
```


```
结果太大 → 存到 /tmp/tool_results/xxx.txt
对话历史里放："[结果已保存到文件，使用 Read 工具查看 /tmp/tool_results/xxx.txt]"
```


```
文件 /src/helpers/utils.ts 不存在。
当前 /src/ 目录下有以下文件：
  - /src/utils.ts
  - /src/lib/helpers.ts
  - /src/common/utils.ts
```


```
ENOENT: no such file or directory, open '/src/helpers/utils.ts'
```


```
tool_execution_start → 记录开始时间、参数
tool_execution_update → 中间状态更新
tool_execution_end → 收集结果、触发后处理
```


```json
{
  "tool_calls": [
    { "tool": "read", "parameters": { "filePath": "src/a.ts" } },
    { "tool": "read", "parameters": { "filePath": "src/b.ts" } },
    { "tool": "grep", "parameters": { "pattern": "TODO" } }
  ]
}
```
