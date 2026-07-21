# 让对话越来越便宜——Prompt Cache 与成本追踪

课程
Super Agent 实战课
让对话越来越便宜——Prompt Cache 与成本追踪
让对话越来越便宜——Prompt Cache 与成本追踪

约 23 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇用三层即时防线把上下文控住了——截断、修剪、估算，毫秒级完成。但还有一个问题没解决：成本。

上下文大小控制住了不代表花钱少了。即使每轮上下文只有 20K tokens，你聊 50 轮就等于把这 20K 重新发给 API 50 次。System prompt、工具描述、前面的对话历史——这些内容在大部分轮次里几乎不变，但每轮都要重新付费。

Prompt Cache 解决的就是这个问题。它把请求的"前缀"缓存在服务端，下次发同样的前缀直接复用，不重新跑前向计算。各家的 cache 命中价格大约是正常 input 的 10-25%——也就是说，稳定不变的那部分上下文，每轮只花正常价格的十分之一到四分之一。让人惊喜的是，Deepseek V4 把这个价格继续砍到了 1/100（详情文档），让缓存的 token 费用近乎变为免费。

这篇我们主要来做三件事：

搞清楚真实世界里面各家模型厂商 Cache 机制的差异；
给 Agent 加上完整的成本追踪链路；
做一个终端面板让你随时看上下文占用和花费。

先装依赖：

bash
运行
复制
pnpm install

Prompt Cache 的三种模式

各家的 Prompt Cache 实现看起来很乱，但归纳起来就三种模式：

隐式缓存——代码什么都不用改，只要前缀够长就自动缓存。OpenAI 和 DeepSeek 走这条路。OpenAI 要求最少 1024 tokens 前缀，DeepSeek 最小单元只有 64 tokens。缓存是透明的，你从 usage 响应里看到 cached_tokens 字段就知道命中了多少。

显式标记模式——你在请求里挂一个 cache_control: { type: "ephemeral" } 标记，告诉 API"从开头到这里都缓存"。Claude 和 Qwen 走这条路（Qwen 的 explicit 模式直接复用了 Claude 的字段名，代码不用改）。最多挂 4 个标记，通常的策略是 tools 末尾挂一个、system 末尾挂一个、稳定的对话历史末尾挂一个。

显式 Cache 创建模式——先调 API 创建一个 cache 对象拿到 ID，后续请求带这个 ID。Gemini 的 explicit 模式和火山豆包走这条路。适合大段固定知识库的场景，但额外收存储费。

简单来说，就是隐式缓存和显式缓存两种模式，后面两个可以归类到显式缓存的范畴。接下来，我们梳理一下各大主流厂商的缓存模式：

提供商	模式	命中折扣	TTL
Claude	显式缓存（标记）	90% off	5min / 1h
OpenAI GPT-5 / GPT-5.5	隐式缓存	75% / 90% off	
Gemini 3	双模式支持	75% off（含存储费）	1h
DeepSeek V4 Flash / Pro	隐式缓存	99% off	数 h 到数天
Qwen 3.6	双模式支持	80~90% off	5min
MiniMax M2 系列	隐式缓存	~80% off	未公开
豆包 2.0	显式缓存（创建对象）	80% off	1h~7d
智谱 GLM-4.6 / 5.x	隐式缓存	~80% off	

这里需要注意一下，隐式缓存并不一定每次都生效。要看两件事：

一是 prompt 前缀够不够长——低于厂商 token 阈值（比如 OpenAI 是 1024、Claude Sonnet 4.6 是 2048、Opus 4.7 / Haiku 4.5 是 4096）的前缀根本不会被写入；
二是厂商缓存池的实时状态——同一份请求被路由到哪台机器（缓存是按节点的，不是全局共享）、缓存有没有在 LRU 中被新请求挤掉、TTL 有没有过期，都会影响命中。所以隐式缓存的命中率本质上是个概率（生产环境 60–90% 算正常），不是 100% 确定。显式缓存因为绑定了 cache_id 或 cache_control 标记，命中是确定的。

生产场景下推荐先用隐式缓存（OpenAI / DeepSeek / MiniMax / GLM）或者显式缓存的标记模式（Claude / Qwen explicit）——代码几乎不用改、命中折扣已经够大。等 Agent 真上线、花费明显比较高的时候，再考虑显式创建对象模式（Gemini / 豆包）做进一步优化：它能保证更高的命中率，同时锁更长的 TTL。

知识体系课的 KV Cache 那篇详细分析过这些机制的底层原理，这里只要记住一个核心原则就够：前缀越稳定，cache 命中率越高，花钱越少。

实战场景哪些操作会杀 Cache

Cache 命中要求前缀字节级一致。几个最常见的实战踩坑点跟大家强调一下：

