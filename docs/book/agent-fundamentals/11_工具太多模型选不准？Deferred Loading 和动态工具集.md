# 工具太多模型选不准？Deferred Loading 和动态工具集

课程
吃透 AI Agent 开发
工具太多模型选不准？Deferred Loading 和动态工具集
工具太多模型选不准？Deferred Loading 和动态工具集

约 29 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

上一篇讲了工具执行的 7 步管线，从验证到权限再到执行，每一步都在防止模型做蠢事。

但有个问题我们一直没聊——工具本身的数量。

你接了 3 个 MCP Server，每个暴露 15-20 个工具。加上系统内置的 30 个工具，一下子就是 75-90 个工具。每个工具的 JSON Schema 定义平均 500-800 token。算一下，光工具描述就吃掉了 40,000-72,000 token。

这还没算你的 system prompt、对话历史、文件内容。

上下文窗口就这么大，工具定义占掉 40%-60%，留给"真正干活"的空间已经不多了。

更要命的是，前面 Function Calling 那篇聊过的数据：工具从 10 个涨到 50 个，选择准确率从 90% 掉到不到 50%，模型的性能直线下降。

在超过了一定的阈值之后，工具越多，模型越傻。

这个问题怎么解？业界有三种截然不同的思路。这篇文章我们来详细分析一下，并且最后来探讨一下如果让你来设计工具管理策略，应该如何来设计。

思路一：延迟加载——Claude Code 的 Deferred Tool Loading

Claude Code 目前有 30+ 个内置工具，再加上用户连接的 MCP Server，工具数量轻松破 50。

它的解法叫 Deferred Tool Loading——不是一开始就把所有工具的完整定义塞给模型，而是按需加载。

具体怎么做？

两类工具会被延迟

第一类是 MCP 工具。所有通过 MCP Server 接入的外部工具，默认全部延迟。原因很简单——MCP 工具是用户自己装的，数量不可控，而且大部分对话用不上。

第二类是内置工具中标记了 shouldDefer: true 的。Claude Code 把自己的 30+ 个工具分成两组：

核心工具：Read、Edit、Write、Bash、Grep、Glob、Agent、Skill——这些几乎每次对话都要用，永远加载。
低频工具：WebSearch、WebFetch、NotebookEdit、LSP、Cron、Task 管理、Plan Mode、Config——增加 shouldDefer 标记。

分类的依据是使用频率。你想想，大部分编程任务用到的就是读写文件和执行命令，WebSearch 之类的偶尔用一次，这也比较合理。

延迟工具的发现机制

这里有一个精巧的设计。

模型的 prompt 里看到的不是完整的工具 Schema，而是一个工具名字列表。类似于：

以下工具可用，但需要先通过 ToolSearch 获取完整定义：
WebSearch, WebFetch, NotebookEdit, LSP, CronCreate, CronList...


然后 Claude Code 提供了一个特殊的元工具——ToolSearch。模型需要某个延迟工具时，先调用 ToolSearch，传入查询关键词或工具名，ToolSearch 返回匹配工具的完整 Schema 定义。

说实话这个设计挺妙的。相当于给工具集加了一个"搜索引擎"——相当于你不需要把所有商品摆在货架上，顾客要什么，搜一下就行。

ToolSearch 支持三种查询方式：

精确选择：select:Read,Edit,Grep——直接按名字取
关键词搜索：notebook jupyter——模糊匹配
必选+排序：+slack send——名字里必须包含 slack，按相关性排

自动触发的阈值

不是所有场景都需要延迟加载。如果你只接了一个 MCP Server、3 个工具，没必要多此一举。

Claude Code 设了一个自动触发阈值：当延迟工具的 JSON Schema 总量超过上下文窗口的 10% 时，才启用延迟加载。

10% 是什么概念？对于 200K 上下文窗口，就是 20,000 token。大概相当于 25-30 个工具的完整定义。

低于这个阈值，所有工具正常加载，没有额外开销。高于这个阈值，自动切换到延迟模式。

完整的发现流程

这不是纯应用层的 trick，它需要 Anthropic Claude API 的原生支持。defer_loading 和 tool_reference 都是 API 的 beta 特性。如果你用的是第三方代理或者自建 API Gateway，代理不认识这些字段就会直接报 400 错误——Claude Code 检测到请求地址不是 Anthropic 原生域名时，会自动禁用延迟加载，退回到全量加载模式。

