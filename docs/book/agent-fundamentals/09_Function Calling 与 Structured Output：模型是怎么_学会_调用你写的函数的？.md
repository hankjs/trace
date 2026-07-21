# Function Calling 与 Structured Output：模型是怎么"学会"调用你写的函数的？

课程
吃透 AI Agent 开发
Function Calling 与 Structured Output：模型是怎么"学会"调用你写的函数的？
Function Calling 与 Structured Output：模型是怎么"学会"调用你写的函数的？

约 20 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

你在 Claude 或 ChatGPT 里输入「北京今天天气怎么样？」，模型会回一段 JSON：

json
复制
{
  "type": "tool_use",
  "name": "get_weather",
  "input": { "city": "北京" }
}


然后你的代码拿到这个 JSON，去调天气 API，把结果塞回给模型，模型再用自然语言回复用户。

看起来很自然，像模型真的「调用了一个函数」。

但如果你停下来想一秒：模型是怎么知道要输出这段 JSON 的？它怎么知道有个工具叫 get_weather？它怎么知道参数应该是 city 而不是 location？

这就是 Function Calling 的核心问题。搞清楚它，后面的工具系统设计才有根基。

拆掉魔法：Function Calling 的真实过程

先说结论：模型不会调用任何函数。

Function Calling 这个名字是有误导性的。实际发生的事情是：

你在 API 请求里塞了一份「工具菜单」——一组 JSON Schema，描述了每个工具叫什么、干什么、参数是什么格式
模型看到了这份菜单，结合用户的问题，决定要不要使用某个工具
如果要用，模型生成一段符合 Schema 的 JSON——这就是所谓的「函数调用」
你的代码解析这段 JSON，你来执行真正的函数
执行结果塞回对话，模型看到结果后继续回复

整个过程中，模型做的事只有一件：输出一段符合格式的 JSON。执行是你做的。

这个区分非常重要。如果你把 Function Calling 理解成「模型能调用函数」，你会低估参数校验的重要性——觉得模型既然「调用了」，参数总不会错吧？错。模型只是生成了一段 JSON，它可以把任何东西填进去。

约束解码：怎么保证输出的 JSON 合法？

那模型是怎么做到「输出的 JSON 一定符合 Schema」的？

主要靠两件事：训练 + 约束解码。

训练好理解——模型在训练阶段看过大量的「输入 Schema + 输出符合 Schema 的 JSON」的样本，学会了怎么根据 Schema 生成对应的 JSON。

但光靠训练不够。OpenAI 公布过一个数据：单靠训练，JSON Schema 符合率只有 93%。93% 在生产环境远远不够——100 次调用里有 7 次格式错误，Agent 跑 10 轮就大概率崩一次。

所以还需要约束解码（Constrained Decoding）。

回顾第 3 篇讲的自回归生成：模型每次只生成一个 Token，从词表里选概率最高的。约束解码做的事就是：在选 Token 之前，把不合法的选项排除掉。

具体来说：

把 JSON Schema 编译成一套语法规则（上下文无关文法）
每生成一个 Token，检查：接下来哪些 Token 在语法上合法
不合法的 Token 概率设为 0
剩下的合法 Token 重新归一化，正常采样

举个例子。Schema 要求 city 是 string 类型。当模型生成到 "city": 的时候，下一个 Token 只能是 "（字符串的开始引号）。数字、布尔值、null 的 Token 全被排除了。

训练 + 约束解码 = 100% 格式合法。

但要注意：约束解码只保证格式，不保证语义。

✅ 能保证：city 字段一定是 string 类型
❌ 不能保证：city 的值是真实存在的城市（模型可以填「哥谭市」）
❌ 不能保证：模型选对了工具（应该用 get_weather 却用了 search）

所以你仍然需要参数校验、执行前检查、以及清晰的错误反馈——这些是后面几篇会详细讲的。

工具越多越不准：一个被低估的问题

Function Calling 在少量工具的时候表现很好。但当工具数量上去之后，准确率会明显下降。

这不是直觉，是有实测数据的：

工具数量	选择准确率	Token 开销
4 个	~95%	~1,200
10 个	~90%	~3,000
30 个	~71%	~25,000
46 个（GitHub MCP）	~71%	~42,000
50+ 个	<50%	~72,000

为什么会这样？里面有三个退化机制在起作用：