System prompt 里塞时间戳。当前时间：${new Date().toISOString()} 放在 system prompt 开头，每秒都变，cache 永远 miss。如果一定要给模型时间感，放在 user message 末尾（不影响前缀），或者按"当天 00:00"对齐成稳定字符串。

工具列表每轮变化。前面做的动态工具加载——tool_search 发现新工具后加进 tools 参数——会改变前缀，cache 失效。

前面讲 Prompt Pipe 时提过的"先静后动"原则——不变的 section 放前面，变的放后面——本质上就是在优化 cache 命中率。

成本追踪：每一步花了多少钱

知道了 cache 的原理，接下来要做的是让成本可见。不追踪就不知道优化有没有效果。

新建 src/usage-tracker.ts，核心是两个东西：一张价格表和一个 normalizeUsage 函数。

src/usage/tracker.ts
应用
复制
export interface ModelPricing {
  input: number;       // $/1M tokens (cache miss)
  output: number;
  cacheWrite: number;
  cacheRead: number;
}

export const PRICE_TABLE: Record<string, ModelPricing> = {
  'claude-sonnet-4-7': { input: 3.00,  output: 15.00, cacheWrite: 3.75,  cacheRead: 0.30 },
  'claude-haiku-4-5':  { input: 1.00,  output: 5.00,  cacheWrite: 1.25,  cacheRead: 0.10 },
  'gpt-5':             { input: 5.00,  output: 15.00, cacheWrite: 5.00,  cacheRead: 1.25 },
  'deepseek-v3-2':     { input: 0.27,  output: 1.10,  cacheWrite: 0.27,  cacheRead: 0.027 },
  'qwen3-6-plus':      { input: 0.40,  output: 1.20,  cacheWrite: 0.40,  cacheRead: 0.04 },
  'mock-model':        { input: 1.00,  output: 5.00,  cacheWrite: 1.25,  cacheRead: 0.10 },
};


各家 SDK 返回的 usage 字段不一样。AI SDK v5 把 cache read 标准化到顶层 cachedInputTokens（OpenAI、DashScope 都映射到这里）；Anthropic 的 cache_creation_input_tokens 是 cache write，没被 AI SDK 标准化，得从 providerMetadata.anthropic 里拿。再加上 OpenAI 的 cached tokens 已经被算进了 inputTokens 总数（要减出来），Anthropic 是单列（不用减）——这些差异如果不抹平，Agent 切 provider 时算账就乱套了。

normalizeUsage 做的事情就是把这些差异抹平到统一的四类 token：

src/usage/tracker.ts
应用
复制
export function normalizeUsage(usage: any): StepUsage {
  if (!usage) return { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 };

  const cacheRead =
    usage.cachedInputTokens                                        // AI SDK 标准字段
    ?? usage.providerMetadata?.openai?.cachedTokens                // OpenAI 原生
    ?? 0;

  const cacheWrite =
    usage.cacheCreationInputTokens                                 // Anthropic SDK 直接挂顶层
    ?? usage.providerMetadata?.anthropic?.cacheCreationInputTokens // AI SDK 走 provider 元数据
    ?? 0;

  // OpenAI 把 cached tokens 含在 inputTokens 里 → 减出来；Anthropic 单列 → 不用减
  let inputTokens = usage.inputTokens ?? 0;
  if (cacheRead && inputTokens >= cacheRead) inputTokens -= cacheRead;

  return {
    inputTokens: Math.max(0, inputTokens),
    outputTokens: usage.outputTokens ?? 0,
    cacheReadTokens: cacheRead,
    cacheWriteTokens: cacheWrite,
  };
}


这种"兼容多家"的胶水代码看着不优雅，但如果你的 Agent 要支持在 Claude / OpenAI / DeepSeek 之间切换，这层一定得有。接新 provider 就在对应的两行里补一句 ?? usage.providerMetadata?.xxx?.cachedTokens 就行。

UsageTracker 本体负责存每一步的记录、按价格表算钱、可选地 append 到 JSONL 持久化：

src/usage/tracker.ts
应用
复制
export class UsageTracker {
  private steps: StepRecord[] = [];

  record(model: string, usage: StepUsage): StepRecord {
    const cost = computeCost(model, usage);
    const record = { ts: Date.now(), model, cost, ...usage };
    this.steps.push(record);
    return record;
  }

  totals() {
    // 累加四类 token + 总成本
    // 额外算一个 baselineCost：假如没有 cache 该花多少钱
    // savedCost = baselineCost - cost 就是 cache 省下的金额
  }
}


totals() 里有一个关键的计算——baselineCost：把所有 cache read/write 都按 input 全价重新算一遍，得到"假如没有 cache 该花多少钱"。跟实际 cost 算个差值就是 cache 帮你省下的金额。

