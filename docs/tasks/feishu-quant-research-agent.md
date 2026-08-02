# 飞书接通 quant 研究能力（quant_research 话题类型）

## 一、背景与目标

### 现状：飞书拿不到 quant 工具

`server/src/chat.rs` 里注册 19 个 `quant_*` A2A 工具的代码在 `chat.rs:421`，
它位于 `let tools = if conversation_agent { Vec::new() } else { ...这里... }`
（`chat.rs:313`）的 **else 分支**里。

而飞书话题经 `create_feishu_session`（`server/src/feishu/router.rs:1711`）
和 `NewTopicDecision::normalized`（`router.rs:292`）后，**只可能是两种形态**：

| 路由结果 | backend | 结果 |
|---|---|---|
| `conversation` | 强制 `native` | `conversation_agent = true` → `tools = Vec::new()`，**没有 quant 工具** |
| `trace_code` / `quant_code` / `general_task` | 强制外部 CLI（codex/claude/grok/kimi） | `chat.rs:212` 提前 return 进 `cli_agent::run_cli_turn`，**根本走不到工具注册** |

所以**飞书渠道当前完全无法调用 quant 的研究能力**。作为对比，微信建会话时
metadata 只写 `{"source":"weixin"}`（`server/src/weixin/router.rs:960`），
没有 `agent_kind`，于是 `conversation_agent = false`，微信**能**拿到 quant 工具。
这个不对称是实现遗漏，不是设计意图。

连带后果：已经写好的飞书确认卡片闸门（`feishu/pusher.rs:301` 的
`quant_confirm:` 分支、`feishu/callback.rs`、`chat.rs` 的
`handle_quant_confirmation`）在飞书侧永远不会触发——只有微信和 Trace 网页
聊天能触发。

### 目标

新增第五种话题类型 `quant_research`：飞书里 @机器人 问「今天有什么信号」、
「帮我回测策略 42」、「校验这个因子表达式」时，路由到一个 **server 侧 native
会话**，挂载 `quant_*` 工具与 `quant-research` skill，**不挂** shell/文件/Git
工具（它没有工作目录）。高成本操作复用现有确认卡片闸门。

### 做完之后的可观察效果

1. 飞书新话题问「今天 quant 有什么买入信号」→ 机器人调用 `quant_screen` /
   `quant_catalog` 等只读工具，直接给出中文结论，任务卡片正常刷新到绿色。
2. 飞书里说「回测一下策略 42」→ 弹出「高成本操作确认」卡片，点「确认」后
   回测真正执行；点「否」则停止。
3. 回复文字「确认5次」可批量授权（飞书**允许**批量，与微信不同）。
4. `/status` 能看出当前话题是 `quant_research` 类型。
5. `quant_a2a.enabled = false` 时，路由**不会**产出 `quant_research`，
   行为与现在完全一致（无回归）。

## 二、涉及文件清单

| 文件 | 改什么 |
|---|---|
| `server/src/feishu/router.rs` | 新增 `AgentKind::QuantResearch`；路由 prompt 增加该类型说明；`normalized` 强制 native；`should_bind_remote_exec_client` 返回 false；`create_feishu_session` 走「无工作区 native」分支；`/help` 与 `/status` 文案 |
| `server/src/chat.rs` | 工具注册从「二分」改为「三分」：conversation（无工具）/ research（仅 quant + ask_user + web_fetch）/ 其余（全量）；research 会话注入研究专用 project segment |
| `docs/feishu.md` | 三、用法表补 quant 研究话题；架构段落说明第五种话题类型 |
| `config.example.toml` | `[quant_a2a]` 注释补一句：开启后飞书才会路由 quant_research |

**不许碰**：

- `crates/code-tools/`（`quant_tools.rs`、`quant_grant.rs` 保持原样，闸门已可用）
- `crates/code-agent/src/session.rs`（`quant_confirm_prompt` 与闸门无需改动）
- `server/src/feishu/pusher.rs`、`callback.rs`（已按 `quant_confirm:` 前缀通配，零改动）
- `server/src/weixin/`（微信链路不动）
- `server/src/deployment.rs`（部署链路是另一份任务）
- `quant/`（Python 侧与看板不动，本任务纯 Rust server 侧）
- `client/`

**保留工作区原有改动**：`docs/tasks/cargo-fmt-whole-project.md` 是未跟踪文件，
不要删除或提交它；不要回退任何与本任务无关的内容。