第一：注意力稀释。 工具越多，每个工具的描述在上下文里占的比例越小。模型的注意力被分散了，就像你同时看 50 个菜单页，反而不知道点什么。

第二：语义碰撞。 当工具多了，难免有功能相似的。search_files 和 find_files 有什么区别？read_file 和 get_file_content 呢？当工程量起来之后，这种重复工具的工具很容易出现，模型容易搞混。

第三：预算挤压。 每个工具的 JSON Schema 都要塞进上下文。46 个 GitHub MCP 工具就吃掉 42,000 Token——还没开始对话，上下文已经被占了一大块。留给用户消息和对话历史的空间就少了。

这就是为什么 Claude Code 有 30 多个工具，但不是一上来就全部塞给模型。它有一套叫 Tool Search 的机制，把不常用的工具标记为「延迟加载」（defer_loading），只在模型需要的时候才让它看到。50+ 个工具从 72K Token 降到 500 Token，准确率从 49% 提到 74%。这个机制下一篇会详细讲。

模型会幻觉出不存在的参数

工具选对了，参数格式也对了。但还有一个坑：参数的值可能是假的。

这就是所谓的「工具幻觉」。模型会：

伪造文件路径：你让它读 /src/utils.ts，它可能输出 /src/helpers/utils.ts——这个路径不存在，是模型根据常见项目结构「猜」的
编造 ID：让它删除某条记录，它可能编一个看起来像 UUID 但完全不存在的 ID
猜测 URL：让它访问某个 API，它可能拼一个看起来合理但并不存在的端点

这些幻觉的麻烦在于：约束解码拦不住它们。 约束解码只管类型——路径是 string 就放行，ID 是 string 就放行，URL 是 string 就放行。至于这个 string 的值是真是假，完全不在它的检查范围内。所以 JSON Schema 验证一路绿灯，但真正执行的时候必然会失败。

要应对这个问题，你需要在 Schema 设计和执行链路上都做防御。

第一：用 enum 约束可选值

如果参数的合法值是有限的，用 enum 而不是裸的 string。

json
复制
{
  "action": {
    "type": "string",
    "enum": ["read", "write", "delete"]
  }
}


模型在约束解码下只能从这三个值里选，没法编造第四个。

第二：执行前校验

在真正调用函数之前，做一次业务级校验。比如文件路径：先检查文件是否存在；如果不存在，返回一个带有建议的错误信息。

第三：清晰的错误反馈

好的错误信息：

"文件 /src/helpers/utils.ts 不存在。当前目录下有 /src/utils.ts 和 /src/lib/helpers.ts，你要找的是哪个？"

坏的错误信息：

"ENOENT: no such file or directory"

前者给了模型足够的信息来纠正自己。后者让模型一脸懵——可能会换个路径再试，也可能换个完全不相关的策略。

实际上 Claude Code 的工具定义里就有专门的 validateInput 方法。在真正执行之前，先做业务级校验——文件路径必须是绝对路径、不能包含 ..、目标文件必须存在等等。通过了才执行，不通过就返回清晰的错误信息让模型自我纠正。

Structured Output：不只是 Function Calling

讲到这里你可能意识到了：Function Calling 本质上就是让模型输出一段符合特定格式的 JSON。

那如果我不需要调用函数，只是想让模型按固定格式输出呢？比如让模型打分：

json
复制
{
  "score": 8,
  "issues": ["变量命名不规范", "缺少错误处理"],
  "pass": true
}


这就是 Structured Output——跟 Function Calling 用的是同一套技术（约束解码），只是场景不同。

来看三个实际应用场景，帮你理解 Structured Output 在真实系统中怎么用。

场景一：上下文摘要

Agent 对话跑了几十轮，上下文快满了，需要压缩。你不能让模型自由发挥写摘要——万一漏掉了关键信息呢？

用 Structured Output 强制输出格式：

json
复制
{
  "summary": "用户要求重构 auth 模块，已完成 login/logout，待处理 token refresh",
  "key_decisions": ["使用 JWT 替代 session", "refresh token 存 httpOnly cookie"],
  "pending_tasks": ["实现 token refresh 端点", "添加 CSRF 防护"],
  "important_context": ["项目用 Next.js 14", "数据库是 PostgreSQL"]
}


强制模型按这个结构输出，每个字段都不能省。这样压缩后的摘要是结构化的，后续 Agent 可以精确地读取 pending_tasks 来继续工作，而不是从一段自然语言里「猜」还有什么没做完。

