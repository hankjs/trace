# 对话太长了怎么办——Microcompact + LLM 摘要压缩

课程
Super Agent 实战课
对话太长了怎么办——Microcompact + LLM 摘要压缩
对话太长了怎么办——Microcompact + LLM 摘要压缩

约 21 分钟

本节示例推荐使用真实的大模型 API Key，填入到 .env 文件。API Key 从阿里云百炼平台获取，免费额度足够完成本课程的所有练习。

上一篇做了 Session 持久化，对话能存了。但你有没有想过一个问题：Agent 跑了 50 轮之后，消息列表有多大？

200K 的上下文窗口，system prompt 吃 5K，工具定义吃 10K，50 次工具调用每次平均返回 2000 token——光工具结果就 100K。还没算对话历史和模型回复。上下文快满了，要么 API 直接报错，要么模型的推理质量因为注意力分散而断崖式下降。

所以压缩不是可选的，是必须的。问题是怎么来压。

知识体系课把压缩分成了两种本质不同的策略：

Compaction（紧凑化） 不改对话结构，只缩小内容，比如移除某些比较大的工具调用的内容；
Summarization（摘要化） 用 LLM 生成摘要替换整段对话，原始的这段对话内容会丢失。

我们的原则是先 Compaction 后 Summarization——能不丢结构就不丢。

这篇我们把这两层都实现出来。

先装依赖：

bash
运行
复制
pnpm install

上下文里什么最占空间

在开始写代码之前，先搞清楚要压什么。

上下文里有三类内容：System Prompt（身份、规则、工具定义）、对话历史（用户消息 + 模型回复）、工具调用记录（调用参数 + 返回结果）。

System Prompt 不能压——压了模型就不知道自己是谁了、指令也给忘了。

能压的是对话历史和工具调用记录。而工具调用记录通常是大头——你读一个文件返回 3000 token，grep 一下返回 2000 token，跑个命令返回 5000 token。50 轮下来，工具结果占上下文的 60-80% 并不夸张。

理解了这个，后面的设计就很自然了：先清理旧的工具结果（Microcompact），不够再调 LLM 做摘要（Summarization）。

Layer 1：Microcompact——清理旧工具结果

这是最轻的一层。不删消息、不改对话结构，只是把旧的工具结果替换成占位符。

你在第 3 轮读了一个文件，返回了 3000 token 的内容。到了第 30 轮，这个文件内容大概率已经没用了——模型后续的决策不再依赖它。但直接删这条消息会破坏对话结构（模型会看到一个工具调用但没有结果）。

Microcompact 的做法是：保留消息，替换内容。把 3000 token 的文件内容替换成 [tool result cleared]，token 占用从 3000 降到不到 10。

AI SDK 5 的 ModelMessage 不再允许把工具结果直接写成字符串，而是使用带 type 的判别联合。先集中处理文本结果的编码与读取，后面的压缩、截断和 TTL 清理都会复用它：

src/context/tool-result-output.ts
应用
复制
import type { ToolResultPart } from 'ai';

type ToolResultOutput = ToolResultPart['output'];

export function textToolResultOutput(value: string): ToolResultOutput {
  return { type: 'text', value };
}

export function toolResultOutputToText(output: ToolResultOutput): string {
  switch (output.type) {
    case 'text':
    case 'error-text':
      return output.value;
    case 'json':
    case 'error-json':
      return JSON.stringify(output.value);
    case 'content':
      return output.value
        .map(part => part.type === 'text' ? part.text : `[media: ${part.mediaType}]`)
        .join('\n');
  }
}

src/context/compressor.ts
应用
复制
import { textToolResultOutput } from './tool-result-output.js';

const CLEARABLE_TOOLS = new Set([
  'read_file', 'bash', 'grep', 'glob', 'list_directory',
  'edit_file', 'write_file',
]);
const KEEP_RECENT_TOOL_RESULTS = 3;

