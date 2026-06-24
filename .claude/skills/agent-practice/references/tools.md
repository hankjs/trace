# Tool System 详解

## 工具定义接口

```typescript
interface ToolDefinition {
  name: string;
  description: string;        // 给模型看的，影响选择准确率
  parameters: JSONSchema;
  execute: (input: any) => Promise<string>;
  isConcurrencySafe?: boolean; // 只读工具可并发
  isReadOnly?: boolean;
  maxOutputLength?: number;    // 必须设置！
  shouldDefer?: boolean;       // 工具太多时延迟加载
}
```

## 7 步执行管线

```
参数格式验证 → 业务校验 → 参数标准化 → Pre-Hook → 权限检查 → 执行 → Post-Hook → 结果截断
```

## ToolRegistry

```typescript
class ToolRegistry {
  private tools = new Map<string, ToolDefinition>();
  register(tool: ToolDefinition) { this.tools.set(tool.name, tool); }

  async execute(name: string, args: unknown) {
    const tool = this.tools.get(name);
    if (!tool) return `Error: tool "${name}" not found`;
    try {
      const result = await tool.execute(args);
      return truncate(result, tool.maxOutputLength ?? 10_000);
    } catch (e) {
      // 面向模型的错误：含纠错上下文
      return `Error executing ${name}: ${e.message}. 建议：检查参数 ${JSON.stringify(args)} 是否正确。`;
    }
  }
}

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  const head = text.slice(0, max * 0.6);
  const tail = text.slice(-(max * 0.4));
  return `${head}\n...[省略 ${text.length - max} 字符]...\n${tail}`;
}
```

## 并发执行（读写分离）

```typescript
async function executeTools(calls: ToolCall[], registry: ToolRegistry) {
  const safe = calls.filter(c => registry.get(c.name)?.isConcurrencySafe);
  const unsafe = calls.filter(c => !registry.get(c.name)?.isConcurrencySafe);

  const safeResults = await Promise.all(safe.map(c => registry.execute(c.name, c.args)));
  const unsafeResults: string[] = [];
  for (const c of unsafe) unsafeResults.push(await registry.execute(c.name, c.args));
  // 按原始顺序合并
  return calls.map(c => safe.includes(c) ? safeResults.shift()! : unsafeResults.shift()!);
}
```

## 标准工具集（代码 Agent）

| 工具 | 要点 |
|------|------|
| `read_file` | 大文件截断，按需分页 |
| `edit_file` | 精确字符串替换（old→new），不全量覆写 |
| `write_file` | 仅用于新建文件 |
| `grep` | 正则匹配，返回行号 |
| `glob` | 模式匹配，返回排序路径 |
| `bash` | 白名单 + 超时 + 输出截断 |
| `list_directory` | 浅层列表 |

## Deferred Loading（工具超阈值时）

- 工具 >15 个时模型选择准确率开始下降，70 个工具定义可占 3~5 万 token
- 不常用工具标记 `shouldDefer: true`，注册为 stub（只有名称描述）
- 提供 `tool_search` 工具让模型按需发现完整 schema
- 首次调用时动态注入完整定义，并从 deferred 池移除

## Mask Don't Remove

禁用工具时用空 schema 替换（mask），不直接删除——删除会破坏 KV Cache，让历史 token 缓存失效。
