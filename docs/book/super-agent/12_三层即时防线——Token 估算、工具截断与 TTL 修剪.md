# 三层即时防线——Token 估算、工具截断与 TTL 修剪

课程
Super Agent 实战课
三层即时防线——Token 估算、工具截断与 TTL 修剪
三层即时防线——Token 估算、工具截断与 TTL 修剪

约 24 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇用 LLM 做了摘要压缩。效果不错，但有一个根本性的问题：调 LLM 本身就要花时间和钱。压缩一次对话可能要几秒钟、几千 token 的额外消耗。如果每轮对话结束都检查一遍是否需要压缩，Agent 的响应速度和成本都会受到影响。

实际上，大部分上下文膨胀不需要动用 LLM。你读了一个 5 万字符的文件，截断到 5000 就够了；你 10 分钟前查的 grep 结果，现在大概率已经没用了，直接清掉就行。这些操作都是纯字符串替换，零 LLM 成本，毫秒级完成。

这篇实现三层不需要调用 LLM 的即时防线。知识体系课把它们叫做"入口管理"——与其等上下文爆了再压缩，不如一开始就少放东西。

实战代码方面，这一节我们把上一篇的 compressor（LLM 压缩模块）替换成新的 defense 模块，同时去掉了之前用来演示的模拟 MCP 工具——这些在防线实战里用不到了，保持代码干净。

先装依赖：

bash
运行
复制
pnpm install

Layer 1：Token 估算——知道自己还剩多少空间

在做任何防御动作之前，你得先知道一个最基本的数据：当前上下文用了多少 token。

精确的 token 计数要等 API 返回 usage.prompt_tokens，但这只有调完 API 才知道。在调 API 之前，我们需要一个快速的估算方法来判断"要不要干预"。

业界通用的启发式方法是 4 个字符约等于 1 个 token。但中文要注意——中文 token 效率比英文低（1 个汉字大约 1.5-2 个 token），所以估算时加一个 1.2 倍的安全系数。

src/context/defense.ts
应用
复制
export class TokenTracker {
  private lastPreciseCount = 0;  // 上次 API 返回的精确值
  private pendingChars = 0;       // 新增消息的字符数

  updateFromAPI(promptTokens: number): void {
    this.lastPreciseCount = promptTokens;
    this.pendingChars = 0;  // 精确值到了，清零增量
  }

  addMessage(content: string): void {
    this.pendingChars += content.length;
  }

  get estimatedTokens(): number {
    return this.lastPreciseCount + Math.ceil(this.pendingChars / 4);
  }
}

export function estimateMessageTokens(messages: ModelMessage[]): number {
  let chars = 0;
  for (const msg of messages) {
    // ... 遍历所有消息内容，累加字符数 ...
  }
  return Math.ceil((chars / 4) * 1.2);  // 1.2x 中文安全系数
}


TokenTracker 的思路是精确基准 + 粗估增量：每次 API 调用返回时用 usage.prompt_tokens 这个精确值校准（updateFromAPI），中间新增的消息用 chars/4 粗估补上。不需要装 tokenizer 库，精度足够做决策。

你可能会问：为什么不每次都精确计算呢？

因为精确计算需要加载 tokenizer 模型文件（如 tiktoken），初始化就要几百毫秒，而且不同模型的 tokenizer 还不一样。chars/4 的误差大概在 10-20%，对于"要不要触发防御"这种二元判断完全够用了。OpenClaw 的压缩系统也用的是 chars/4，加了 1.2x 安全系数处理中文场景。

Layer 2：工具结果动态截断

Token 估算告诉你"空间紧不紧"，而截断会告诉你"超了怎么办"。

这里继续复用上一节的 tool-result-output.ts：toolResultOutputToText 负责读取 AI SDK 5 的工具结果，textToolResultOutput 负责把截断后的文本重新编码成合法的 ModelMessage。

前面 Tool System 那章我们实现了 truncateResult——固定的 Head/Tail 60/40 分割，意思是截断时保留前 60% 和后 40% 的内容，中间丢弃。

那个是注册时配置的静态截断（maxResultChars: 3000）。这一层做的是动态截断——根据当前上下文的使用率，实时调整截断阈值。