export function microcompact(messages: ModelMessage[]): {
  messages: ModelMessage[];
  cleared: number;
} {
  // 找到所有 tool result 消息的位置
  const toolResultIndices: number[] = [];
  for (let i = 0; i < messages.length; i++) {
    if (messages[i].role === 'tool') toolResultIndices.push(i);
  }

  // 保留最近 N 个工具结果不动，只清理更早的
  const toClear = toolResultIndices.slice(
    0, Math.max(0, toolResultIndices.length - KEEP_RECENT_TOOL_RESULTS)
  );

  let cleared = 0;
  const result = messages.map((msg, idx) => {
    if (!toClear.includes(idx)) return msg;
    if (!CLEARABLE_TOOLS.has(toolName)) return msg;

    cleared++;
    return {
      ...msg,
      content: msg.content.map(part => ({
        ...part, output: textToolResultOutput('[tool result cleared]'),
      })),
    };
  });

  return { messages: result, cleared };
}


两个关键设计：

CLEARABLE_TOOLS 白名单——只清理"查询类"工具的结果。read_file、bash、grep 这些，它们的返回值是一次性的。如果你定义了一个 create_issue 工具，它的返回值（新 Issue 的 ID）可能后续还要用，不能清理。

KEEP_RECENT_TOOL_RESULTS = 3——保留最近 3 个工具结果不动。因为最近几轮的结果很可能还在被模型引用——你刚读的文件、刚跑的命令，模型下一步可能还要用。Claude Code 也是这个思路，只清理"足够老"的结果。

Layer 2：LLM 摘要压缩

如果 Microcompact 之后上下文还是太大，上第二层——调 LLM 把早期对话压缩成一段结构化摘要。

这一层的核心在于压缩 Prompt 怎么写。一个好的压缩 Prompt 要解决三个问题：

保什么——不是让模型自由发挥写摘要，而是给一个明确的模板让它填
不保什么——笼统的概述没用，只保留具体的、可操作的信息
标识符保护——文件路径、UUID、版本号这些不能被模型"翻译"或改写

我们用的压缩 Prompt 长这样：

src/context/compressor.ts
应用
复制
const COMPRESS_PROMPT = `你是一个对话压缩系统。你的任务是把 Agent 和用户之间的
对话历史压缩成一份结构化摘要，确保后续对话能够无缝继续。

请严格按照以下模板输出，每个字段都要填写：

## 用户意图
（用户在这次对话中想要完成什么）

## 已完成的操作
（Agent 执行了哪些工具调用、产生了什么结果）

## 关键发现
（读取的文件内容要点、搜索结果、命令输出中的关键信息）

## 当前状态
（对话进行到哪一步了、还有什么没做完）

## 需要保留的细节
（文件路径、变量名、配置值、错误信息等不能丢失的具体内容）

注意事项：
- 用对话中使用的语言输出
- 文件路径、UUID、版本号等标识符必须原样保留，不要翻译或改写
- 不要写笼统的概述，只保留具体的、可操作的信息
- 总长度控制在 800 字以内`;


这段用英文来写 Prompt 也完全没问题。

这个 Prompt 的设计思路参考了 Sitor 产品的实现——它在教学场景下需要保留"学到了什么"、"在做什么练习"、"哪里卡住了"这些信息。你根据自己的业务场景调整模板字段就行，比如编程场景可能要加"代码改动记录"，客服场景要加"用户情绪状态"。

这也是 Manus 对外分享里面提到的一个如何保证压缩质量的最佳实践。

核心原则是：给模型一个表格让它填，而不是让它自由写作。 模板越具体，压缩结果越稳定。自由写作的摘要每次输出格式都不一样，模板化的摘要次次稳定，后续对话也更容易利用。

压缩的触发和执行逻辑：

