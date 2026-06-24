# Context Engineering 详解

## ORRIC 五维框架

| 维度 | 手段 | 目的 |
|------|------|------|
| **O**ffload | 写文件/DB，按需取回 | 减少上下文占用 |
| **R**educe | 压缩（三层） | 缩短已有上下文 |
| **R**etrieve | JIT 加载（三路线） | 只在需要时加载信息 |
| **I**solate | 子 Agent 独立上下文 | 防止相互干扰 |
| **C**ache | 稳定前缀 | 降低推理成本 |

## 三层压缩策略（按成本递增）

```
Layer 1: Microcompact — 截断工具调用结果（零 LLM 成本）
         大结果 → 保留前 2KB + 后 500B，标注省略量
         工具结果通常占上下文 60-80%，优先压缩

Layer 2: Snip — 修剪过期 TTL 消息
         为每条工具结果设置 TTL，过期后自动替换为摘要占位符

Layer 3: LLM 摘要 — 对话历史 LLM 压缩（最后手段）
         保留最近 4 条消息，其余用 LLM 摘要替换
         ⚠ 会丢失原始结构，且要保留错误信息（不能只保留成功路径）
```

## Prompt Pipe（模块化 System Prompt）

```typescript
class PromptBuilder {
  private pipes: ((ctx: PromptContext) => string | null)[] = [];
  pipe(fn: (ctx: PromptContext) => string | null) { this.pipes.push(fn); return this; }
  build(ctx: PromptContext) {
    return this.pipes.map(fn => fn(ctx)).filter(Boolean).join('\n\n');
  }
}

// 静态前缀在前（利于 Cache），动态内容追加到末尾
const prompt = new PromptBuilder()
  .pipe(() => ROLE_DEFINITION)           // 静态，永远缓存
  .pipe(() => TOOL_USAGE_GUIDE)          // 静态，永远缓存
  .pipe(ctx => ctx.skills.join('\n'))    // 动态，按需加载
  .pipe(ctx => ctx.memoryContext)        // 动态，每次不同
  .build(ctx);
```

## JIT 三路线选择

| 场景 | 路线 | 原因 |
|------|------|------|
| 代码库探索、任务结构不确定 | Agentic Search | Agent 自主决定搜什么 |
| 有结构化知识库 | RAG | 语义 + 关键词混合检索 |
| 长链路任务中间状态 | Context Offloading | 写文件，用时取回 |

## Prompt Cache 最佳实践

- **Cache 杀手**：system prompt 含时间戳、工具列表每轮变化
- 静态部分放最前面，动态部分（当前时间、会话状态）追加末尾
- Claude 显式标记 `cache_control: { type: "ephemeral" }`，OpenAI/DeepSeek 隐式缓存
- 最大 Cache 收益：system prompt + 工具定义 + 早期消息（占 80%+ 成本）

## Token 估算（无需 tokenizer 库）

```typescript
function estimateTokens(messages: Message[]): number {
  // 精确基准：上次 API 返回的 usage.totalTokens
  // 增量：新消息用 chars/4 粗估
  const recentChars = messages.slice(lastCheckpoint).reduce((s, m) => s + JSON.stringify(m).length, 0);
  return lastKnownTokens + Math.ceil(recentChars / 4);
}
```

## Context Rot 防御

- 不要在 system prompt 里塞大量可能过期的信息
- 过长对话中模型会"偷懒"，最好的压缩是一开始就不塞无用信息
- Skills 三级按需加载：frontmatter（always）→ 完整内容（on demand）→ reference 文件（deep dive）