## 三、实现步骤

### 1. router.rs：新增 AgentKind 变体

在 `enum AgentKind`（约 `router.rs:208`）加一个变体，并同步三个 match：

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentKind {
    Conversation,
    QuantResearch,   // 新增
    TraceCode,
    QuantCode,
    GeneralTask,
}
```

- `agent_kind()` → `"quant_research"`
- `workspace_kind()` → `WorkspaceKind::None`（与 Conversation 一致，不建目录）

### 2. router.rs：normalized 强制 native

`NewTopicDecision::normalized`（约 `router.rs:292`）当前把 Conversation 的
backend 强制成 Native。`QuantResearch` 也必须强制 Native——它要走 server 侧
native 会话才能挂 quant 工具，落到外部 CLI 就会被 `chat.rs:212` 提前 return：

```rust
fn normalized(mut self, default_backend: AgentBackend) -> Self {
    match self.agent_kind {
        AgentKind::Conversation | AgentKind::QuantResearch => {
            self.agent_backend = AgentBackend::Native
        }
        _ if self.agent_backend == AgentBackend::Native => {
            self.agent_backend = default_backend
        }
        _ => {}
    }
    self
}
```

### 3. router.rs：不绑远程执行节点

`should_bind_remote_exec_client`（约 `router.rs:1846`）目前是
`agent_kind != "conversation"`。`quant_research` 同样没有工作目录，绑了
`exec_client_id` 会让 `chat.rs` 注入「工作目录在你本地桌面」的说明，与
「只有 quant 工具、没有文件工具」自相矛盾（这个坑注释里已经记过一次）：

```rust
/// conversation 与 quant_research 都无工作目录，不得绑定远程节点。
fn should_bind_remote_exec_client(agent_kind: &str) -> bool {
    !matches!(agent_kind, "conversation" | "quant_research")
}
```

### 4. router.rs：路由 prompt 增加类型，且受 quant_a2a 开关约束

`try_decide_new_topic`（约 `router.rs:1019`）需要多一个入参或从 `state` 读
`state.config.quant_a2a`，**只在 `enabled = true` 时**把 `quant_research`
写进可选值。关闭时 prompt 与现在逐字一致，保证无回归。

建议在 `decide_new_topic` 里算好一个 `quant_enabled: bool` 传进去：

```rust
let quant_enabled = state
    .config
    .quant_a2a
    .as_ref()
    .is_some_and(|c| c.enabled);
```

prompt 里 `agent_kind 可选值` 列表在 `quant_enabled` 时插入一条（放在
`conversation` 之前）：

```text
- quant_research：用户在问 A 股行情、信号、选股、策略、因子、回测、持仓记账，
  或要求验证/研究某个量化想法。这类任务由 quant 研究工具直接回答，不需要
  读写代码文件。注意：修改 quant 项目代码本身属于 quant_code，不是 quant_research。
