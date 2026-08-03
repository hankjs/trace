# 10 Agent 后端类型化重构（纯重构，零行为变更）

> 独立任务，与飞书交互五个场景（09A~09D）无关，可单独执行。
> **这是纯重构：不新增功能、不改变任何运行时行为。**
> 验收标准是「测试全绿 + 行为不变 + 加后端时漏一处就编译不过」。

## 背景与目标

Agent 后端（`codex` / `claude` / `grok` / `kimi`）目前**全部以字符串字面量流转**。
实测分布（`grep -c '"codex"\|"claude"\|"grok"\|"kimi"'`）：

| 文件 | 字面量次数 |
|------|-----------|
| `server/src/cli_agent.rs` | 59 |
| `server/src/feishu/router.rs` | 25 |
| `cli/src/agent.rs` | 14 |

另外 `server/src/remote_exec.rs`、`server/src/admin.rs`、`server/src/chat.rs`、
`server/src/feishu/card.rs`、`server/src/team_task/card.rs`、`crates/hank-db/src/lib.rs`
也各有若干处。

### 问题 1：同一份后端清单存在五个副本

```
cli/src/agent.rs:23          const SUPPORTED_BACKENDS: [&str; 4] = ["codex","claude","grok","kimi"];
server/src/remote_exec.rs:357    const ALLOWED: [&str; 4]      = ["codex","claude","grok","kimi"];
server/src/cli_agent.rs:2138 const PREFERRED_EXTERNAL_BACKEND_ORDER: [&str; 4] = [...同上...];
server/src/chat.rs:218       matches!(agent_backend, "codex" | "claude" | "grok" | "kimi")
server/src/feishu/router.rs:1535 matches!(backend, "codex" | "claude" | "grok" | "kimi")
```

五处各自硬编码 `[&str; 4]` 或 `matches!`。加第五个后端要同时改五处，
**编译器一处都不会提醒**。

### 问题 2：已有 enum 但困在单个文件里

`server/src/feishu/router.rs:243` 已经定义了 `AgentBackend`
（`Native` / `Codex` / `Claude` / `Grok` / `Kimi`），但 `grep -rln AgentBackend`
只命中 router.rs 一个文件——25 处引用全在文件内。它通过 `as_str()` 退化成字符串后
跨模块传递，类型安全在模块边界就丢了。

**抽象已经存在，只是没被贯穿。**

### 问题 3：未知后端的处理方式互相矛盾

```rust
// cli_agent.rs:1696 —— 静默丢弃
match backend {
    "codex" => handle_codex_event(...).await,
    "claude" | "grok" => handle_claude_event(...).await,
    "kimi" => handle_generic_event(...).await,
    _ => {}          // ← 拼错一个字母：事件全丢，无日志，任务看起来"跑着但没进度"
}

// cli_agent.rs:1469 —— 直接报错
_ => bail!("不支持的外部 Agent 后端: {backend}"),
```

同一个概念，一处静默、一处报错。这种不一致本身就是抽象缺失的症状。
`:1696` 那处最危险：新后端能启动、能跑完，但进度卡永远空白，**而且不报错**。

### 目标

把 `AgentBackend` 提升为跨模块的类型，让「后端有哪些、每个后端怎么做」
集中在一处，加后端时**漏一处就编译不过**。

**做完之后的可观察效果**：

- 运行时行为与重构前**完全一致**（所有现有测试不改期望值即通过）。
- 在 enum 上加一个变体后，`cargo build` 会在所有需要适配的地方报
  `non-exhaustive patterns` 错误，而不是静默走兜底分支。
- 后端清单只有一处定义。

## 关键约束：`cli/` 是独立项目

`Cargo.toml` 的 workspace `exclude = ["client/src-tauri", "cli"]`，且
`cli/Cargo.toml` 自己声明了 `[workspace]`（注释说明：部署到服务器构建时
父目录 workspace 没 exclude 它，所以要自足）。`cli/Cargo.toml` 的
`[dependencies]` **不依赖任何本地 crate**。

**所以不能让 `cli/` 引用 server 侧的 enum。** 本任务的处理方式：