在 agent-loop.ts 里，每一步算完 usage 后调 tracker.record()，cache 命中时打印一行提示：

src/agent/loop.ts
应用
复制
const norm = normalizeUsage(stepUsage);
const stepRecord = tracker?.record(modelId, norm);

if (norm.cacheReadTokens > 0) {
  console.log(`  [cache hit] read ${norm.cacheReadTokens} tokens · $${stepRecord.cost.toFixed(5)}`);
}

/context：把上下文画给你看

Claude Code 有个 /context 命令——输进去之后终端里出现一张方块矩阵，每个方块代表上下文的一小片，按"谁在占用"标不同颜色。一眼看清楚当前上下文 60% 是消息历史、3% 是 system prompt、1% 是工具描述。

我们也做一个。长会话下觉得"模型变笨了"，第一反应就是看 /context——是不是历史消息把推理空间挤没了？或者工具列表是不是悄悄涨了？通过这个看板可以一目了然。

src/context/view.ts
应用
复制
// 把 1M window 切 256 份，每份约 4000 tokens
// 按 slices 顺序填彩色方块：system ●  tools ●  messages ●  free ○  buffer ▢
export function renderContextMatrix(snapshot: ContextSnapshot): string {
  // ... 16×16 方块矩阵渲染 ...
}


跑起来看效果：