具体的发现流程分四步，理解了这四步你就完全搞清楚了：

第一步：初始请求只发核心工具。 Claude Code 在每次调 API 之前，会动态过滤工具列表——未被发现的延迟工具直接从请求里过滤掉，它们的 Schema 根本不发给 API。模型看到的只是 system prompt 里的一段名字列表："以下工具可用但需要先搜索：WebSearch, NotebookEdit, LSP..."。这些名字不占工具定义区域的空间。

第二步：模型调用 ToolSearch。 模型发现自己需要 WebSearch，就调用 ToolSearch 传入关键词。ToolSearch 返回的不是纯文本，而是 tool_reference 类型的内容块——这个块只包含工具名字，不包含完整 Schema。它被放在 tool_result 消息里，成为对话历史的一部分。

第三步：API 展开 tool_reference。 API 看到 tool_reference 块，就会在模型的上下文里注入对应工具的完整 Schema 定义。但这里有个前提——这个工具的 Schema 必须在当前请求的 tools 参数里。那它怎么进去的？我们接着说。

第四步：下一轮请求带上已发现的工具。 Claude Code 在构建每一轮请求之前，都会扫描整个对话历史，提取所有出现过的 tool_reference 块，构建一个"已发现工具"集合。这次请求的工具过滤逻辑变成了：核心工具 + 已发现的延迟工具。已发现的工具现在会被包含在 tools 参数里（带 defer_loading: true 标记），API 就能展开它们的 Schema 了。

这个流程还有一个容易忽略的细节：上下文压缩后已发现的工具不会丢失。Claude Code 在压缩对话历史时，会把当前的"已发现工具"名单单独存一份快照。压缩后即使 tool_reference 块被裁掉了，重建时会从快照里恢复，工具不会"失忆"。

整个闭环串起来就是：初始 prompt 只给核心工具的完整 Schema → 模型按需调用 ToolSearch → tool_reference 记录在对话历史里 → 下一轮请求自动带上已发现工具的 Schema → 模型正常调用。多了一次工具调用的开销，但省下来的 token 是巨大的。

不用 Anthropic API 怎么办

上面的流程依赖 defer_loading 和 tool_reference 这两个 Anthropic 的 beta 特性。如果你用的是 OpenAI、Qwen 或者其他模型，没有这些 API 能力，能不能实现类似的效果？

可以，核心思路不变，只是实现方式要调整。下面我来为你介绍两种解决的思路。

最简单的做法：ToolSearch 返回纯文本 Schema，然后动态修改 tools 列表。 自己实现一个 ToolSearch 工具，模型传入关键词，你把匹配到的工具的完整 JSON Schema 当作普通文本返回。下一轮请求时，把这个工具加进 tools 参数里。模型在对话历史里已经"看过"了这个工具的 Schema，知道怎么调用它。

这个做法的代价是：每次加入新工具都会改变 tools 列表，KV Cache 从工具定义的位置开始全部失效。对话越长，这个代价越大。但如果你的工具发现主要集中在对话前几轮（大部分场景确实如此），后面工具列表稳定下来之后 Cache 就不会再被打断了。实际用起来，这个代价是可以接受的。

更稳的做法：ToolSearch + CallTool 双工具代理模式。 不动态改 tools 列表，而是用两个固定的"元工具"配合完成整个流程：

json
复制
// tools 列表里永远只有这两个工具，从头到尾不变
[
  {
    "name": "tool_search",
    "description": "搜索可用工具，返回匹配工具的完整参数定义",
    "parameters": { "query": { "type": "string" } }
  },
  {
    "name": "call_tool",
    "description": "调用指定工具。参数格式参考 tool_search 返回的 Schema",
    "parameters": {
      "tool_name": { "type": "string" },
      "arguments": { "type": "string" }
    }
  }
]


关键在于这两个工具的配合流程：

第一步，模型调 tool_search 发现工具。 比如模型需要搜索网页，它调用 tool_search({ query: "web search" })，你在 tool_result 里返回 WebSearch 工具的完整 JSON Schema——工具名、参数类型、每个字段的含义。这段 Schema 作为对话历史的一部分，留在了上下文里。

