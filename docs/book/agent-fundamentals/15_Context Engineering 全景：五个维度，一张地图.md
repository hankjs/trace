# Context Engineering 全景：五个维度，一张地图

课程
吃透 AI Agent 开发
Context Engineering 全景：五个维度，一张地图
Context Engineering 全景：五个维度，一张地图

约 14 分钟

AI 私教
专属 1v1 AI 私教，围绕本节内容深度教学
进入私教

从这篇开始，进入第四章——Context Engineering，整个课程最重的部分。

先说一个判断：Context Engineering 是 2025-2026 年 Agent 开发最重要的工程能力，没有之一。

为什么这么说？因为模型能力大家都在用同一批（Claude、GPT、Gemini、Qwen），工具系统大同小异（都是 Function Calling + Bash），但上下文管理的好坏直接决定了你的 Agent 在第 50 轮还能不能做出正确的决策。

Manus 在今年的 LangChain Webinar 上分享了一个很好的观点：Context Engineering 是应用层和模型层之间最清晰、最实际的分界线。模型的能力你改不了（除非你去 fine-tune，但那是另一条路），你能改的就是喂给模型什么上下文。

先避开两个坑

在展开之前，先说两个容易先入为主的坑。

第一个坑：既然 Agent 不够聪明，那我去训一个自己的模型吧。

Fine-tuning 和 post-training 现在门槛确实越来越低了。现在在国内的火山方舟或者百炼平台上，微调一个模型的成本越来越低了。但问题是：模型迭代的速度远远跟不上产品迭代的速度。你花三个月训了一个模型，结果底座模型升级了，你的训练数据和策略可能全得重来。在产品还没找到 PMF 之前，把精力花在 Context Engineering 上的 ROI 远高于训模型。

第二个坑：等产品稳定了，我用 RL（强化学习） 来优化 Agent 的行为。

听起来很美好，但你还得支持 MCP、支持用户自定义工具、支持各种边界情况。基础模型公司在 tool use 上投入的训练资源，你是很难追上的。所以，不要重建基础模型公司已经做好的东西。Context Engineering 才是正道。

所以你能看到，无论是什么层次的 Agent 公司，都开始纷纷重视起 Context Engineering，原因无他，这个东西太重要了，而且别无他路，训模型终究做不赢模型厂商。但平心而论，大部分人对 Context Engineering 一直都是一知半解的，不知道怎么去系统来学。

今天，我们就正式地开始 Context Engineering 的学习，我会综合各个知名 Agent 产品的生产级实践，以及我之前 Agent 的开发经历，给大家系统梳理这个领域的地图，让大家能够建立完整的知识脉络，并把各个核心的概念各个击破。

五个维度：一张完整的地图

Context Engineering 到底包含哪些事情？不同团队的实践不一样，但归纳起来其实就是五个维度：

维度	核心问题	做什么
Offload（卸载）	上下文太挤了	把信息搬到上下文之外（文件、数据库）
Reduce（压缩）	上下文太大了	就地缩小（compaction）或用摘要替换（summarization）
Retrieve（检索）	需要的信息不在上下文里	从外部按需取回（RAG、文件读取）
Isolate（隔离）	一个上下文装不下所有事	拆成多个独立的上下文（Multi-Agent）
Cache（缓存）	重复计算太贵了	复用已有的计算结果（KV Cache、Prompt Cache）

有意思的是，行业对这五个维度并没有共识。

Manus 重度使用 Offload 和 Cache，但不太鼓励 Reduce——因为压缩有信息丢失风险，他们更倾向于把信息卸载到文件系统而不是压缩。

Cognition（Devin）重度使用 Reduce，但不太鼓励 Isolate——因为多 Agent 的协调开销太大。

Anthropic 自己的 research 团队五个维度都用。

没有银弹，每个维度都有 trade-off。 你需要根据自己的场景选择组合。

这一章前半段会深入讲 Reduce 和 Cache，后半段进入 Memory 和 RAG，同时在各处穿插 Offload 和 Retrieve 的内容。Isolate 放到 Multi-Agent 章节里详细展开。

