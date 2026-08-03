# 任务 05a：团队任务开关移到数据库（后端 + admin REST）

> 这是对第 5 步的**架构调整**，插在原计划的第 5 步与第 6 步之间。
> 拆成两份：**05a（本份）做后端与 REST**，05b 做 admin 前端页面。
> 先只执行本份。

## 背景与目标

### 背景

第 5 步把两个开关放在了 `config.toml`：

```toml
[server_agent]
task_gate_enabled = true

[team_task]
enabled = false
```

问题：改这两个开关要编辑服务器上的文件并重启 `hank-server`。而它们是**运行时策略开关**
（要不要弹闸门、要不要走多角色流水线），不是部署基础设施 —— 应该能在 admin 页面上点一下就生效。

对比现有做法，项目里已经有清晰的先例：

| 类型 | 存哪 | 例子 |
|------|------|------|
| 部署基础设施 | `config.toml` | 数据库地址、worktree 根目录、sandbox 路径、执行用户 |
| 运行时策略 / 凭据 | 数据库 + admin REST | `job_states`（定时任务启停）、`agent_cli_profiles`（CLI 凭据）、`providers`、`feishu_accounts` |

这两个开关属于第二类，放错了地方。

### 本任务目标

1. 两个开关移到 `settings` 表，admin REST 可读写，**改完即时生效、无需重启**。
2. `config.toml` 里的值降级为**首次启动的默认值**（迁移用），DB 有值时以 DB 为准。
3. 校验从「启动时 `bail`」改为「**写入时拒绝**」——这对用户更友好：
   点保存立刻看到错误，而不是重启后服务起不来。

### 做完之后的可观察效果

1. `GET /api/admin/team-task/config` 返回当前生效配置。
2. `PATCH /api/admin/team-task/config` 改开关，**下一个飞书任务立刻按新配置走**。
3. 非法配置（如 `enabled=true` 但 `task_gate_enabled=false`）被 REST 拒绝并返回清晰原因。
4. `config.toml` 删掉 `[team_task]` 段也能正常启动（用默认值）。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `crates/hank-db/src/lib.rs` | 新增 `TeamTaskSettings` 结构 + `get_team_task_settings` / `save_team_task_settings` |
| `server/src/team_task/settings.rs` | **新建**。运行时配置解析（DB 优先、config.toml 兜底）+ 校验纯函数 + 单测 |
| `server/src/team_task/mod.rs` | 加 `pub mod settings;` |
| `server/src/team_task/routes.rs` | **新建**。admin REST：GET / PATCH |
| `server/src/main.rs` | 挂两条路由到 `admin_api` |
| `server/src/cli_agent.rs` | 3 处 `state.config.*` 改成读运行时配置 |
| `server/src/interaction_flow.rs` | 1 处同上 |
| `server/src/team_task/orchestrator.rs` | 2 处同上 |
| `server/src/config.rs` | `TeamTaskConfig::validate` 的启动校验**去掉**（移到写入时）；结构本身保留作默认值来源 |

## 实现步骤

### 步骤 1：DB 层

- [ ] **1.1** 在 `crates/hank-db/src/lib.rs` 加结构（放在 `TeamTask` 附近）：

```rust
/// 团队任务流水线的运行时配置。存在 settings 表的单行 JSON 里
/// （key = 'team_task_config'），admin 可改、改完即时生效。
///
/// 为什么用一行 JSON 而不是每个开关一个 settings key：
/// 这几个字段有互相约束（enabled=true 要求 task_gate_enabled=true、
/// roles 与 gates 取值要匹配），分成多行会出现「改了一半」的中间态。
/// 单行 JSON 保证一次写入是原子的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamTaskSettings {
    /// 两阶段闸门总开关（原 [server_agent].task_gate_enabled）
    pub task_gate_enabled: bool,
    /// 多角色流水线总开关（原 [team_task].enabled）
    pub enabled: bool,
    pub roles: Vec<String>,
    pub gates: Vec<String>,
    pub max_dev_rounds: i32,
    pub dashboard_base_url: Option<String>,
    /// 最后修改人（admin 用户名），审计用
    pub updated_by: Option<String>,
}
```