第二步，模型根据上下文中的 Schema 调 call_tool 执行。 模型已经在对话里"看过"了 WebSearch 的参数定义，知道需要传 query 字段。于是它调用 call_tool({ tool_name: "WebSearch", arguments: "{\"query\": \"MCP specification\"}" })。应用层拿到后，根据 tool_name 路由到真正的工具实现，把 arguments 反序列化成对象再传进去。

这样 tools 列表从头到尾就两个元工具，永远不变，Cache 完全不受影响。新工具的 Schema 出现在对话历史里（tool_result 消息中），不在工具定义区域。

前面讲过的 OpenCode 的 Batch Tool 也是类似的方向——模型只调用一个 batch 工具，把实际要调的工具打包进参数里。

这个做法的代价是：模型不是通过 tools 参数里的结构化 Schema 来"认识"工具的，而是通过对话历史里的文本描述来理解参数格式。参数复杂的工具准确率会略低一些，对模型的指令遵循能力要求更高。但对于能力强的模型，实际使用中准确率是够的。

两种做法各有取舍——前者（动态修改 tools 列表）更直接但修改 tools 的那一轮失去 Cache，后者（双工具代理）对 Cache 更友好，但对模型能力要求更高。根据你的工具数量和对话长度来选就行。

对 Prompt Cache 的影响

你可能会以为，模型通过 ToolSearch 加载了一个新工具的完整 Schema，相当于改了工具列表，Cache 就炸了。实际上不会。关键在于 defer_loading: true 的工具从一开始就不参与 Cache 计算。

源码里在计算 prompt 的 cache key 时，所有标记了 defer_loading 的工具会被直接过滤掉。因为 API 端也不会把这些工具塞进模型的 prompt，它们压根就不在 cache 的范围里。

tool_reference 返回后，新加载的工具 Schema 去向也和你想的不一样——它出现在对话历史里（tool_result 消息中），不是在工具定义区域。工具定义区域（prompt 前部）从头到尾只有那几个核心工具，永远不变。

所以整个过程对 Cache 的影响是：核心工具的 Cache 前缀始终稳定，不管你通过 ToolSearch 加载了多少个延迟工具。这也是为什么这个方案比直接动态增删工具列表要好得多——它把"工具发现"这件事从 prompt 结构层面移到了对话内容层面。

这个优化机制非常的精妙，以至于我在后续讲 prompt cache 那一节的时候还是跟大家再重点强调一次。

小结一下，Deferred Tool Loading 主要是三个点：

defer_loading 的工具从一开始就不参与 Cache key 计算，在源码里直接 filter 掉了。
新加载的 Tool Schema 现在对话历史（tool_result）里，不是在工具定义区域，所以不影响 Prompt 前缀。
本质是把"工具发现"从 prompt 结构层面移到了对话内容层面，这是它比动态增删工具列表高明的地方。
效果

我从源码中统计了一下，Claude Code 有大约 20 个工具被标记为延迟加载。按平均每个工具 600-800 token 算，延迟加载省下 12,000-16,000 token。加上 MCP 工具（数量不封顶），节省更加可观。

而 ToolSearch 工具自身的 Schema 只占几百 token。

一个值得注意的细节：每个延迟工具都有一个 searchHint 字段——一个 3-10 个词的短语，帮助 ToolSearch 做关键词匹配。比如 LSP 工具的 hint 是 "code intelligence (definitions, references, symbols, hover)"，NotebookEdit 是 "edit Jupyter notebook cells (.ipynb)"。这些 hint 不会显示给模型，只在 ToolSearch 内部用于匹配。

思路二：工具配置文件——OpenClaw 的 Tool Profile

OpenClaw 的思路不太一样。它不是延迟加载，而是按场景预选。

OpenClaw 定义了 4 种 Tool Profile（工具配置文件）：

Profile	适用场景	包含工具数
minimal	最基础的交互	~3 个（session_status 等）
coding	编程场景	~15 个（read/write/edit/exec/memory 等）
messaging	通讯场景	~8 个（message/sessions 等）
full	全部能力	所有 25+ 个核心工具

每个核心工具在定义时就标注了自己属于哪些 Profile。比如 read、write、edit、exec 属于 coding Profile，message 属于 messaging Profile，session_status 在前三个 Profile 中都存在。

系统根据当前会话的场景选择对应的 Profile，然后只加载该 Profile 允许的工具。

Tool Group：批量引用

OpenClaw 还有一个 Tool Group 的概念。工具按功能分组：