场景二：生成式 UI

这是 Structured Output 最酷的一个应用。让模型基于 JSON Schema 来描述 UI 组件：

json
复制
{
  "type": "card",
  "title": "天气预报",
  "children": [
    { "type": "text", "content": "北京 · 晴 · 25°C", "style": "heading" },
    { "type": "chart", "data": [22, 25, 28, 26, 24], "labels": ["周一", "周二", "周三", "周四", "周五"] },
    { "type": "button", "label": "查看详情", "action": "navigate:/weather/beijing" }
  ]
}


前端拿到这个 JSON，直接渲染成真实的 UI 组件。模型不用写 HTML/CSS，只需要按 Schema 描述「我想要什么」，渲染层负责「怎么画」。Vercel AI SDK 的 Generative UI 就是这个思路，基于 JSON 来渲染组件 UI。

约束解码在这里的价值特别大——保证输出的 JSON 一定能被前端解析，不会出现格式错误导致页面白屏。

场景三：信息提取

从非结构化文本里提取结构化数据：

json
复制
{
  "name": "张三",
  "company": "某科技公司",
  "role": "CTO",
  "contact": "zhangsan@example.com"
}


这个场景也特别常见，一个 LLM Call 就能搞定，通过 Structure Output 拿到 JSON 结构化数据。

什么时候不该用 Structured Output

有个容易踩的坑：在模型需要自由推理的阶段强制 JSON 格式，反而会降低推理质量。

约束解码在每一步都在限制 Token 的选择范围。当模型需要深度思考、权衡多个方案、做复杂推理的时候，这些限制可能影响它「想清楚」的能力。

什么意思呢？想象你在写一篇文章，但每个段落都必须严格按固定模板来。你可能为了凑格式而牺牲了表达的准确性。

所以一般的做法是：

推理阶段（Agent 在思考下一步该做什么）：不用 Structured Output，让模型自由生成文本
动作阶段（Agent 决定调用哪个工具、传什么参数）：用 tool_use，约束输出格式
输出阶段（需要固定格式的结果）：用 Structured Output

Claude 的 tool_use 设计天然就把这两个阶段分开了：模型可以在同一次回复里先输出一段自由文本（思考过程），再输出 tool_use 块（结构化的工具调用）。文本不受约束，JSON 严格约束——各取所需。

分析一下 Claude Code 的工具定义

最后看看实际的工具定义长什么样。

Claude Code 里一个工具的定义包含这些元素：

typescript
复制
{
  name: "Read",                    // 工具名
  inputSchema: z.object({...}),    // Zod Schema，定义参数类型
  description(...),                // 动态描述，根据上下文变化
  call(...),                       // 真正的执行逻辑

  // 元数据——告诉系统这个工具的「性格」
  isConcurrencySafe(input),        // 能不能并发执行？
  isReadOnly(input),               // 只读还是会修改东西？
  isDestructive(input),            // 是不是不可逆的操作？
  validateInput(input),            // 执行前的业务级校验
  checkPermissions(input),         // 权限检查

  // 加载策略
  shouldDefer: true,               // 是否延迟加载（不塞进初始上下文）
  searchHint: "jupyter notebook",  // 关键词，帮助 ToolSearch 找到它

  // 结果处理
  maxResultSizeChars: 50000,       // 结果超过这个大小就存磁盘
}


值得拆开看的设计决策：

inputSchema 用 Zod 而不是手写 JSON Schema。 Zod 是 TypeScript 的运行时类型校验库。用它定义参数，既能在编译时做类型检查，又能在运行时验证模型输出的 JSON。一份定义，两处校验。

isConcurrencySafe 依赖输入而不是工具本身。 上一章的流式架构讲过：Read 工具总是可以并发的（读文件不冲突），但 Edit 工具要看具体编辑的是哪个文件——编辑不同文件可以并发，编辑同一个文件必须串行。所以这些方法都接收 input 参数，根据具体输入来判断，这样会非常灵活。

description 是个函数而不是字符串。 工具描述可以根据上下文动态变化。比如在非交互式会话里（CI 环境），某些工具的描述会强调「不要请求用户输入」。

maxResultSizeChars 控制结果大小。 默认上限 50,000 字符。超过这个大小，结果不直接塞进对话历史，而是存到磁盘上，给模型一个摘要 + 文件路径。防止一次工具调用就把上下文撑爆。