- [ ] **1.2** 访问器。复用现有 `settings` 表与 `get_setting` / `set_setting`，
  **不要新建表**：

```rust
/// settings 表里存团队任务配置的 key。
pub const TEAM_TASK_SETTINGS_KEY: &str = "team_task_config";

/// 读运行时配置。返回 Ok(None) 表示 DB 里还没有（调用方用 config.toml 兜底）。
/// JSON 解析失败也返回 Ok(None) 并 warn——配置坏了应该退回默认值继续跑，
/// 而不是让每个飞书任务都报错。
pub async fn get_team_task_settings(&self) -> Result<Option<TeamTaskSettings>>;

/// 覆盖写入。调用方（admin REST）必须先校验。
pub async fn save_team_task_settings(&self, s: &TeamTaskSettings) -> Result<()>;
```

### 步骤 2：运行时配置解析

- [ ] **2.1** 新建 `server/src/team_task/settings.rs`：

```rust
//! 团队任务运行时配置：DB 优先、config.toml 兜底。
//!
//! 为什么每次读 DB 而不缓存在 AppState：
//! 这些开关只在「派发一个任务」「推进一次状态机」时读，一个任务全程也就几次，
//! 不是每 token 都读。直接读 DB 换来的是「admin 改完立刻生效」和
//! 「没有缓存失效 bug」，这个取舍很划算。多实例共库时也天然一致。
```

- [ ] **2.2** 核心函数：

```rust
/// 取当前生效的运行时配置。
///
/// DB 有值用 DB；DB 没有（首次部署、或还没在 admin 里改过）则用 config.toml
/// 的值作为默认。这样升级上线时行为不变，不需要先去 admin 点一遍。
pub async fn effective(state: &AppState) -> TeamTaskSettings;

/// 从 config.toml 的两段配置拼出默认值（迁移兜底）。
pub fn defaults_from_config(cfg: &Config) -> TeamTaskSettings;
```

`effective` 读 DB 失败时**不要 bail**，用 config.toml 兜底并 `warn`——
DB 抖一下不该让所有飞书任务失败。

- [ ] **2.3** 校验纯函数（从 `config.rs` 搬过来并改造）：

```rust
/// 校验一份待写入的配置。返回 Err 时 admin REST 用它的消息回 400。
///
/// 与原先「启动时 bail」的区别：现在是写入时拒绝，用户点保存立刻看到原因，
/// 而不是重启后服务起不来才发现。
pub fn validate(s: &TeamTaskSettings) -> Result<(), String>;
```

校验项（与原 `config.rs` 的 `validate` 一致，错误信息改成面向 admin 用户的中文）：
- `enabled && !task_gate_enabled` → 「多角色流水线依赖两阶段闸门，请同时开启闸门」
- `enabled && roles.is_empty()` → 「至少要配置一个角色」
- `roles` 有未知角色名 → 列出合法值 `developer` / `reviewer` / `tester`
- `roles` 有重复项 → 拒绝（`next_role` 靠位置查找，重复会让流转错乱）
- `gates` 有未知边界名 → 列出合法值
- `enabled && max_dev_rounds < 1` → 「最大返工轮次至少为 1」
- `max_dev_rounds > 10` → 拒绝（防手滑输入 1000 导致 token 烧穿）

> 这里可以直接用 `team_task::ROLE_DEFS` 做合法性判断了 ——
> `settings.rs` 在 `team_task` 模块内，没有 `config.rs` 那个循环引用问题。
> 原先在 `config.rs` 里写的本地常量副本请**删掉**，避免两份清单漂移。

- [ ] **2.4** 单测：`defaults_from_config` 正确映射两段配置；
  `validate` 覆盖上面 7 种情况 + 一个全合法的通过用例；
  `enabled = false` 时其他字段非法**仍然拒绝**
  （与原先启动校验不同 —— 写入时应该严格，不能让 admin 存一份坏配置进去等着以后炸）。

### 步骤 3：admin REST