```

并在示例后补一句判定优先级，避免模型把「回测策略 42」误判成 `quant_code`：

```text
只要用户是在用 quant 的数据和能力做研究（查信号、跑回测、评估因子），
一律选 quant_research；只有当用户要改 quant 的源码、看板或文档时才选 quant_code。
```

`enabled = false` 时额外声明一句「本环境没有 quant 研究工具，不得输出
quant_research」更稳妥（防止模型凭常识乱猜）。

### 5. router.rs：create_feishu_session 走无工作区 native

`create_feishu_session`（`router.rs:1711`）当前第二个分支是
`if state.config.server_agent.enabled && agent_backend == "native"`，
它会调 `ensure_server_agent_admin`（要求 `can_login_admin`）。

**quant_research 不应要求 admin**——它没有 server 工作区，只调 quant REST，
权限由透传 JWT 在 quant 侧收敛（策略可见性过滤天然生效）。这与
`docs/feishu.md`「管理员边界」一节一致：admin 只在创建 server 侧
native/worktree **工作区**时校验。

所以在该分支**之前**插入 quant_research 的专属分支，写明 metadata：

```rust
// quant 研究话题：server 侧 native 会话，只挂 quant_* 工具，无工作区、
// 不绑执行节点，因此不要求 can_login_admin。
if agent_kind == "quant_research" {
    let metadata = serde_json::json!({
        "source": "feishu",
        "agent_backend": "native",
        "agent_kind": agent_kind,
        "workspace_kind": "none",
    })
    .to_string();
    let session = state
        .db
        .create_session(
            "", "", None, Some(user_id),
            Some("remote"), Some("chat"), Some(&metadata),
        )
        .await
        .map_err(|e| anyhow!("create quant research session: {e:#}"))?;
    return Ok(session);
}
```

要点：

- **必须写 `"source": "feishu"`**。`chat.rs:425` 从 metadata 读 `source`
  传给 `quant_tools()`，它最终决定确认话术与是否允许批量授权
  （`parse_quant_confirmation` 里 `source != "weixin"` 才允许「确认N次"）。
  漏写会 fallback 成 `trace_chat`，虽然行为相同但日志和话术会误导。
- `work_dir` 传 `None`，**不要**调 `set_session_exec_client`。
- **不要**写 `"server_agent": true`。写了会让
  `is_repository_workspace_metadata`（`deployment.rs:1077`）在
  `workspace_kind` 缺失时回退判 true；这里显式写 `"none"` 已经安全，但也
  不要多写这个字段，避免 `chat.rs` 的 `server_agent_session` 分支注入
  「你正在 wananyun 工作区」的错误说明。

### 6. chat.rs：工具注册从二分改三分

`chat.rs:289` 附近已有 `conversation_agent` / `quant_code_agent` 的解析。
同样从 metadata 解析出 research 标记：

```rust
let research_agent = routed_agent_kind
    .map(|kind| kind == "quant_research")
    .unwrap_or(false);
```

然后把 `chat.rs:313` 的 `let tools = if conversation_agent { ... } else { ... }`
改为三分支。**新分支只挂研究需要的工具**，不挂 shell/fs/git/test_runner
（会话没有 `work_dir`，挂了必然报错或误导模型）：

```rust
let tools: Vec<Arc<dyn Tool>> = if conversation_agent {
    Vec::new()
} else if research_agent {
    // quant 研究话题：只挂 quant_* 工具与 ask_user；没有工作目录，
    // 不注册 shell/文件/Git/测试工具。
    let mut t: Vec<Arc<dyn Tool>> = vec![Arc::new(AskUserTool::new())];
    t.push(Arc::new(WebFetchTool::new()));
    if let Some(ref quant_cfg) = state.config.quant_a2a {
        if quant_cfg.enabled && !token.is_empty() {
            let mut quant = quant_tools(
                quant_cfg.base_url.clone(),
                token.clone(),
                session_id.clone(),
                metadata_source,   // 见下
                state.quant_grant_store.clone(),
            );
            t.append(&mut quant);
            quant_tools_added = true;
        }
    }
    t
} else {
    // ...现有全量分支，逐字保留...
};
```

注意：

- `token` 与 `metadata_source` 的取值逻辑现在写在 else 分支内部
  （`chat.rs:422`~`429`）。请把 `source` 的解析**提取成一个变量**放到 if 之前
  复用，不要复制两份解析代码。
- `AskUserTool` 是确认闸门返回用户答案的载体，**必须注册**。
- `quant_tools_added` 变量已存在（`chat.rs:311`），复用它即可让
  `quant_research_prompt_inputs` 自动注入 `quant-research` skill 全文。
- 若 `quant_a2a` 未开启却进了 research 分支（理论上路由已挡住，属防御），
  工具集为空，模型会退化成纯对话——加一条 `tracing::warn!` 记录该异常。

### 7. chat.rs：注入研究专用 project segment

在 `project_segments` 组装处（`chat.rs:798` 附近，`conversation_agent`
分支旁边）加一个 `else if research_agent` 分支：

```rust
} else if research_agent {
    project_segments.push(code_agent::PromptSegment::Dynamic(
        "路由 Agent 已将当前话题标记为 quant 研究话题。你没有工作目录，也没有\
         shell、文件或 Git 工具，只能通过 quant_* 工具读取数据与执行研究操作。\
         回答用中文，说明结论用了哪一天的数据、命中了什么条件。\
         quant 只提供研究信息与模拟结果：不要输出买卖指令，也不要暗示自动交易。\
         若用户想修改 quant 源码或看板，提示用 /new 开启新话题重新路由。"
            .to_string(),
    ));
}
```

`repository_workspace` / `server_agent_session` 两个分支保持原样。产品口径见
`quant/PRODUCT.md` 第 13 行与 `quant/README.md` 第 6 行——**不得**把研究结论
写成交易建议，`chat.rs` 现有测试
`test_quant_research_skill_forbids_trading_wording` 就是守这条线的。

### 8. router.rs：/help 与 /status 文案

- `/help`（约 `router.rs:637`）不需要加新命令，但可在开头补一句：
  「直接问行情、信号、回测即可（quant 研究话题）」。
- `/status` 已经会展示会话 ID 与状态；确认它展示的 `agent_kind` 能显示
  `quant_research`（若当前实现没展示 agent_kind，则不要为此改造，跳过）。

### 9. 单元测试

在 `router.rs` 的 `mod tests` 里补（对齐 `router.rs:2035` 附近既有风格）：

```rust
#[test]
fn quant_research_decision_parsing_and_normalization() {
    assert_eq!(
        parse_new_topic_decision(
            r#"{"agent_kind":"quant_research","agent_backend":"native"}"#
        ).unwrap(),
        NewTopicDecision {
            agent_kind: AgentKind::QuantResearch,
            agent_backend: AgentBackend::Native,
        }
    );
    // 模型误填外部 backend 时必须被拉回 native，否则会落到 cli_agent
    assert_eq!(
        NewTopicDecision {
            agent_kind: AgentKind::QuantResearch,
            agent_backend: AgentBackend::Codex,
        }
        .normalized(AgentBackend::Claude)
        .agent_backend,
        AgentBackend::Native
    );
    assert_eq!(AgentKind::QuantResearch.workspace_kind(), WorkspaceKind::None);
    assert!(!should_bind_remote_exec_client("quant_research"));
}
```

在 `chat.rs` 的 `mod tests` 里补一条：research 会话的 project segment
含「不要输出买卖指令」且不含「工作目录」类措辞（参考既有
`test_quant_research_skill_forbids_trading_wording` 的写法）。若该 segment
的组装逻辑难以在单测中隔离，则把文案提取成一个 `const` 或纯函数再断言。

## 四、验收标准

```bash
# 1. 全 workspace 编译（必须零错误零新增 warning）
cargo build --workspace