下面先把五个维度快速过一遍，给你建立全局认知。

Offload：把信息搬到上下文之外

这个维度的核心思想是：上下文窗口是昂贵且有限的，但文件系统是廉价且无限的。

你的 Agent 读了一个 3000 token 的文件，这个结果放在上下文里。到了第 30 轮，这个文件内容大概率已经没用了，但还占着 3000 token。如果你把它写到文件系统里，上下文里只留一个引用（"文件内容已存到 /tmp/auth.ts"），需要的时候再读回来，3000 token 就释放了。

这就是 Manus 说的"文件系统当外挂大脑"。不是删信息，是把信息搬到更便宜的存储介质上。

但 Manus 更进一步，把 Offloading 做成了一个三层的分级体系——不只是数据要卸载，连工具本身也要卸载：

Level 1：Function Calling

标准的工具调用，模型看到完整的工具定义（JSON Schema）然后决定调哪个。好处是安全、规范。但有两个问题：工具定义吃上下文（每个工具几百到几千 token），而且工具列表一变就破坏 Cache。

Level 2：Sandbox Utilities

模型不通过 Function Calling 来调工具，而是直接在 sandbox 里跑 shell 命令。比如要查一个文件，不用定义 read_file 工具，直接 cat /path/to/file。要安装一个包的场景，直接 pip install xxx。

好处是不碰模型的上下文——shell 命令不需要工具定义，新增能力不需要改工具列表，Cache 稳定。而且大输出可以直接写文件，不进上下文。Manus 的每个会话跑在独立的 VM sandbox 里，所以模型可以安全地 sudo 执行命令。

Level 3：Packages & APIs

最极端的卸载方式——模型不直接做数据密集型操作，而是写一段 Python 脚本来做。比如要查 20 个城市的天气，不是模型调 20 次天气 API（每次结果都进上下文），而是模型写一段脚本：取城市列表、循环调 API、汇总结果、写到文件。

整个过程上下文里只有"我写了一段脚本"和"脚本执行完了，结果在 /tmp/weather.json"，中间的过程由机器来执行，跟模型无关。

核心原则：上下文留给推理，数据操作交给代码。

这三层每一层都在做同一件事：把不必要的信息从模型上下文里搬出去。

其实我们在第 10 篇讲的 Deferred Tool Loading、Tool Profile，本质上也是 Offloading 的思路——不一开始就把所有工具定义塞进上下文。

Reduce：就地压缩

上下文已经满了，Offload 来不及了，怎么办？就地压缩。

这里有一个重要的区分，很多人容易混淆：Compaction 和 Summarization 是两种完全不同的策略。

Compaction（紧凑化）：不改对话结构，只缩小内容。比如把老旧的工具结果替换成一个占位符（"[Old tool result cleared]"），或者把工具调用的参数缩短（去掉 content 字段，只留 path）。消息还在，结构不变，只是每条消息变小了。

Summarization（摘要化）：用一段摘要替换整段对话历史。消息结构被破坏了，取而代之的是一段 LLM 生成的总结文字。信息损失更大，但 token 回收也更多。

Compaction 是可逆的（文件还在文件系统里，需要的时候读回来），Summarization 是不可逆的（原始对话永久丢失）。这两个概念大家记住，我们在后续讲上下文压缩的时候还会展开。

而下一篇我们会详细拆解这两种策略在 Claude Code 和 OpenClaw 里的具体实现。

Retrieve：按需检索

上下文里没有模型需要的信息，怎么办？从外部取回来。

这个维度跟 RAG 直接相关，也跟我们第 10 篇讲的 Deferred Tool Loading 相关——模型发现需要一个工具的完整 Schema，通过 ToolSearch 把它检索回来。

Retrieve 的关键挑战不是"能不能取到"，而是"取回来的东西有没有用"。语义相似不等于任务相关——这个话题我们在后面讲 Memory 和 RAG 的时候会深入展开。