- [ ] **3.1** 新建 `server/src/team_task/routes.rs`，仿 `scheduler/routes.rs` 的风格：

```rust
/// GET /api/admin/team-task/config — 当前生效配置 + 元信息
pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse;

/// PATCH /api/admin/team-task/config — 改配置。校验失败回 400。
pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<UpdateConfigBody>,
) -> impl IntoResponse;
```

- [ ] **3.2** `GET` 返回：

```json
{
  "config": { ...TeamTaskSettings... },
  "source": "db" | "config_file",
  "role_options": [
    { "id": "developer", "label": "开发" },
    { "id": "reviewer", "label": "评审" },
    { "id": "tester", "label": "测试" }
  ],
  "gate_options": [
    { "id": "dev_start", "label": "开发前（分析后确认是否开始修）" },
    { "id": "review_start", "label": "进入评审前" },
    { "id": "dev_restart", "label": "评审打回后重新开发前" },
    { "id": "test_start", "label": "进入测试前" }
  ]
}
```

`role_options` / `gate_options` 由后端给，前端不要硬编码 —— 加角色时只改一处。
`source` 让前端能提示「当前还在用配置文件默认值」。

- [ ] **3.3** `UpdateConfigBody` 所有字段 `Option<T>`，只改传了的字段
  （PATCH 语义）。合并到 `effective()` 的结果上，再整体 `validate`，
  通过才 `save_team_task_settings`。`updated_by` 写 `claims.username`（或 `claims.sub`，
  按 `Claims` 实际字段来）。

- [ ] **3.4** 在 `server/src/main.rs` 的 `admin_api` 里挂两条路由
  （紧邻现有 `/api/admin/jobs` 那几条）：

```rust
.route("/api/admin/team-task/config", get(team_task::routes::get_config))
.route("/api/admin/team-task/config", patch(team_task::routes::update_config))
```

### 步骤 4：所有读取点改成运行时配置

逐个替换，**共 6 处**：

- [ ] **4.1** `server/src/cli_agent.rs:492` — `should_gate_turn` 的第一个参数。
  改成先 `let settings = crate::team_task::settings::effective(state).await;`
  再传 `settings.task_gate_enabled`。

- [ ] **4.2** `server/src/cli_agent.rs:2497` — `finish_as_task_gate` 里
  `if state.config.team_task.enabled` → `if settings.enabled`。

- [ ] **4.3** `server/src/interaction_flow.rs:241` — 分路判断里的 `enabled`。
  注意这是 `match` 的 guard，需要在 `match` 之前先 `await` 拿到 settings。

- [ ] **4.4** `server/src/team_task/orchestrator.rs:91` — worker 内的早退判断。

- [ ] **4.5** `server/src/team_task/orchestrator.rs:674` — `pick_upstream_run` 的
  `state.config.team_task.roles` → `settings.roles`。

- [ ] **4.6** `server/src/main.rs:243` — 启动收尾僵尸任务的判断。
  这里在 `AppState` 构造之后，可以 `await`。

- [ ] **4.7** `decide_next` 与 `dispatch_from_pending_gate` 的签名目前收
  `&TeamTaskConfig`（来自 `config.rs`）。改成收 `&TeamTaskSettings`。
  **这会影响第 2、4 步写的 69 项单测的构造代码**——
  逐个把 `TeamTaskConfig { ... }` 换成 `TeamTaskSettings { ... }`
  （多两个字段 `task_gate_enabled` 与 `updated_by`）。
  **断言逻辑一行都不要改**，只改构造。

> 提示：如果嫌 69 处构造改起来烦，可以在测试模块里加一个
> `fn test_settings(roles, gates, max) -> TeamTaskSettings` 辅助函数，
> 但**不要**为此改动任何断言。

### 步骤 5：清理 config.rs

- [ ] **5.1** 去掉 `Config::load` 里对 `team_task.validate(...)` 的调用
  和 `TeamTaskConfig::validate` 方法本身（校验搬到 `settings::validate`）。
  以及步骤 2.3 提到的本地角色/边界常量副本。