# 2. 格式（本仓库已按 cargo fmt 统一，见 commit 671b0ae）
cargo fmt --all --check

# 3. Clippy
cargo clippy --workspace --all-targets

# 4. 测试
cargo test -p server
cargo test -p code-tools
cargo test -p code-agent
```

期望结果：

- 四条命令全部通过；`cargo test -p server` 中新增的
  `quant_research_decision_parsing_and_normalization` 通过。
- 既有测试**一条都不能改动或删除**。特别是
  `should_bind_remote_exec_client` 的既有断言
  （`router.rs:2027`~`2030`）必须继续通过：`conversation` 为 false，
  `general_task` / `trace_code` / `quant_code` 为 true。
- `test_quant_research_prompt_injected_when_quant_enabled` 与
  `test_quant_research_prompt_not_injected_when_quant_disabled` 保持通过。

**人工验收**（我来跑，不用 Grok 做）：本地起 server + quant，飞书新话题
发「今天有什么买入信号」，确认路由到 `quant_research`（看 server 日志的
`feishu: new topic workspace decision`），且能拿到工具结果；再发
「回测策略 X」确认弹出确认卡片。

## 五、约定

- 遵循 `CLAUDE.md`：中文注释、中文 commit message、后端错误用 `anyhow`。
- 注释写**为什么**，不写做了什么。本仓库注释风格是记录踩过的坑
  （参考 `router.rs:1806` 的「conversation 不得绑 exec_client，否则 prompt
  自相矛盾」那段），请保持同一密度与语气。
- commit message 建议：`feat(feishu): 新增 quant 研究话题类型`
- 不要新增依赖，不要改 `Cargo.toml`。
- 不要修改 `config.toml`（含真实凭据，且不入库）；只改
  `config.example.toml` 的注释。

## 六、遗留（不在本任务范围）

飞书的 `/diff` `/test` `/deploy` `/rollback` 目前同样是死链路：
`deployment::prepare_repository_workspace`（`deployment.rs:82`）**零调用方**，
而每个飞书会话都是 client-only 或 conversation，两者都被这四个命令显式拒绝。
这是下一份任务文档的内容，本次不要动 `deployment.rs`。