OpenClaw 的做法是双重约束：单个工具结果不超过上下文窗口的 50%，总上下文不超过 75%。我们也对齐这个做法——先按单条截断，如果总量还超预算，就从最老的工具结果开始逐条清理：

src/context/defense.ts
应用
复制
import { textToolResultOutput, toolResultOutputToText } from './tool-result-output.js';

const CONTEXT_WINDOW = 200_000;

export function truncateToolResults(
  messages: ModelMessage[],
  config = {
    maxSingleResult: CONTEXT_WINDOW * 0.5 * 2,  // 50% 窗口，2 chars/token
    contextBudgetChars: CONTEXT_WINDOW * 0.75 * 4, // 75% 窗口，4 chars/token
  },
): { messages: ModelMessage[]; truncated: number; compacted: number } {
  let truncated = 0;
  let compacted = 0;

  // Pass 1: 单条截断——超过窗口 50% 的工具结果做 Head/Tail 分割
  let result = messages.map(msg => {
    if (msg.role !== 'tool') return msg;
    const newContent = msg.content.map((part: any) => {
      if (!part.output) return part;
      const outputText = toolResultOutputToText(part.output);
      if (outputText.length <= config.maxSingleResult) return part;
      truncated++;
      const maxChars = config.maxSingleResult;
      const head = outputText.slice(0, Math.floor(maxChars * 0.6));
      const tail = outputText.slice(-Math.floor(maxChars * 0.4));
      return {
        ...part,
        output: textToolResultOutput(
          `${head}\n\n[truncated: ${outputText.length} → ${maxChars} chars]\n\n${tail}`,
        ),
      };
    });
    return { ...msg, content: newContent };
  });

  // Pass 2: 总量预算——如果总字符数还超 75%，从最老的 tool result 开始清理
  let totalChars = result.reduce((sum, msg) => {
    if (typeof msg.content === 'string') return sum + msg.content.length;
    if (Array.isArray(msg.content)) {
      return sum + msg.content.reduce((s: number, p: any) =>
        s + (p.output ? toolResultOutputToText(p.output).length : p.text?.length || 0), 0);
    }
    return sum;
  }, 0);

  if (totalChars > config.contextBudgetChars) {
    for (let i = 0; i < result.length && totalChars > config.contextBudgetChars; i++) {
      const msg = result[i];
      if (msg.role !== 'tool' || !Array.isArray(msg.content)) continue;
      const toolName = (msg.content[0] as any)?.toolName || 'unknown';
      const oldSize = msg.content.reduce((s: number, p: any) =>
        s + (p.output ? toolResultOutputToText(p.output).length : 0), 0);
      result[i] = { ...msg, content: msg.content.map((p: any) => ({
        ...p,
        output: textToolResultOutput(`[compacted: ${toolName} output removed to free context]`),
      })) };
      totalChars -= oldSize;
      compacted++;
    }
  }

  return { messages: result, truncated, compacted };
}


为什么是 Head/Tail 60/40 而不是只保留头部？知识体系课里解释过：文件尾部的信息往往比中间更有价值。日志文件的最新条目在尾部，代码文件的函数实现在尾部，命令输出的结论在尾部。只截头部会丢掉这些关键信息。

截断标记 [truncated: 80000 → 50000 chars] 也很重要——它告诉模型"这里有内容被截掉了"，模型可以根据需要重新读取完整内容。如果不加标记，模型可能以为它看到的就是全部，基于不完整的信息做出错误决策。

Layer 3：TTL 修剪——时间衰减

截断管的是"单条太大"的问题，TTL 管的是"老消息还留着干嘛"的问题。

核心洞察跟知识体系课讲的一样：老的工具结果几乎一定比新的更没用。你 5 分钟前读的文件内容，大概率已经不影响当前决策了。但直接删掉又会破坏对话结构。

TTL 修剪分两档：

软修剪（5 分钟）——保留头部和尾部各 1500 字符，中间替换成 [soft pruned] 标记。模型还能看到文件的开头和结尾，知道"这里有过一个工具结果"，但中间的大段内容不再占空间了。

硬清除（10 分钟）——整个工具结果替换成 [tool result expired: read_file]。只保留"发生过什么"的事实，内容全部清掉。