src/context/compressor.ts
应用
复制
export async function summarize(
  model: any,
  messages: ModelMessage[],
  existingSummary?: string,
): Promise<CompactionResult> {
  const tokenEstimate = estimateTokens(messages);
  if (tokenEstimate < CONTEXT_TOKEN_THRESHOLD) {
    return { messages, summary: existingSummary || '', compressedCount: 0 };
  }

  // 保留最近 N 条消息，对齐到 user 消息边界
  const splitIdx = Math.max(0, messages.length - KEEP_RECENT_MESSAGES);
  // ... 对齐逻辑 ...

  const toCompress = messages.slice(0, alignedIdx);
  const toKeep = messages.slice(alignedIdx);

  // 如果有上一次的摘要，合并进去一起压缩
  const userPrompt = existingSummary
    ? `## 已有摘要\n\n${existingSummary}\n\n## 新对话\n\n${conversationText}`
    : conversationText;

  const { text: summary } = await generateText({
    model, system: COMPRESS_PROMPT, prompt: userPrompt,
  });

  // 摘要作为第一条消息，后面跟着保留的最近对话
  const summaryMessage: ModelMessage = {
    role: 'user',
    content: `[以下是之前对话的压缩摘要]\n\n${summary}\n\n[摘要结束]`,
  };

  return {
    messages: [summaryMessage, ...toKeep],
    summary,
    compressedCount: toCompress.length,
  };
}


整体流程看清楚之后，我希望你能够再次注意几个细节：

对齐到 user 消息边界——切分点一定不能落在 assistant 或 tool 消息上，否则保留的消息列表会以非 user 开头，很多 LLM API 会报错。注意要从切分点往前找到最近的 user 消息再切。

已有摘要合并——如果之前已经压缩过一次，新的压缩会把旧摘要和新对话一起传给 LLM。这样摘要是累积的，不会因为多次压缩而丢失最早期的信息。Claude Code 的 Auto-compact 也用了同样的策略。

摘要作为 user 消息注入——压缩后的摘要放在消息列表最前面，角色是 user。模型看到这条消息就知道"之前有过对话，这是摘要"，可以基于摘要继续工作。

集成到 Agent Loop

入口文件里把两层压缩串起来——启动时先跑一遍压缩（处理恢复的历史消息），每轮对话结束后检查是否需要再次压缩：

src/index.ts
应用
复制
// 启动时压缩
const mc = microcompact(messages);
messages = mc.messages;
console.log(`[Layer 1: Microcompact] 清理了 ${mc.cleared} 个工具结果`);

const compResult = await summarize(model, messages, summary);
messages = compResult.messages;
summary = compResult.summary;
if (compResult.compressedCount > 0) {
  console.log(`[Layer 2: Summarization] 压缩了 ${compResult.compressedCount} 条消息`);
}

// 每轮对话后检查
const currentTokens = estimateTokens(messages);
if (currentTokens > THRESHOLD) {
  // 再次执行 microcompact + summarize
}


代码里预注入了 16 条模拟历史消息（4 轮工具调用），Apply 后跑一下：

bash
运行
复制
pnpm start

[Session] 新会话（已注入 16 条模拟历史）

[压缩前] 16 条消息, ~416 tokens
[Layer 1: Microcompact] 清理了 1 个工具结果, ~394 tokens
[Layer 2: Summarization] 压缩了 8 条消息, ~425 tokens
[摘要预览] ## 用户意图
用户在探索项目结构和代码，了解工具系统的设计。

## 已完成的操作
- 列出了当前目录文件（.env, package.json, sample-data.txt, src/）
- 读取了 package.json（项目名 super-agent-08-compaction, 版本...
[压缩后] 9 条消息, ~425 tokens


16 条消息压缩成了 9 条（1 条摘要 + 8 条保留的最近消息）。Microcompact 先清理了 1 个旧工具结果，然后 Summarization 把前 8 条消息压缩成了一段结构化摘要。

摘要的质量直接取决于你的 COMPRESS_PROMPT 写得好不好。几个有用实战原则跟大家分享一下：

模板字段要贴合你的业务场景。如果你的 Agent 是做代码审查的，模板里应该有"审查过的文件"、"发现的问题"、"修复建议"。如果是做客服的，应该有"用户诉求"、"已尝试的解决方案"、"当前情绪"。Claude Code 的摘要模板有 9 个字段（用户意图、技术概念、文件改动、错误修复等），完全针对编程场景设计。没有万能模板，必须根据场景定制。