- [ ] **5.2** `TeamTaskConfig` 结构与 `[team_task]` 段**保留**——
  它现在的职责是「首次部署的默认值」。在结构的文档注释里改写清这一点：

```rust
/// 团队任务流水线的**初始默认值**。运行时真正生效的配置在数据库
/// （settings 表，admin 可改），见 team_task::settings::effective。
/// 这里的值只在 DB 里还没有配置时作为兜底，便于升级上线时行为不变。
```

- [ ] **5.3** `[server_agent].task_gate_enabled` 同样保留作默认值，
  注释改成指向 DB。**不要**动 `[server_agent]` 的其他字段——
  那些是部署基础设施（路径、执行用户、sandbox），本来就该在文件里。

- [ ] **5.4** `config.toml` 的 `[team_task]` 段加一行注释说明：

```toml
# 注意：以下只是「数据库里还没有配置时」的初始默认值。
# 运行时配置在 admin「团队任务」页，改完即时生效、无需重启。
[team_task]
```

## 明确边界

**不许碰**：
- `admin/`（前端页面是 05b）
- `client/`、`quant/`、`cli/`
- `server/src/feishu/`（主卡是第 6 步）
- `Cargo.toml`（不新增依赖）
- `CLAUDE.md`、`docs/`
- `[server_agent]` 除 `task_gate_enabled` 注释外的任何字段

**不许做**：
- 不要新建 settings 表，复用现有 `settings` 表 + `get_setting`/`set_setting`
- 不要把配置缓存进 `AppState`（步骤 2.1 注释解释了为什么）
- 不要改任何既有单测的**断言**，只允许改 `TeamTaskConfig` → `TeamTaskSettings` 的构造
- 不要写 admin 前端、飞书主卡、看板
- `effective()` 读 DB 失败时不要 bail

**保留**：第 1–5 步的全部成果。

## 验收标准

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test -p hank-server
cargo test --workspace
```

期望结果：
- 编译成功，`deployment.rs` 那 5 个既有 warning 属正常
- clippy 基线 **46**，无新增
- `cargo test -p hank-server team_task` ≥ 69 + 新增，全绿
- `cargo test -p hank-server` ≥ 202 + 新增，全绿
- `cargo test --workspace` 全绿

**手工验证**（本机起 server 即可，不需要飞书）：
- [ ] `curl -H "Authorization: Bearer $ADMIN_JWT" $SERVER/api/admin/team-task/config`
      → 返回配置，`source` 为 `config_file`
- [ ] `PATCH` 设 `{"enabled": true, "task_gate_enabled": true}` → 200，
      再 `GET` 确认 `source` 变 `db`
- [ ] `PATCH` 设 `{"enabled": true, "task_gate_enabled": false}` → **400**，
      错误信息说明依赖关系
- [ ] `PATCH` 设 `{"roles": ["developer", "designer"]}` → **400**，列出合法值
- [ ] `PATCH` 设 `{"roles": ["developer", "developer"]}` → **400**（重复项）
- [ ] `PATCH` 设 `{"max_dev_rounds": 1000}` → **400**
- [ ] 把 `config.toml` 的 `[team_task]` 整段删掉 → server 仍能正常启动
- [ ] **不重启 server**，`PATCH` 把 `enabled` 从 true 改成 false，
      观察日志确认下一次任务判定用的是新值（即时生效）

## 约定

- **中文注释**。四处必须写清「为什么」：
  1. 用单行 JSON 而非多个 settings key —— 字段间有互相约束，避免「改了一半」
  2. 每次读 DB 不缓存 —— 读取频率低，换来即时生效与无缓存失效 bug
  3. DB 优先、config.toml 兜底 —— 升级上线时行为不变
  4. 校验从启动时改成写入时 —— 点保存立刻看到错误，而不是重启后起不来
- **中文 commit message**，形如
  `refactor(team-task): 开关移到数据库，admin 可改且即时生效`
- `effective()` 返回 `TeamTaskSettings`（不是 `Result`）——失败已在内部兜底
- `validate` 返回 `Result<(), String>`，消息面向 admin 用户，用中文
