# Agent Loop 详解

## 基础结构

```typescript
async function agentLoop({ model, system, messages, tools, maxSteps = 30, budgetTokens = 200_000 }) {
  let totalTokens = 0;
  for (let step = 0; step < maxSteps; step++) {
    // 90% token nudge
    if (totalTokens > budgetTokens * 0.9) {
      messages.push({ role: 'user', content: '请完成当前任务并给出最终结果，不要继续工具调用。' });
    }
    const response = await callWithRetry(() => generateText({ model, system, messages, tools }));
    totalTokens += response.usage.totalTokens;
    if (!response.toolCalls?.length) return response.text; // end_turn
    const results = await executeTools(response.toolCalls, tools);
    messages.push({ role: 'assistant', content: response.content });
    messages.push({ role: 'tool', content: results });
  }
  throw new Error('Max steps exceeded');
}
```

## 死循环检测（三级响应）

```typescript
class LoopDetector {
  private history: string[] = [];
  record(name: string, args: unknown) {
    this.history.push(sha256(name + JSON.stringify(args)));
    if (this.history.length > 30) this.history.shift();
  }
  check(): 'ok' | 'warn' | 'critical' | 'break' {
    const last5 = this.history.slice(-5);
    if (new Set(last5).size === 1 && last5.length === 5) return 'warn';
    const last8 = this.history.slice(-8);
    if (new Set(last8).size === 1 && last8.length === 8) return 'critical';
    const last10 = this.history.slice(-10);
    if (new Set(last10).size === 1 && last10.length === 10) return 'break';
    return 'ok';
  }
}
```

| 级别 | 触发 | 响应 |
|------|------|------|
| warn | 连续 5 次相同 | 注入 nudge，继续 |
| critical | 连续 8 次相同 | 强制要求换路径 |
| break | 连续 10 次相同 | 硬停止，返回错误 |

## API 容错（指数退避）

```typescript
async function callWithRetry(fn, { maxRetries = 3, baseDelay = 1000 } = {}) {
  for (let i = 0; i <= maxRetries; i++) {
    try { return await fn(); }
    catch (e) {
      if (!isRetryable(e) || i === maxRetries) throw e;
      await sleep(baseDelay * 2 ** i + Math.random() * 500); // jitter
    }
  }
}

function isRetryable(e: any) {
  return e.status === 429 || e.status === 500 || e.status === 503 || e.code === 'ECONNRESET';
}
```

- 优先读 `Retry-After` 响应头
- 429/5xx 可重试，400/401 不可重试
- 生产级：多 Provider 兄弟模型 Failover

## 流式输出（"边说边执行"）

```typescript
const stream = streamText({ model, system, messages, tools });
for await (const event of stream) {
  if (event.type === 'text-delta') process.stdout.write(event.textDelta);
  if (event.type === 'tool-call-streaming-start') { /* 准备执行 */ }
  if (event.type === 'tool-call') { /* 参数攒齐，立即执行，不等整条消息 */ }
}
```

工具块完成即执行，不等整条 assistant 消息 —— 这是"边说边执行"的关键。