src/context/defense.ts
应用
复制
export function ttlPrune(
  messages: ModelMessage[],
  timestamps: Map<number, number>,  // 消息索引 → 创建时间戳
  config = { softTTLMs: 5 * 60_000, hardTTLMs: 10 * 60_000 },
): PruneResult {
  const now = Date.now();
  let softPruned = 0, hardPruned = 0;

  const result = messages.map((msg, idx) => {
    // 只修剪 tool 结果，user/assistant 消息永不修剪
    if (msg.role !== 'tool') return msg;

    const age = now - (timestamps.get(idx) || now);

    // 保留错误经验——失败的工具结果永不修剪
    const outputText = msg.content
      .map((p: any) => p.output ? toolResultOutputToText(p.output) : '').join('');
    if (/error|失败|不存在|denied|timeout/i.test(outputText)) return msg;

    if (age >= config.hardTTLMs) {
      hardPruned++;
      return {
        ...msg,
        content: msg.content.map((part: any) => ({
          ...part,
          output: textToolResultOutput('[tool result expired]'),
        })),
      };
    }

    if (age >= config.softTTLMs) {
      softPruned++;
      // 保留 head + tail，替换中间
      return softPruneMessage(msg, config.keepHeadTail);
    }

    return msg;
  });

  return { messages: result, softPruned, hardPruned };
}


只修剪 tool 结果，user/assistant 消息永不修剪——这是一个铁律。用户说过什么、模型回复过什么，永远保留。只有工具的返回值（文件内容、命令输出、搜索结果）才会被修剪。对话结构完整保留，模型知道"第 5 轮调用了 read_file 读了某个文件"，只是看不到文件内容了。需要的时候可以再读一次。

还有一个容易忽略的点：知识体系课强调过保留错误经验。如果某次工具调用失败了（比如文件不存在、命令执行报错），这个失败信息不应该被 TTL 清掉——模型需要记住"这条路走不通"，否则它会重复尝试同样的错误操作，浪费轮次和 token。上面的代码里已经实现了：TTL 修剪前先检查工具结果是否包含 error、失败、不存在 等错误关键词，命中的直接跳过不修剪。

三层联合防御

三层防线在每轮对话前按顺序执行：

src/index.ts
应用
复制
const defense = applyDefense(messages, timestamps);
messages = defense.messages;
console.log(`[Layer 2: 截断] ${defense.truncated} 个超长结果被截断`);
console.log(`[Layer 3: TTL] ${defense.softPruned} 个软修剪, ${defense.hardPruned} 个硬清除`);
console.log(`[Token] ~${defense.tokenEstimate} tokens`);


Apply 代码后跑一下。代码里预注入了 12 条模拟历史消息，时间跨度 12 分钟——最老的 12 分钟前（会被硬清除），中间的 7 分钟前（会被软修剪），最近的 1 分钟前（不动）：

bash
运行
复制
pnpm start

[Session] 新会话（已注入 12 条模拟历史，时间跨度 12 分钟）

=== 三层即时防线 ===
[防线前] 12 条消息, ~463 tokens
[Layer 2: 截断] 0 个超长结果被截断
[Layer 3: TTL] 0 个软修剪, 1 个硬清除
[防线后] 12 条消息, ~408 tokens (节省 55)
====================


12 分钟前的 read_file 结果被硬清除了（[tool result expired]），节省了 55 tokens。演示数据比较小所以节省不多，在真实场景下——一个 bash 命令输出 5000 token、一个文件读取 3000 token——TTL 修剪几轮就能回收上万 token，效果非常明显。

代码里内置了几个快捷命令，方便你直接体验防线效果：

You: sim           ← 注入 20 条模拟历史（含大量工具结果），~11000 tokens
You: status        ← 查看当前消息数和 token 估算
You: defend        ← 执行三层防线，看截断和修剪效果

--- 执行三层防线 ---
  [Layer 2] 截断: 0 条, 预算清理: 0 条
  [Layer 3] 软修剪: 1, 硬清除: 3
  [结果] ~11781 → ~3353 tokens (节省 8428)


一个 sim + defend 就能看到完整效果：11781 tokens 压到 3353，节省 72%，全部是零 LLM 成本的纯字符串操作。

与 LLM 压缩的配合关系

