---
name: agent-practice
description: "Agent 工程实践知识库。覆盖 Agent 六大支柱：Loop 设计、Tool System、Context Engineering、Memory、权限安全、Multi-Agent。用于回答 Agent 开发问题、代码生成、架构设计、调试排错。"
when_to_use: "用户询问 Agent 开发、Agent 架构、工具系统、上下文管理、记忆系统、多 Agent 协作、权限控制、流式输出、MCP 集成等话题时使用。"
user-invocable: true
---

# Agent 工程实践

基于 Hank Agent 系列课程（agent-fundamentals + super-agent）的生产级 Agent 工程知识体系。

## 六大支柱速查

| 支柱 | 核心问题 | Reference |
|------|---------|-----------|
| Agent Loop | 循环结构、终止条件、保险丝 | [loop](references/loop.md) |
| Tool System | 工具注册、执行管线、截断、并发、动态加载 | [tools](references/tools.md) |
| Context Engineering | 压缩、JIT、Cache、Prompt Pipe | [context](references/context.md) |
| Memory | 文件派 vs 数据库派、失效模式 | [memory](references/memory.md) |
| Permission & Safety | 四层防线、沙箱、Prompt Injection | [permission](references/permission.md) |
| Multi-Agent | 父子模式、Swarm、上下文隔离 | [multi-agent](references/multi-agent.md) |

---

## Agent 本质

Agent = **while 循环** + **工具调用**

```
while true:
  response = model(context)
  if response.stop_reason == "end_turn": break
  execute(response.tool_calls)
  append results to context
```

与 ChatBot 的核心区别：**AI 主导控制流**，而非用户每次驱动。

---

## 三个保险丝（必须实现）

1. **死循环检测** — SHA256 指纹，连续相同调用升级响应（警告→强制换路→熔断）
2. **Token 预算** — 90% 时注入 nudge，100% 时停止
3. **输出截断恢复** — 提高上限 → 注入恢复消息 → 认栽三步走

---

## 工具系统核心原则

- `maxOutputLength` 必设，防止单次工具调用吃掉大量 token（head 60% + tail 40%）
- 只读工具可并发，写工具串行
- 工具数量 >15 考虑 Deferred Loading
- 错误信息面向模型（含纠错上下文），不是面向开发者（只有错误码）
- Mask Don't Remove：禁用工具用掩码，保护 KV Cache

---

## Context Engineering 五维 ORRIC

- **Offload** — 上下文写出去（文件/DB），按需取回
- **Reduce** — 压缩（Microcompact → Snip → LLM 摘要）
- **Retrieve** — JIT 按需加载（Agentic Search / RAG / Offloading）
- **Isolate** — 子 Agent 隔离上下文
- **Cache** — 静态前缀稳定，动态内容追加末尾

---

## Memory 系统选择

- **文件派**（简单场景）：MEMORY.md 索引 + 独立记忆文件，200 行上限
- **数据库派**（需跨会话语义检索）：SQLite + 向量，混合检索 + 时间衰减

---

## Skills vs MCP vs Tools

| 概念 | 本质 | 适用场景 |
|------|------|---------|
| Tool | 可执行函数，模型调用 | 操作文件、调 API、执行命令 |
| Skill | Markdown 行为指导，注入 system prompt | 领域 SOP、角色定义、约束规范 |
| MCP | 远程服务连接协议（JSON-RPC） | 连接外部有状态服务（GitHub、数据库） |