- server 侧（workspace 内）统一用新 enum。
- `cli/src/agent.rs` 保持字符串 + `SUPPORTED_BACKENDS` 不动，但**补一处校验**
  （见步骤 6），让它至少在解析入口拒绝未知值。
- 两侧的"契约"是 wire format 上的字符串值，本任务不改这些字符串，
  所以跨进程兼容性不受影响。

**不要为了统一类型去给 `cli/` 加 workspace 依赖**——那会改变部署构建方式，
风险远大于收益。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `crates/hank-db/src/lib.rs` | 新增 `AgentBackend` enum（定义点） |
| `server/src/cli_agent.rs` | 9 处决策点改为 enum 方法或穷尽 match |
| `server/src/feishu/router.rs` | 删除本地 enum，改用 hank-db 的；`is_external_backend` 改为 enum 判定 |
| `server/src/chat.rs` | `matches!` 改为 enum 解析 |
| `server/src/remote_exec.rs` | `ALLOWED` 改用 enum 全集 |
| `server/src/admin.rs` | 3 处 `backend == "claude"` 改为 enum 比较 |
| `cli/src/agent.rs` | 只补解析入口校验，不引入 enum |

**不许碰**：`server/src/feishu/card.rs`、`server/src/team_task/card.rs`
（那些字面量都在测试 fixture 里，是测试数据不是逻辑）、`admin/`、`client/`、
`team/`、`quant/`、`server/src/weixin/`。

**不要改任何 wire format 字符串**：DB 里存的 `agent_backend` 值、
hank-cli 上报的 `agent_backends` 数组、路由 Agent 返回的 JSON 字段值，
全部保持 `"codex"` / `"claude"` / `"grok"` / `"kimi"` 原样。

保留工作区原有改动，不回退与本任务无关的内容。
**注意**：工作区可能有 `quant/app/factors/evaluation.py` 等未提交改动，一律不要动。

## 实现步骤

### 1. 定义 enum（放 `hank-db`）

**为什么放 `hank-db`**：`server` 依赖 `hank-db`，且 DB 里存着 `agent_backend` 字段，
类型定义和持久化放一起最自然。`code-agent` / `code-tools` 不需要它。

- [ ] 在 `crates/hank-db/src/lib.rs` 的类型定义区（`AgentInteraction` 附近）新增：

```rust
/// 外部 Agent CLI 后端。
///
/// 为什么要类型：后端清单曾在五处各自硬编码（cli/agent.rs、remote_exec.rs、
/// cli_agent.rs、chat.rs、feishu/router.rs），加一个后端要改五处且编译器不提醒。
/// 收敛到 enum 后，加变体会让所有穷尽 match 编译失败，漏一处就过不了。
///
/// wire format 是 `as_str()` 的返回值：DB 的 agent_backend 列、hank-cli 上报的
/// agent_backends、路由 Agent 返回的 JSON 都用这些字符串，**不可更改**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    Codex,
    Claude,
    Grok,
    Kimi,
}

impl AgentBackend {
    /// 全部变体。加变体时这里也要加——它是 `remote_exec` 白名单等处的唯一来源。
    pub const ALL: [AgentBackend; 4] = [Self::Codex, Self::Claude, Self::Grok, Self::Kimi];

    /// 默认优先级顺序（原 PREFERRED_EXTERNAL_BACKEND_ORDER）。
    pub const PREFERRED_ORDER: [AgentBackend; 4] =
        [Self::Codex, Self::Claude, Self::Grok, Self::Kimi];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Kimi => "kimi",
        }
    }

    /// 严格解析：未知值返回 None（不做任何猜测或回落）。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.as_str() == value)
    }
}

impl std::fmt::Display for AgentBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
```

- [ ] 注意 `ALL` 与 `PREFERRED_ORDER` 当前内容相同，但语义不同（一个是全集、
      一个是偏好次序），**保持两个常量**——将来偏好次序可能变，全集不会。
- [ ] 单测：`parse` 对四个合法值返回 `Some`、对 `"native"` / `"" ` / `"Codex"`
      （大写）返回 `None`；`as_str` 与 `parse` 往返一致；`ALL.len() == 4`。