Isolate：上下文隔离

一个上下文装不下所有事的时候怎么办？拆成多个。

这本质上就是 Multi-Agent，但从 Context Engineering 的角度看，它是一种上下文管理策略，不只是"多个 Agent 协作"。

对于多 Agent 场景，有两种上下文的隔离模式：

By communicating（消息传递）：主 Agent 给子 Agent 发一条指令（"帮我分析这个文件的安全风险"），子 Agent 有自己完全独立的上下文，完成后只把结果返回给主 Agent。主 Agent 的上下文里只多了一条指令和一个结果，而不是子 Agent 的整个思考过程。

By sharing context（Fork）：子 Agent 直接复制主 Agent 的完整上下文，在此基础上继续工作。好处是子 Agent 拥有完整的历史信息，不需要重新解释背景。坏处是上下文被复制了一份，token 开销翻倍。

Claude Code 两种都用——子 Agent 走"by communicating"模式（独立上下文），Auto-Compact 的摘要生成走 Fork 模式（需要看到完整对话历史才能做摘要）。

这个话题我们在 Multi-Agent 那一章会详细展开，这里先建立认知。

Cache：复用计算结果

同样的 system prompt 每轮都要发，同样的对话历史每轮都在增长——这些重复的内容有没有办法不重新计算？

Cache 分三层：KV Cache（模型推理层，你管不了）、Prompt Cache（API 层，ROI 最高）、Context Collapse（应用层，可逆折叠）。后面有一整篇专门讲这个。

这五个维度的优先级

如果你在做自己的 Agent 产品，这五个维度的优先级大致是：

先 Offload——能不放进上下文的就不放。工具结果写文件、比较大的输出存磁盘、数据操作使用脚本完成。
再 Cache——减少重复计算。静态 prompt 缓存、对话历史前缀匹配。
然后 Reduce——先 Compaction（缩小但保留结构），实在不行再 Summarization（摘要替换）。
需要的话 Isolate——一个 Agent 搞不定就拆成多个，每个有自己的上下文。
贯穿始终 Retrieve——模型需要什么信息就按需取，不要预加载。

从轻到重，能用简单手段解决的就不上复杂方案。

当然，如果你要跟别人讲 Context Engineering，你把这套框架 show 给他，也可以充分体现你对 Agent 的理解深度，一定会给你加不少分。

这一章的路线图

这一章是整个课程最重的一章，一共九篇，分两个阶段展开：

前半段：当前会话的上下文管理

System Prompt 工程化——怎么组装 prompt、怎么分模块、怎么做缓存分层，加上 Context Rot 的问题
上下文压缩——Compaction vs Summarization，Claude Code 和 OpenClaw 的方案对比
Cache 全解——KV Cache / Prompt Cache / Context Collapse，各家 API 的缓存策略对比，成本控制
JIT Context——按需加载的三条路线：Agentic Search、RAG、Context Offloading

后半段：跨会话的长期上下文

Agent 记忆系统——文件派 vs 数据库派，存什么、怎么存、怎么召回
RAG 全流程——从一堆文档到 Agent 能用的知识库
GraphRAG——当向量检索搞不定多跳推理，图数据库怎么补位
知识编译——让知识库持续积累和互联
检索优化——语义相似 ≠ 任务相关，怎么让 Agent 找到真正需要的信息
记忆失效——Agent 记忆的五种失效模式和工程解法

Offloading 的内容会贯穿在各篇里（因为它不是一个独立的技术，而是一种思维方式），Isolation 留给 Multi-Agent 章节。

好，让我们继续，下一篇从 System Prompt 工程化的话题讲起，Let's go!

检验一下这章的理解
AI 面试官针对本章内容提问，帮你巩固理解
开始检验
上一篇
你敢让 AI 直接跑 rm -rf 吗？生产级权限系统的四层防线
下一篇 · 第四章：Context Engineering —— Agent 真正的护城河（含 Memory）
System Prompt 工程化与 Context Rot：从"写一段提示词"到"搭建一个行为控制系统"