# 权限与安全详解

## 四层防线

```
Layer 1: 模式（Mode）
  - plan 模式：只读，禁止写操作
  - default 模式：写操作需确认
  - full 模式：自动允许（需明确授权）

Layer 2: 规则（Rules）
  - alwaysAllow：白名单，直接放行
  - alwaysDeny：黑名单，永远拒绝
  - alwaysAsk：每次都问用户

Layer 3: 危险模式识别（Pattern Detection）
  - 正则匹配危险命令：rm -rf, sudo, curl | sh, chmod 777
  - LLM 分类器兜底（处理 shell 注入、间接危险）

Layer 4: 交互确认（Interactive Approval）
  - 未命中前三层的未知操作 → 展示给用户确认
  - 限制审批疲劳：聚合相似请求，批量审批
```

## 权限检查实现

```typescript
type PermissionResult = 'allow' | 'deny' | 'ask';

async function checkPermission(tool: string, args: unknown, mode: Mode): Promise<PermissionResult> {
  if (mode === 'plan' && !isReadOnly(tool)) return 'deny';
  if (ALWAYS_DENY.some(p => matchPattern(tool, args, p))) return 'deny';
  if (ALWAYS_ALLOW.some(p => matchPattern(tool, args, p))) return 'allow';
  if (isDangerous(tool, args)) return mode === 'full' ? 'allow' : 'ask';
  return mode === 'full' ? 'allow' : 'ask';
}
```

## 沙箱路径校验

```typescript
function validatePath(inputPath: string, workspaceRoot: string): string {
  const resolved = path.resolve(workspaceRoot, inputPath);
  if (!resolved.startsWith(workspaceRoot)) {
    throw new Error(`路径穿越攻击：${inputPath} 超出工作区 ${workspaceRoot}`);
  }
  return resolved;
}
```

## 真实攻击面

| 攻击向量 | 防御 |
|---------|------|
| **Prompt Injection**（工具结果含恶意指令） | 工具结果标记为 data，不作为 instruction |
| **持久化记忆注入** | 写入记忆前 LLM 校验内容合法性 |
| **Shell 内置命令绕过** | 不能只靠字符串匹配，需要 LLM 语义分类 |
| **审批疲劳** | 聚合相似操作，批量审批 |
| **路径穿越** | resolve() 后检查是否在 workspace 内 |

## MCP 安全硬伤

MCP 的主要安全风险：  
- **Tool Poisoning**：恶意 MCP Server 返回的工具描述含 Prompt Injection
- 防御：只连接可信 MCP Server，对工具描述做内容审查
- 不要把 MCP Server 当做沙箱——它们有完整的网络访问权限