group:fs → read, write, edit, apply_patch
group:runtime → exec, process
group:web → web_search, web_fetch
group:memory → memory_search, memory_get
group:sessions → sessions_list, sessions_history, sessions_send, sessions_spawn

配置时可以直接引用整个组，而不是逐个列出工具名。

这个设计的好处是可预测——你知道每个场景下模型能用什么工具，不会有意外。坏处是不够灵活——如果用户在 coding 场景下突然需要发消息，messaging 工具不可用，需要切换 Profile。

与延迟加载的本质区别

Claude Code 的延迟加载是按需发现——所有工具都可用，只是模型需要先搜索。

OpenClaw 的 Profile 是按场景裁剪——不在当前 Profile 里的工具，模型完全看不到、也不能用。

前者更灵活，后者更可控。

思路三：小工具集——Manus 的极简哲学

Manus 选了一条更激进的路：从源头控制工具数量。

核心理念是：不到 20 个原子工具 + CLI sandbox。

Manus 的工具集分三层：

第一层：原子工具（~20 个）

固定的、最小化的 function calling 工具集。类似 file_write、browser_navigate、bash、search。这些工具定义永远不变。

第二层：CLI 工具（通过 sandbox 暴露）

遇到原子工具覆盖不了的能力，Manus 的思路是不加新工具，让模型通过 bash 调用系统命令。要用 ffmpeg 的时候，执行 bash("ffmpeg -i input.mp4 ...")。要用 curl 的时候，执行 bash("curl https://...")。甚至 MCP 工具也通过 mcp-cli 命令行包装器暴露。

第三层：写脚本

更复杂的组合逻辑就让模型写 Python 或 Node.js 脚本放到 sandbox 里跑。

Manus 的原话是："heavily armed agents get dumber"——给 Agent 堆太多武器，它反而变笨了。

这个哲学非常反直觉。大多数人的第一反应是"工具越多越好"，但 Manus 反过来做——用最少的工具覆盖最多的场景，复杂操作通过组合原子工具来实现。

仔细一想，这个分层的确非常的优雅，既减少了默认的工具数量，节省了 token，又极大地保证了工具本身的灵活性，因为通过 CLI 和脚本可以做的事情太多了。

KV Cache 杀手：为什么不能随便改工具列表

三种思路都绕不开一个底层问题：KV Cache。

前面讲过，KV Cache 是模型推理的加速器——上下文的前缀不变，计算结果就能复用。Anthropic 的 Prompt Caching 缓存命中是 $0.30/百万 token，未命中是 $3.00/百万 token，10 倍差距。

工具的 JSON Schema 定义在 prompt 的前部（system prompt 之后、对话历史之前）。如果你在对话过程中动态增减工具，从工具定义的位置开始，后面所有内容的 KV Cache 全部失效。

什么意思呢？假设你第一轮有 10 个工具，第二轮变成 12 个——不是只多算那 2 个工具的 token，而是从工具定义开始到对话末尾的所有 token 都要重新计算。如果对话已经有 100K token 了，你多加 2 个工具的代价是重新计算 100K token。

这就是为什么 Claude Code 的延迟加载要和 API 配合——被延迟的工具用 defer_loading: true 标记，API 知道这些工具不影响 Cache 计算。ToolSearch 加载新工具后，通过 tool_reference 块注入，而不是直接修改工具列表。

Manus 的 "Mask Don't Remove"

Manus 对 Cache 问题有一个特别好的解法：不删除工具定义，用 mask 标记不可用。

传统做法是：要禁用某个工具就从工具列表里移除。但这样改变了 prompt 结构，Cache 直接失效。

Manus 的做法：工具列表永远不变。需要禁用某些工具时，不修改定义，而是在解码阶段通过 response prefill 让模型选不了那些工具——把被禁工具的生成概率直接设为零。

工具名用一致的前缀（browser_*、shell_*），这样可以按组整体 mask，不需要逐个管理。

效果是：Manus 的 Prompt Cache 命中率达到 ~95%，相比动态增删工具的传统做法（~20% 命中率），成本降低 60%-85%，响应速度提升 2-3 倍。

三种思路的权衡
	Claude Code 延迟加载	OpenClaw Profile	Manus 小工具集
