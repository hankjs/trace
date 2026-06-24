# Multi-Agent 详解

## 拆 Agent 的真实动机

**不是为了"角色分工"，是为了分上下文。**

- 单 Agent 处理大任务时，上下文窗口会被中间步骤的工具结果塞满
- 子 Agent 有独立上下文，父 Agent 只看到子任务的最终结果
- 并发子 Agent = 并行压缩上下文，是处理大规模任务的核心手段

## 父子模式（最基础架构）

```typescript
// 父 Agent
async function orchestratorAgent(task: string) {
  const subtasks = await planTasks(task);           // 分解任务
  const results = await Promise.all(
    subtasks.map(t => spawnWorkerAgent(t))           // 并发子 Agent
  );
  return synthesize(results);                        // 汇总
}

// 子 Agent（独立上下文，只返回结果）
async function workerAgent(subtask: string): Promise<string> {
  const messages: Message[] = [];
  return agentLoop({ system: WORKER_PROMPT, messages, tools: WORKER_TOOLS });
}
```

## 并发安全常见坑

| 问题 | 场景 | 解法 |
|------|------|------|
| 文件竞争 | 多个 Worker 同时写同一文件 | 按文件路径分配 Worker，避免重叠 |
| 任务重复 | 多个 Worker 领到同一任务 | 共享任务列表 + 原子 claim（SQLite EXCLUSIVE） |
| 结果顺序 | Promise.all 结果乱序 | 用 index 对齐原始任务顺序 |
| 上下文泄漏 | 父 Agent 塞了太多子任务细节 | 子 Agent 结果只返回摘要，不返回完整工具历史 |

## 受影响路径冲突检测

```typescript
function hasPathConflict(tasks: Task[]): boolean {
  const paths = tasks.flatMap(t => t.affectedPaths ?? []);
  return new Set(paths).size < paths.length; // 有重复路径
}

// 有路径重叠或写操作 → 串行；无冲突只读 → 并行
const runConcurrently = !hasPathConflict(tasks) && tasks.every(t => t.isReadOnly);
```

## Agent Swarm（松耦合多 Agent）

基于文件的 Mailbox 消息系统：
- 每个 Agent 监听自己的消息目录
- 支持单播（agent-id）/ 广播（all）/ 组播（group-name）
- 共享任务列表防止"抢活"

```
.swarm/
  mailbox/
    agent-a/  # Agent A 的收件箱
    agent-b/  # Agent B 的收件箱
  tasks/
    pending/  # 未认领任务
    active/   # 处理中任务（原子 rename 认领）
    done/     # 已完成任务
```

## Harness 模式

Harness = 包裹模型的工程层

```
Input → [System Prompt] → Model → [Tool Dispatch] → Output
                ↑                         ↑
          Context Manager            Permission Guard
          (压缩/JIT/Cache)           (四层防线)
                              ↑
                         Hook System
                     (观测/插件/限流)
```

**Generator/Evaluator** 是最经典的 Harness 模式：
- Generator：生成候选方案
- Evaluator：评估结果，决定是否接受或要求重试
- 过度工程化陷阱：不是所有 Agent 都需要 Evaluator，简单任务一个 Generator 够用

## Hook 系统（不改源码定制行为）

```typescript
hooks.on('tool:before', async ({ tool, args }) => {
  logger.info(`[Tool] ${tool}`, args);  // 可观测性
});
hooks.on('tool:after', async ({ tool, result, duration }) => {
  metrics.record('tool.duration', duration, { tool });
});
hooks.on('message:before', async ({ messages }) => {
  // 动态注入 context 而不修改 Agent 核心逻辑
});
```

Hook 四组事件：生命周期（session start/end）、工具（before/after/error）、消息（before send）、错误。