### 2. `feishu/router.rs`：删除本地 enum

router.rs 现有的 `AgentBackend` **多一个 `Native` 变体**，这是它和新 enum 的
唯一实质差异。处理方式：

- [ ] 保留一个**本地**的路由决策类型承载 `Native`，因为路由 Agent 确实会返回
      `native`（表示不用外部 CLI）。改名避免与新 enum 混淆：

```rust
/// 路由 Agent 对新话题选择的执行后端。native 表示走 server 内建 Agent（无外部 CLI）。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RoutedBackend {
    Native,
    External(...)   // ← 见下
}
```

**但 serde 对这种形状不友好**。更简单的做法（**推荐**）：保留原 enum 名与形状不变，
只把它的 `as_str` / `preferred` 改为委托给 `hank_db::AgentBackend`：

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentBackend {
    Native,
    Codex,
    Claude,
    Grok,
    Kimi,
}

impl AgentBackend {
    /// 转成跨模块的外部后端类型；Native 无对应值（它不是外部 CLI）。
    fn external(self) -> Option<hank_db::AgentBackend> {
        match self {
            Self::Native => None,
            Self::Codex => Some(hank_db::AgentBackend::Codex),
            Self::Claude => Some(hank_db::AgentBackend::Claude),
            Self::Grok => Some(hank_db::AgentBackend::Grok),
            Self::Kimi => Some(hank_db::AgentBackend::Kimi),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            other => other.external().expect("非 Native 必有外部值").as_str(),
        }
    }
}
```

这样 `Native` 的特殊性显式表达为 `Option`，且字符串只有一处定义。
**注意** `as_str` 里 `other => other.external()...` 这种写法需要 `other` 能匹配剩余变体，
Rust 允许；但若 clippy 不满意，改成完整列举也可以——以编译和 clippy 通过为准。

- [ ] `router.rs:1535` 的 `is_external_backend`：

```rust
fn is_external_backend(backend: &str) -> bool {
    hank_db::AgentBackend::parse(backend).is_some()
}
```

- [ ] 现有测试 `parse_new_topic_decision` 等**不改期望值**，它们断言的是解析结果，
      与内部表示无关。

### 3. `cli_agent.rs`：9 处决策点

这是本任务主体。逐处改造，**每处都要保持行为完全一致**：

| 行号 | 所在函数 | 现状 | 改法 |
|------|---------|------|------|
| 232 | `backend_env_whitelist` | `"claude"` / `"codex"` → 返回 `Some(..)`，其余 None | 改为 `match AgentBackend`，`Grok`/`Kimi` 显式返回 `None` |
| 294 | `auth_from_db_config` | `"codex"` → 设 base_url | 穷尽 match，其余变体显式 no-op |
| 302 | `auth_from_db_config` | `if backend == "claude"` | `if backend == AgentBackend::Claude` |
| 1467 | `build_command` | codex/claude 给路径，其余 `bail!` | 穷尽 match，`Grok`/`Kimi` 显式 `bail!`（**保留原错误文案**） |
| 1578 | `build_command` | codex/claude 参数构造 | 同上，穷尽 match |
| 1696 | `handle_json_line` | `_ => {}` 静默 | **穷尽 match，去掉 `_`**（见下方说明） |
| 2053 | `resolve_cli_auth` | env 白名单 | 穷尽 match |
| 2085 | `resolve_cli_auth` | 凭据完整性判定 | 穷尽 match |
| 2102 | `resolve_cli_auth` | provider 兼容性 | 穷尽 match |
| 2118 | `resolve_cli_auth` | 错误文案 | 穷尽 match |
| 2138 | 常量 | `PREFERRED_EXTERNAL_BACKEND_ORDER` | 删除，用 `AgentBackend::PREFERRED_ORDER` |
| 2194 | `effective_auth_source` | provider 匹配 | 穷尽 match |
| 2218 | `validate_runtime` | `if backend == "claude" {"claude"} else {"codex"}` | 穷尽 match（**注意这里原逻辑是 grok/kimi 也落到 "codex" 目录**，重构必须保持！） |

- [ ] **`:2218` 是最容易改错的一处**。原代码：

```rust
.join(if backend == "claude" { "claude" } else { "codex" })
```

grok 和 kimi 走的是 `"codex"` 目录。这可能是历史遗留（甚至可能是 bug），
但本任务是**纯重构**，必须原样保持：

```rust
// 注意：grok / kimi 历史上共用 codex 的状态目录。这里保持原行为，
// 是否该各自独立目录属于行为变更，不在本次重构范围。
.join(match backend {
    AgentBackend::Claude => "claude",
    AgentBackend::Codex | AgentBackend::Grok | AgentBackend::Kimi => "codex",
})
```

- [ ] **`:1696` 的 `_ => {}` 必须去掉**，这是本次重构最重要的收益：

```rust
match backend {
    AgentBackend::Codex => handle_codex_event(state, session_id, auth, &value, run).await,
    AgentBackend::Claude | AgentBackend::Grok => {
        handle_claude_event(state, session_id, auth, &value, run).await
    }
    AgentBackend::Kimi => handle_generic_event(state, session_id, auth, &value, run).await,
}
```

去掉 `_` 后，加新后端时这里会编译失败——这正是我们要的。

- [ ] **函数签名改造**：上述函数的 `backend: &str` 参数改为
      `backend: AgentBackend`。调用方需要解析字符串的地方，在**边界处**解析一次：
      从 DB 读出的 `agent_backend`、从 session metadata 读出的值，
      用 `AgentBackend::parse(..)` 解析，`None` 时返回明确错误
      （**不要**默认回落到 Codex——那会掩盖数据问题）。

- [ ] `preferred_backend_from_online_capabilities` 与
      `preferred_external_backend`（`:2158`~`:2172`）的签名也改为返回
      `Option<AgentBackend>` / `AgentBackend`。`:2171` 的
      `.unwrap_or("codex")` 改为 `.unwrap_or(AgentBackend::Codex)`，
      保留原注释（说明这是"确定失败路径"）。
      **现有测试 `:3347`~`:3364` 的期望值要跟着改类型**，
      但断言的语义必须一致（如 `Some("claude")` → `Some(AgentBackend::Claude)`）。

### 4. `chat.rs` 与 `remote_exec.rs`

- [ ] `server/src/chat.rs:218`：

```rust
// 原: if matches!(agent_backend, "codex" | "claude" | "grok" | "kimi") {
if hank_db::AgentBackend::parse(agent_backend).is_some() {
```

- [ ] `server/src/remote_exec.rs:357` 的 `sanitize_agent_backends`：
      把 `const ALLOWED: [&str; 4]` 换成用 `AgentBackend::ALL`：

```rust
// 白名单唯一来源是 AgentBackend::ALL，不再各自维护一份清单。
let allowed: Vec<&str> = hank_db::AgentBackend::ALL.iter().map(|b| b.as_str()).collect();
```

或更直接：`AgentBackend::parse(v).is_some()` 做过滤。
现有测试（`:572`~`:584`）**不改期望值**，它们断言的是过滤结果。

### 5. `admin.rs` 的三处

- [ ] `:638` `for backend in ["claude", "codex"]`：这是遍历要检查的两个后端，
      **保持只有这两个**（grok/kimi 没有 admin 侧凭据校验）。
      改为 `for backend in [AgentBackend::Claude, AgentBackend::Codex]`。
- [ ] `:928` / `:935` / `:990` 的 `backend == "claude"` / `is_claude`：
      改为与 `AgentBackend::Claude` 比较。注意 `:990` 是
      `config.backend == "claude"`，`config.backend` 来自 DB 是 `String`，
      在这里 `AgentBackend::parse(&config.backend) == Some(AgentBackend::Claude)`。

### 6. `cli/src/agent.rs`：只补校验，不引入 enum

`cli/` 是独立项目，不依赖 `hank-db`（见上文「关键约束」）。

- [ ] `SUPPORTED_BACKENDS`、`build_command` 的字符串 match **全部保持不动**。
- [ ] 只在 `:456` 起的 `build_command` 的 match 末尾，把兜底分支的错误文案
      补明确（如果当前是静默或含糊的话）。先读代码确认现状：若已有明确
      `Err(...)` 兜底，本步骤**什么都不用做**，在文档里注明"已满足"即可。
- [ ] 在 `SUPPORTED_BACKENDS` 上方补注释，指明它与 server 侧
      `hank_db::AgentBackend::ALL` 必须保持一致：

```rust
// 必须与 server 侧 hank_db::AgentBackend::ALL 保持一致（wire format 契约）。
// cli 是独立 Cargo 项目、不依赖 workspace crate，所以无法共享类型，
// 加后端时两处都要改。
const SUPPORTED_BACKENDS: [&str; 4] = ["codex", "claude", "grok", "kimi"];
```

### 7. 验证「漏一处就编译不过」

- [ ] 临时在 enum 上加一个变体 `Dummy`（**只用于验证，最后要删掉**），
      跑 `cargo build --workspace`，确认编译器报出所有穷尽 match 缺口。
      记录下报错的位置数量。
- [ ] 删除 `Dummy`，确认重新编译通过。
- [ ] 在 commit message 或 PR 描述里写明：加变体时会有 N 处编译错误
      提示需要适配。**不要**把 `Dummy` 留在代码里。

## 明确边界

- **零行为变更**。任何"顺手修的 bug"都不属于本任务。特别是：
  - `:2218` 的 grok/kimi 共用 codex 状态目录——**保持**，即使看起来像 bug。
  - `:1696` 的 `claude | grok` 共用事件解析——**保持**。
  - `:638` 只检查 claude/codex 两个后端——**保持**。
  - 如果你发现了疑似 bug，**写进 commit message 的备注里**，不要动手改。
- 不改任何 wire format 字符串（DB 值、上报值、JSON 字段值）。
- 不给 `cli/` 加 workspace 依赖，不改 `cli/Cargo.toml`。
- 不改测试的**断言语义**（类型可以跟着变，期望的行为不能变）。
- 不动 `feishu/card.rs`、`team_task/card.rs` 里的测试 fixture 字面量。
- 不新增第三方依赖。
- 保留工作区原有改动（如 `quant/` 下的未提交改动），不回退。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo test -p hank-db
cargo test -p code-agent
cargo test -p code-tools
cd cli && cargo build && cargo test
```

期望：全部通过。当前基线：

| crate | 测试数 |
|-------|-------|
| hank-server | 243 passed |
| hank-db | 4 passed |
| code-agent | 34 + 16 + 1 passed |
| code-tools | 32 passed |

**测试数量可以增加**（enum 的新单测），但**不能减少**，且现有测试的断言语义
必须不变。

特别确认：

- `grep -c '"codex"' server/src/cli_agent.rs` 应显著下降（重构前 59 处
  四个后端字面量合计，重构后应只剩测试 fixture 与 `as_str()` 定义处）。
- `grep -rn 'matches!(.*"codex"' server/src/` 无输出（五份副本清单已收敛）。
- `grep -n '_ => {}' server/src/cli_agent.rs` 在 `handle_json_line` 的 backend
  match 处无输出。
- `cargo fmt --all -- --check` 干净（`c68b887` 已清掉既有欠账，不要再引入新的）。
- `cargo clippy --workspace --all-targets -- -D warnings`：
  **改动前 `crates/code-tools/` 与 `hank-db` 有既有 clippy 欠账**，
  本任务不要求清理它们，但**不得新增**。可用
  `cargo clippy -p hank-server --all-targets 2>&1 | grep -c warning` 对比前后。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；注释写"为什么"而非"是什么"。
enum 的文档注释要讲清 wire format 契约（为什么字符串值不能改）。

commit message 建议：

```
refactor(agent): 后端字符串收敛为 AgentBackend 枚举

五处各自维护的后端清单合为一处；cli_agent 的 9 个决策点改为穷尽 match，
handle_json_line 去掉静默兜底分支——加后端时漏一处即编译失败。
零行为变更：grok/kimi 共用 codex 状态目录、claude|grok 共用事件解析等
既有行为原样保留。
```