三层即时防线和上一篇的 LLM 摘要压缩不是互斥的，是分工配合的：

即时防线在每轮对话前自动执行，零 LLM 成本，毫秒级完成。它负责截断超大结果、清理过期内容、追踪 token 用量。

LLM 压缩只在即时防线不够用的时候才触发——上下文达到 75% 以上，即时防线已经清理了能清理的，但历史对话实在太多，只能调 LLM 做摘要。

整个防御体系的执行顺序是：截断（Layer 2）→ TTL 修剪（Layer 3）→ Token 估算（Layer 1，判断是否需要 LLM 压缩）→ 如果需要，触发 Microcompact → 如果还不够，触发 Summarization。从轻到重，能用简单手段解决的绝不上复杂方案。

知识体系课里 Claude Code 和 OpenClaw 的分层压缩策略，核心思路都是这个——先 Compaction 后 Summarization，先无损后有损。Claude Code 的 Microcompact 在 API 侧自动清理旧工具结果，OpenClaw 的 Tool Result Context Guard 在每次发送前做实时截断——实现细节不同，但背后的原则完全一致。

在一个典型的 50 轮编程对话中，工具结果占上下文的 60-80%。三层即时防线主要就是在对这 60-80% 动手——截断超大的、清理过期的、估算剩余的。对话历史本身（user/assistant 消息）通常只占 20-30%，那是 LLM 摘要压缩要处理的部分。

如此一来，你就有了处理长对话的 Agent 工程能力了，即使面对超长的对话记录，你也有充分的基础设施保证上下文在合适的时机压缩，让 Agent 长时间地运行下去。

到这里，回头看看我们做了什么：Session 持久化让对话跨终端保留，Prompt Pipe 让 system prompt 模块化可维护，LLM 摘要压缩解决对话太长的问题，三层即时防线在不调 LLM 的情况下做日常的上下文管理。

接下来我们进入到一个细节比较繁琐、很多人容易忽视，但又在生产环境非常重要的一个实践环节：Prompt Cache 与成本追踪，我们来分析一下各家模型厂商的 Cache 策略，以及如果来追踪我们 Agent 的成本，我们下一节再见👋🏻

参考链接
Anthropic - Effective Context Engineering
Manus - Context Engineering for AI Agents
上一篇
对话太长了怎么办——Microcompact + LLM 摘要压缩
下一篇 · 第三章：Context Engineering
让对话越来越便宜——Prompt Cache 与成本追踪
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
export class TokenTracker {
  private lastPreciseCount = 0;  // 上次 API 返回的精确值
  private pendingChars = 0;       // 新增消息的字符数

  updateFromAPI(promptTokens: number): void {
    this.lastPreciseCount = promptTokens;
    this.pendingChars = 0;  // 精确值到了，清零增量
  }

  addMessage(content: string): void {
    this.pendingChars += content.length;
  }

  get estimatedTokens(): number {
    return this.lastPreciseCount + Math.ceil(this.pendingChars / 4);
  }
}

export function estimateMessageTokens(messages: ModelMessage[]): number {
  let chars = 0;
  for (const msg of messages) {
    // ... 遍历所有消息内容，累加字符数 ...
  }
  return Math.ceil((chars / 4) * 1.2);  // 1.2x 中文安全系数
}
```


```typescript
import { textToolResultOutput, toolResultOutputToText } from './tool-result-output.js';

const CONTEXT_WINDOW = 200_000;