核心思路	按需发现	按场景裁剪	从源头控制
工具上限	无限（延迟的不占空间）	由 Profile 决定	~20
Cache 影响	低（API 支持 defer_loading）	切换 Profile 会影响	极低（工具列表永远不变）
额外开销	多一次 ToolSearch 调用	无	复杂操作需要多步组合
灵活性	最高	中等	最低但最可控
需要 API 支持	是（defer_loading + tool_reference）	否	是（logit masking）

没有哪种方案是绝对最优的。Claude Code 的延迟加载最灵活，但依赖 API 层的 beta 特性。Manus 的小工具集最省 token、Cache 最友好，但需要模型有更强的组合能力。OpenClaw 的 Profile 是个务实的中间路线——不需要 API 支持，开发成本低，大多数场景够用。

你想想，这三种方案本质上在解决同一个问题：怎么在有限的上下文窗口里，让模型既能"看到"足够多的能力，又不被太多选项搞晕？

说白了就是一个信息架构问题——跟搜索引擎的索引、电商的商品分类、操作系统的文件系统没什么本质区别。你不会把 100 万个商品全铺在首页，你也不应该把 100 个工具全塞进 prompt。

设计你自己的工具管理策略

如果你在做自己的 Agent 产品，怎么选？

工具 < 15 个：别折腾，全部加载。这个量级模型是可以直接处理的，加什么管理机制都是过度工程。

工具 15-30 个：开始可以考虑拆分了。有两种拆分思路：

参考 OpenClaw，按场景把工具分成几个子集，对话开始时根据任务类型选一个子集加载。实现起来很简单，一个 map 就搞定，不需要什么框架。
启动 Sub Agent（Multi-Agent 这一章会展开讲），把某些特定的工具放到子 Agent 中，这样主 Agent 的工具集就能够得到精简了。

工具 30-50 个：需要延迟加载。核心思路是：prompt 里只放工具名字列表，再提供一个"搜索工具"让模型按需获取完整 Schema。这个思路不依赖任何特定 API——你可以自己实现一个 ToolSearch，模型传入关键词，你返回匹配工具的完整定义。

工具 50+ 个：先停下来想一个问题——你真的需要 50 个独立工具吗？很多能力可以通过 bash + sandbox 组合出来，不需要每个都定义成独立工具。Manus 用不到 20 个原子工具覆盖了绝大多数场景，靠的就是这个思路。

另外有一件事特别容易忽略：工具列表的稳定性比工具数量更重要。

宁可多几个用不上的工具一直放在列表里（反正模型不选它就不影响），也不要在每轮对话时动态增删——那会杀死你的 Cache。

Manus 的 "Mask Don't Remove" 是一个非常值得借鉴的原则：不可用的工具，禁用它的输出概率，而不是从列表里删掉它。如果你没有 response prefill 的能力，至少保持工具列表在整个对话过程中不变。

下一篇我们进入 Tool System 的最后一个核心话题——MCP 和 Skills 两条路线的对比。同样是"给 Agent 扩展能力"，两者各自解决了什么问题、又各自留下了什么坑，MCP 是不是一个要被淘汰的技术，下一篇我们来深入聊。

参考资料
Anthropic Prompt Caching 定价与文档: https://docs.claude.com/en/docs/build-with-claude/prompt-caching
Anthropic Tool Use 文档: https://docs.claude.com/en/docs/agents-and-tools/tool-use/overview
Manus: Context Engineering for AI Agents: https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus
MCP 规范: https://modelcontextprotocol.io/specification
OpenClaw: https://github.com/openclaw/openclaw
OpenCode 源码: https://github.com/sst/opencode
检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
一次工具调用背后经历了什么？以 Claude Code 为例展开聊聊
下一篇 · 第三章：Tool System —— 给 Agent 装手脚
MCP 的工程真相：协议很好，但也有一些硬伤


---
## 代码块


```
以下工具可用，但需要先通过 ToolSearch 获取完整定义：
WebSearch, WebFetch, NotebookEdit, LSP, CronCreate, CronList...
```


```json
// tools 列表里永远只有这两个工具，从头到尾不变
[
  {
    "name": "tool_search",
    "description": "搜索可用工具，返回匹配工具的完整参数定义",
    "parameters": { "query": { "type": "string" } }
  },
  {
    "name": "call_tool",
    "description": "调用指定工具。参数格式参考 tool_search 返回的 Schema",
    "parameters": {
      "tool_name": { "type": "string" },
      "arguments": { "type": "string" }
    }
  }
]
```