字数限制很重要。不设上限的话，模型可能生成一个比原始对话还长的"摘要"，压缩变成了膨胀。我们设了 800 字的上限，Sitor 的教学场景设的是 1500 words，根据你的上下文窗口大小来定。

"不要什么"比"要什么"更重要。Prompt 里明确说"不要写笼统的概述"，否则模型会输出"用户进行了一系列操作"这种没有任何信息量的句子。

压缩的稳定性保障

最后，关于如何保证压缩的稳定性，其实也有一些很容易踩的坑，在这里我们来梳理一下。

首先是标识符可能被模型改写。你的对话里有 src/tool-registry.ts 这个路径，模型在摘要里可能把它写成"工具注册文件"。后续对话模型就找不到这个文件了。解决办法就是在 Prompt 里明确要求"文件路径、UUID、版本号等标识符必须原样保留"。

压缩有失败的可能性。网络问题、模型超时等等原因都可能导致失败。我们在 summarize 里用了 try-catch，失败了就返回原始消息列表，不做任何压缩。Claude Code 的 Auto-compact 连续失败 3 次后也会放弃，不再尝试。压缩失败不能影响 Agent 的正常工作。

用便宜的模型做压缩——压缩本身也要消耗 token，用主力模型做压缩太贵了，Claude Code 用的也不是 Opus 而是更轻量的模型。压缩不需要复杂推理能力，一个小模型按模板填表就够了。

何时触发——阈值设太低会频繁压缩（浪费 LLM 调用），设太高可能来不及压缩就溢出了。Claude Code 的阈值大约在上下文窗口的 87%，我们的演示用了 300 tokens 方便看到效果。生产环境根据你的上下文窗口大小来设——200K 窗口设 170K~180K 左右比较合理。

到这里，两层压缩都落地了。Microcompact 零成本清理旧工具结果，LLM Summarization 在必要时生成结构化摘要。下一篇我们继续往下挖——Token 估算、工具截断和 TTL 修剪，三层不需要调用 LLM 的防线，比摘要压缩更轻量、更快速。

参考链接
Anthropic - Effective Context Engineering
Anthropic - Prompt Caching
上一篇
Session 持久化 + Prompt Pipe——对话存档与模块化 Prompt 组装
下一篇 · 第三章：Context Engineering
三层即时防线——Token 估算、工具截断与 TTL 修剪
编辑器


---
## 代码块


```bash
pnpm install
```


```typescript
import type { ToolResultPart } from 'ai';

type ToolResultOutput = ToolResultPart['output'];

export function textToolResultOutput(value: string): ToolResultOutput {
  return { type: 'text', value };
}

export function toolResultOutputToText(output: ToolResultOutput): string {
  switch (output.type) {
    case 'text':
    case 'error-text':
      return output.value;
    case 'json':
    case 'error-json':
      return JSON.stringify(output.value);
    case 'content':
      return output.value
        .map(part => part.type === 'text' ? part.text : `[media: ${part.mediaType}]`)
        .join('\n');
  }
}
```


```typescript
import { textToolResultOutput } from './tool-result-output.js';

const CLEARABLE_TOOLS = new Set([
  'read_file', 'bash', 'grep', 'glob', 'list_directory',
  'edit_file', 'write_file',
]);
const KEEP_RECENT_TOOL_RESULTS = 3;

export function microcompact(messages: ModelMessage[]): {
  messages: ModelMessage[];
  cleared: number;
} {
  // 找到所有 tool result 消息的位置
  const toolResultIndices: number[] = [];
  for (let i = 0; i < messages.length; i++) {
    if (messages[i].role === 'tool') toolResultIndices.push(i);
  }

  // 保留最近 N 个工具结果不动，只清理更早的
  const toClear = toolResultIndices.slice(
    0, Math.max(0, toolResultIndices.length - KEEP_RECENT_TOOL_RESULTS)
  );

  let cleared = 0;
  const result = messages.map((msg, idx) => {
    if (!toClear.includes(idx)) return msg;
    if (!CLEARABLE_TOOLS.has(toolName)) return msg;

    cleared++;
    return {
      ...msg,
      content: msg.content.map(part => ({
        ...part, output: textToolResultOutput('[tool result cleared]'),
      })),
    };
  });

  return { messages: result, cleared };
}
```