export function truncateToolResults(
  messages: ModelMessage[],
  config = {
    maxSingleResult: CONTEXT_WINDOW * 0.5 * 2,  // 50% 窗口，2 chars/token
    contextBudgetChars: CONTEXT_WINDOW * 0.75 * 4, // 75% 窗口，4 chars/token
  },
): { messages: ModelMessage[]; truncated: number; compacted: number } {
  let truncated = 0;
  let compacted = 0;

  // Pass 1: 单条截断——超过窗口 50% 的工具结果做 Head/Tail 分割
  let result = messages.map(msg => {
    if (msg.role !== 'tool') return msg;
    const newContent = msg.content.map((part: any) => {
      if (!part.output) return part;
      const outputText = toolResultOutputToText(part.output);
      if (outputText.length <= config.maxSingleResult) return part;
      truncated++;
      const maxChars = config.maxSingleResult;
      const head = outputText.slice(0, Math.floor(maxChars * 0.6));
      const tail = outputText.slice(-Math.floor(maxChars * 0.4));
      return {
        ...part,
        output: textToolResultOutput(
          `${head}\n\n[truncated: ${outputText.length} → ${maxChars} chars]\n\n${tail}`,
        ),
      };
    });
    return { ...msg, content: newContent };
  });

  // Pass 2: 总量预算——如果总字符数还超 75%，从最老的 tool result 开始清理
  let totalChars = result.reduce((sum, msg) => {
    if (typeof msg.content === 'string') return sum + msg.content.length;
    if (Array.isArray(msg.content)) {
      return sum + msg.content.reduce((s: number, p: any) =>
        s + (p.output ? toolResultOutputToText(p.output).length : p.text?.length || 0), 0);
    }
    return sum;
  }, 0);

  if (totalChars > config.contextBudgetChars) {
    for (let i = 0; i < result.length && totalChars > config.contextBudgetChars; i++) {
      const msg = result[i];
      if (msg.role !== 'tool' || !Array.isArray(msg.content)) continue;
      const toolName = (msg.content[0] as any)?.toolName || 'unknown';
      const oldSize = msg.content.reduce((s: number, p: any) =>
        s + (p.output ? toolResultOutputToText(p.output).length : 0), 0);
      result[i] = { ...msg, content: msg.content.map((p: any) => ({
        ...p,
        output: textToolResultOutput(`[compacted: ${toolName} output removed to free context]`),
      })) };
      totalChars -= oldSize;
      compacted++;
    }
  }

  return { messages: result, truncated, compacted };
}
```


```typescript
export function ttlPrune(
  messages: ModelMessage[],
  timestamps: Map<number, number>,  // 消息索引 → 创建时间戳
  config = { softTTLMs: 5 * 60_000, hardTTLMs: 10 * 60_000 },
): PruneResult {
  const now = Date.now();
  let softPruned = 0, hardPruned = 0;

  const result = messages.map((msg, idx) => {
    // 只修剪 tool 结果，user/assistant 消息永不修剪
    if (msg.role !== 'tool') return msg;

    const age = now - (timestamps.get(idx) || now);

    // 保留错误经验——失败的工具结果永不修剪
    const outputText = msg.content
      .map((p: any) => p.output ? toolResultOutputToText(p.output) : '').join('');
    if (/error|失败|不存在|denied|timeout/i.test(outputText)) return msg;

    if (age >= config.hardTTLMs) {
      hardPruned++;
      return {
        ...msg,
        content: msg.content.map((part: any) => ({
          ...part,
          output: textToolResultOutput('[tool result expired]'),
        })),
      };
    }

    if (age >= config.softTTLMs) {
      softPruned++;
      // 保留 head + tail，替换中间
      return softPruneMessage(msg, config.keepHeadTail);
    }

    return msg;
  });

  return { messages: result, softPruned, hardPruned };
}
```


```typescript
const defense = applyDefense(messages, timestamps);
messages = defense.messages;
console.log(`[Layer 2: 截断] ${defense.truncated} 个超长结果被截断`);
console.log(`[Layer 3: TTL] ${defense.softPruned} 个软修剪, ${defense.hardPruned} 个硬清除`);
console.log(`[Token] ~${defense.tokenEstimate} tokens`);
```


```bash
pnpm start
```


```
[Session] 新会话（已注入 12 条模拟历史，时间跨度 12 分钟）

=== 三层即时防线 ===
[防线前] 12 条消息, ~463 tokens
[Layer 2: 截断] 0 个超长结果被截断
[Layer 3: TTL] 0 个软修剪, 1 个硬清除
[防线后] 12 条消息, ~408 tokens (节省 55)
====================
```


```
You: sim           ← 注入 20 条模拟历史（含大量工具结果），~11000 tokens
You: status        ← 查看当前消息数和 token 估算
You: defend        ← 执行三层防线，看截断和修剪效果

--- 执行三层防线 ---
  [Layer 2] 截断: 0 条, 预算清理: 0 条
  [Layer 3] 软修剪: 1, 硬清除: 3
  [结果] ~11781 → ~3353 tokens (节省 8428)
```