bash
运行
复制
pnpm start

  ● ● ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    Mock Model
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    1.7k/1.0M tokens (0.2%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○
  ...                                  ● System prompt: 1.1k (0.1%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ● System tools:  550 (0.1%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ○ Free space:   948k (94.8%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ▢ Buffer:       50k (5.0%)


刚启动只用了 0.2%，绝大部分都是 free space。但聊到第 50 轮之后，矩阵会被蓝色消息方块填满一大片——那时候 /context 才真正体现价值。

autocompact buffer（空心方块 ▢）是预留给摘要压缩的缓冲区——上下文达到这个水位线就会触发上一篇讲的 LLM 压缩。在 /context 视图里一眼就能看到"我离触发自动压缩还有多远"。这个视图不是为了好看，是实打实的调试工具——长会话下模型突然变笨了，先看 /context，十有八九是消息历史把推理空间挤没了。

/usage：花了多少、省了多少

/context 看"占了多少"，/usage 看"花了多少"。

  Usage Summary
  3 步累计

  ◎ Input             67 tokens
  ◈ Cache write     1.1k tokens
  ◉ Cache read      2.3k tokens   (65.4% hit)
  ◇ Output            69 tokens

  Cache hit rate  ████████████████████░░░░░░░░░░  65.4%

  Cost            $0.0021
  Without cache   $0.0039
  Saved           $0.0018 (46.1% off)


第一轮全是 cache write（写入比 miss 还贵 25%），单看这一步不省。但 Round 2 / Round 3 都 hit 了——读取只花十分之一价。三轮平均下来，实际只花了假想成本的一半。

Prompt Cache 的回报模式是前期投入、后期省心。 第一轮写入缓存要多花 25%，但后续每一轮都省 90%。所以调用次数越多越划算——单次调用的场景不适合开 explicit cache（写入比不写还贵），但 Agent 这种动辄几十轮的多步对话，是 cache 的最佳应用场景。

实测：开 cache vs 关 cache

代码里内置了 /cache off 命令，关掉之后每次请求都按 cache miss 全价计算。对比一下：

开 cache 跑 3 轮:
  Cost            $0.0021
  Without cache   $0.0039
  Saved           $0.0018 (46.1% off)

关 cache 再跑 1 轮:
  Cost            $0.0034    ← 只多一轮但成本涨了 60%
  Without cache   $0.0052
  Saved           $0.0018 (34.3% off)


开 cache 三轮花 $0.0021，关掉之后只多聊一轮就到了 $0.0034——单轮成本接近翻倍。如果你的 Agent 一天处理几千轮对话，这是实打实的倍数级差距。

这种前后对比自己跑一下就能感受到。你不需要完全相信文档里写的"90% off"，真正整体省了多少钱，看自己的 /usage 面板就知道了。

到这里 Context Engineering 这一章就全部完成了。Session 持久化、Prompt Pipe、LLM 摘要压缩、三层即时防线、Cache 优化和成本追踪——从"对话不丢"到"花钱变少"，每一层都在让 Agent 更加适合生产环境。

下一章进入跨会话记忆和 RAG——让 Agent 不仅能在一次对话里保持连贯，还能跨会话记住用户是谁、记住上次做了什么决定、从外部知识库检索信息。一个真正"认识你"的 Agent，要从那里开始。我们下一章再见👋🏻

参考链接
Anthropic Prompt Caching
OpenAI Prompt Caching
Google Gemini Context Caching
DeepSeek Context Caching
阿里云 Qwen Context Cache
Manus Context Engineering
上一篇
三层即时防线——Token 估算、工具截断与 TTL 修剪
下一篇 · 第四章：Memory + RAG
关掉终端再打开，Agent 还记得你是谁——持久化记忆系统
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
export interface ModelPricing {
  input: number;       // $/1M tokens (cache miss)
  output: number;
  cacheWrite: number;
  cacheRead: number;
}

export const PRICE_TABLE: Record<string, ModelPricing> = {
  'claude-sonnet-4-7': { input: 3.00,  output: 15.00, cacheWrite: 3.75,  cacheRead: 0.30 },
  'claude-haiku-4-5':  { input: 1.00,  output: 5.00,  cacheWrite: 1.25,  cacheRead: 0.10 },
  'gpt-5':             { input: 5.00,  output: 15.00, cacheWrite: 5.00,  cacheRead: 1.25 },
  'deepseek-v3-2':     { input: 0.27,  output: 1.10,  cacheWrite: 0.27,  cacheRead: 0.027 },
  'qwen3-6-plus':      { input: 0.40,  output: 1.20,  cacheWrite: 0.40,  cacheRead: 0.04 },
  'mock-model':        { input: 1.00,  output: 5.00,  cacheWrite: 1.25,  cacheRead: 0.10 },
};
```


```typescript
export function normalizeUsage(usage: any): StepUsage {
  if (!usage) return { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 };

  const cacheRead =
    usage.cachedInputTokens                                        // AI SDK 标准字段
    ?? usage.providerMetadata?.openai?.cachedTokens                // OpenAI 原生
    ?? 0;

  const cacheWrite =
    usage.cacheCreationInputTokens                                 // Anthropic SDK 直接挂顶层
    ?? usage.providerMetadata?.anthropic?.cacheCreationInputTokens // AI SDK 走 provider 元数据
    ?? 0;

  // OpenAI 把 cached tokens 含在 inputTokens 里 → 减出来；Anthropic 单列 → 不用减
  let inputTokens = usage.inputTokens ?? 0;
  if (cacheRead && inputTokens >= cacheRead) inputTokens -= cacheRead;

  return {
    inputTokens: Math.max(0, inputTokens),
    outputTokens: usage.outputTokens ?? 0,
    cacheReadTokens: cacheRead,
    cacheWriteTokens: cacheWrite,
  };
}
```


```typescript
export class UsageTracker {
  private steps: StepRecord[] = [];

  record(model: string, usage: StepUsage): StepRecord {
    const cost = computeCost(model, usage);
    const record = { ts: Date.now(), model, cost, ...usage };
    this.steps.push(record);
    return record;
  }

  totals() {
    // 累加四类 token + 总成本
    // 额外算一个 baselineCost：假如没有 cache 该花多少钱
    // savedCost = baselineCost - cost 就是 cache 省下的金额
  }
}
```


```typescript
const norm = normalizeUsage(stepUsage);
const stepRecord = tracker?.record(modelId, norm);

if (norm.cacheReadTokens > 0) {
  console.log(`  [cache hit] read ${norm.cacheReadTokens} tokens · $${stepRecord.cost.toFixed(5)}`);
}
```


```typescript
// 把 1M window 切 256 份，每份约 4000 tokens
// 按 slices 顺序填彩色方块：system ●  tools ●  messages ●  free ○  buffer ▢
export function renderContextMatrix(snapshot: ContextSnapshot): string {
  // ... 16×16 方块矩阵渲染 ...
}
```


```bash
pnpm start
```


```
● ● ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    Mock Model
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    1.7k/1.0M tokens (0.2%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○
  ...                                  ● System prompt: 1.1k (0.1%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ● System tools:  550 (0.1%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ○ Free space:   948k (94.8%)
  ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○    ▢ Buffer:       50k (5.0%)
```


```
Usage Summary
  3 步累计

  ◎ Input             67 tokens
  ◈ Cache write     1.1k tokens
  ◉ Cache read      2.3k tokens   (65.4% hit)
  ◇ Output            69 tokens

  Cache hit rate  ████████████████████░░░░░░░░░░  65.4%

  Cost            $0.0021
  Without cache   $0.0039
  Saved           $0.0018 (46.1% off)
```


```
开 cache 跑 3 轮:
  Cost            $0.0021
  Without cache   $0.0039
  Saved           $0.0018 (46.1% off)

关 cache 再跑 1 轮:
  Cost            $0.0034    ← 只多一轮但成本涨了 60%
  Without cache   $0.0052
  Saved           $0.0018 (34.3% off)
```