```typescript
const COMPRESS_PROMPT = `你是一个对话压缩系统。你的任务是把 Agent 和用户之间的
对话历史压缩成一份结构化摘要，确保后续对话能够无缝继续。

请严格按照以下模板输出，每个字段都要填写：

## 用户意图
（用户在这次对话中想要完成什么）

## 已完成的操作
（Agent 执行了哪些工具调用、产生了什么结果）

## 关键发现
（读取的文件内容要点、搜索结果、命令输出中的关键信息）

## 当前状态
（对话进行到哪一步了、还有什么没做完）

## 需要保留的细节
（文件路径、变量名、配置值、错误信息等不能丢失的具体内容）

注意事项：
- 用对话中使用的语言输出
- 文件路径、UUID、版本号等标识符必须原样保留，不要翻译或改写
- 不要写笼统的概述，只保留具体的、可操作的信息
- 总长度控制在 800 字以内`;
```


```typescript
export async function summarize(
  model: any,
  messages: ModelMessage[],
  existingSummary?: string,
): Promise<CompactionResult> {
  const tokenEstimate = estimateTokens(messages);
  if (tokenEstimate < CONTEXT_TOKEN_THRESHOLD) {
    return { messages, summary: existingSummary || '', compressedCount: 0 };
  }

  // 保留最近 N 条消息，对齐到 user 消息边界
  const splitIdx = Math.max(0, messages.length - KEEP_RECENT_MESSAGES);
  // ... 对齐逻辑 ...

  const toCompress = messages.slice(0, alignedIdx);
  const toKeep = messages.slice(alignedIdx);

  // 如果有上一次的摘要，合并进去一起压缩
  const userPrompt = existingSummary
    ? `## 已有摘要\n\n${existingSummary}\n\n## 新对话\n\n${conversationText}`
    : conversationText;

  const { text: summary } = await generateText({
    model, system: COMPRESS_PROMPT, prompt: userPrompt,
  });

  // 摘要作为第一条消息，后面跟着保留的最近对话
  const summaryMessage: ModelMessage = {
    role: 'user',
    content: `[以下是之前对话的压缩摘要]\n\n${summary}\n\n[摘要结束]`,
  };

  return {
    messages: [summaryMessage, ...toKeep],
    summary,
    compressedCount: toCompress.length,
  };
}
```


```typescript
// 启动时压缩
const mc = microcompact(messages);
messages = mc.messages;
console.log(`[Layer 1: Microcompact] 清理了 ${mc.cleared} 个工具结果`);

const compResult = await summarize(model, messages, summary);
messages = compResult.messages;
summary = compResult.summary;
if (compResult.compressedCount > 0) {
  console.log(`[Layer 2: Summarization] 压缩了 ${compResult.compressedCount} 条消息`);
}

// 每轮对话后检查
const currentTokens = estimateTokens(messages);
if (currentTokens > THRESHOLD) {
  // 再次执行 microcompact + summarize
}
```


```bash
pnpm start
```


```
[Session] 新会话（已注入 16 条模拟历史）

[压缩前] 16 条消息, ~416 tokens
[Layer 1: Microcompact] 清理了 1 个工具结果, ~394 tokens
[Layer 2: Summarization] 压缩了 8 条消息, ~425 tokens
[摘要预览] ## 用户意图
用户在探索项目结构和代码，了解工具系统的设计。

## 已完成的操作
- 列出了当前目录文件（.env, package.json, sample-data.txt, src/）
- 读取了 package.json（项目名 super-agent-08-compaction, 版本...
[压缩后] 9 条消息, ~425 tokens
```