这些元数据看起来琐碎，但它们构成了整个工具系统的「规则体系」——下一篇讲工具执行管线的时候你就会看到，每一个元数据字段都会在执行链路的某个环节被用到。

设计工具描述的实战建议

工具描述直接决定模型能不能选对工具、填对参数，这里有三条实战经验值得记住。

工具描述不是给人看的，是给模型看的。它直接影响模型能不能选对工具、能不能填对参数。

第一：描述要详细，至少 3-4 句话。

不要只写「获取天气」。要写这种比较完整的描述：

"获取指定城市的当前天气信息，包括温度、湿度和天气状况。city 参数应该是城市名称（如'北京'、'上海'），不接受经纬度。只返回当前天气，不返回预报。如果城市不存在会返回错误。"

Anthropic 有个测试显示，详细的描述能把复杂参数的处理准确率从 72% 提到 90%。

第二：用命名空间前缀区分相似工具。

如果你同时有 GitHub 和 Slack 的工具，不要叫 list_comments 和 list_messages。用 github_list_comments 和 slack_list_messages。命名空间前缀能显著减少语义碰撞。

Manus 就深谙这种优化策略，大量地用命名空间前缀来定义工具名。

第三：description 比 type 更重要。

模型在决定用哪个工具、怎么填参数的时候，主要看 description，不是看 type。你把参数名叫 user 还是 user_id，description 写清楚「这是用户的唯一标识符，格式为 UUID」，比只标注 type: string 有效得多。

下一篇

Function Calling 是工具系统的入口。模型输出了一段 JSON，说「我要调用 read_file」——然后呢？

从「模型表达意图」到「工具真正被执行」，中间还有 7 个步骤：参数校验、输入补全、Hook 拦截、权限检查……每一步都在防止模型的「好意」变成「灾难」。

下一篇我们就来拆这条执行管线。

检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
死循环、重复犯错、Token 烧穿：你的 Agent Loop 缺这三个"保险丝"
下一篇 · 第三章：Tool System —— 给 Agent 装手脚
一次工具调用背后经历了什么？以 Claude Code 为例展开聊聊


---
## 代码块


```json
{
  "type": "tool_use",
  "name": "get_weather",
  "input": { "city": "北京" }
}
```


```json
{
  "action": {
    "type": "string",
    "enum": ["read", "write", "delete"]
  }
}
```


```json
{
  "score": 8,
  "issues": ["变量命名不规范", "缺少错误处理"],
  "pass": true
}
```


```json
{
  "summary": "用户要求重构 auth 模块，已完成 login/logout，待处理 token refresh",
  "key_decisions": ["使用 JWT 替代 session", "refresh token 存 httpOnly cookie"],
  "pending_tasks": ["实现 token refresh 端点", "添加 CSRF 防护"],
  "important_context": ["项目用 Next.js 14", "数据库是 PostgreSQL"]
}
```


```json
{
  "type": "card",
  "title": "天气预报",
  "children": [
    { "type": "text", "content": "北京 · 晴 · 25°C", "style": "heading" },
    { "type": "chart", "data": [22, 25, 28, 26, 24], "labels": ["周一", "周二", "周三", "周四", "周五"] },
    { "type": "button", "label": "查看详情", "action": "navigate:/weather/beijing" }
  ]
}
```


```json
{
  "name": "张三",
  "company": "某科技公司",
  "role": "CTO",
  "contact": "zhangsan@example.com"
}
```


```typescript
{
  name: "Read",                    // 工具名
  inputSchema: z.object({...}),    // Zod Schema，定义参数类型
  description(...),                // 动态描述，根据上下文变化
  call(...),                       // 真正的执行逻辑

  // 元数据——告诉系统这个工具的「性格」
  isConcurrencySafe(input),        // 能不能并发执行？
  isReadOnly(input),               // 只读还是会修改东西？
  isDestructive(input),            // 是不是不可逆的操作？
  validateInput(input),            // 执行前的业务级校验
  checkPermissions(input),         // 权限检查

  // 加载策略
  shouldDefer: true,               // 是否延迟加载（不塞进初始上下文）
  searchHint: "jupyter notebook",  // 关键词，帮助 ToolSearch 找到它

  // 结果处理
  maxResultSizeChars: 50000,       // 结果超过这个大小就存磁盘
}
```
