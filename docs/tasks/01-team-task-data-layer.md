# 任务 01：团队任务流水线 — 数据层（三张表 + hank-db CRUD）

> 本任务是 `docs/feature/team-task-pipeline.md` 实施顺序的**第 1 步**，共 9 步。
> 只做数据层，**不接任何现有链路，不改任何现有行为**。做完后 `cargo build --workspace`
> 通过、新增的纯函数单测通过即可验收。后续任务（状态机、编排器、飞书卡片、看板）
> 会有独立文档，本任务**不要**提前实现它们。

## 背景与目标

### 背景

飞书渠道现有的代码任务是**单角色两阶段**：第一轮只读分析 → 落 `task_gate` 交互单 →
用户点「开始修」→ 在同一 CLI thread 上 resume 第二轮执行。

现在要把「开始修」之后的执行，从**一次 run** 扩展成**开发 → 评审 → 测试三角色流水线**，
每个角色一次独立的 hank-cli run（各自独占 CLI thread），并配一个独立的 team 看板。

完整设计见 `docs/feature/team-task-pipeline.md`（**动手前请通读第 3、5 节**）。

### 本任务目标

建立三张新表与对应的 CRUD 方法，为后续编排器提供持久化基础：

- `team_tasks` — 任务主体（一个飞书话题任务 = 一行）
- `team_task_runs` — 角色轮次（一个角色的一次执行 = 一行）
- `team_task_events` — 任务级时间线（角色边界与人工决策）

### 做完之后的可观察效果

1. `cargo build --workspace` 通过。
2. `cargo test -p hank-db` 通过（含本任务新增的 `task_no` 生成单测）。
3. 启动 server 后连上 MySQL，三张表被自动创建（`SHOW TABLES LIKE 'team_%'` 出三行）。
4. 现有功能行为**完全不变**——本任务不改动任何既有代码路径，只新增。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `crates/hank-db/src/lib.rs` | **唯一需要改的文件**。新增 3 个 `CREATE TABLE` 到 `migrate` 段、3 个数据结构、1 组入参结构、约 11 个 CRUD 方法、1 个 `task_no` 生成函数 + 其单测 |

**没有其他文件需要改。** 不要动 `server/`、`admin/`、`client/`、`crates/code-agent/`、
`config.toml`、`CLAUDE.md`。

## 实现步骤

### 步骤 1：建表语句

位置：`crates/hank-db/src/lib.rs` 的 `Database::new()` 内。找到最后一张表
`agent_cli_profiles` 的 `.execute(&pool).await?;`（约 1394 行），在它之后、
紧接着的 `// 从单行结构（agent_cli_configs）迁移到多配置` 注释**之前**插入三段建表。

沿用文件里既有风格：`sqlx::query("CREATE TABLE IF NOT EXISTS ...").execute(&pool).await?;`，
`DEFAULT CHARSET=utf8mb4`，中文注释说明设计意图。

- [ ] **1.1 `team_tasks`**

```rust
// 团队任务：一个飞书话题任务的主体，串起开发/评审/测试多个角色轮次。
// session_id 不加外键——与 channel_messages 同理，session 清理后仍要保留任务审计快照。
sqlx::query(
    "CREATE TABLE IF NOT EXISTS team_tasks (
        id VARCHAR(36) PRIMARY KEY,
        task_no VARCHAR(32) NOT NULL,
        session_id VARCHAR(36) NOT NULL,
        user_id VARCHAR(36) NOT NULL,
        source VARCHAR(32) NOT NULL DEFAULT 'feishu',
        issue_key VARCHAR(64) DEFAULT NULL,
        title VARCHAR(255) NOT NULL DEFAULT '',
        goal TEXT DEFAULT NULL,
        analysis MEDIUMTEXT DEFAULT NULL,
        status VARCHAR(32) NOT NULL DEFAULT 'pending_confirm',
        current_role VARCHAR(32) DEFAULT NULL,
        dev_rounds INT NOT NULL DEFAULT 0,
        backend VARCHAR(32) NOT NULL,
        exec_client_id VARCHAR(36) DEFAULT NULL,
        agent_kind VARCHAR(32) NOT NULL DEFAULT 'general_task',
        account_id VARCHAR(36) DEFAULT NULL,
        chat_id VARCHAR(128) DEFAULT NULL,
        topic_id VARCHAR(128) DEFAULT NULL,
        card_message_id VARCHAR(256) DEFAULT NULL,
        result MEDIUMTEXT DEFAULT NULL,
        error TEXT DEFAULT NULL,
        created_at DATETIME NOT NULL DEFAULT NOW(),
        updated_at DATETIME NOT NULL DEFAULT NOW(),
        finished_at DATETIME DEFAULT NULL,
        UNIQUE KEY uk_team_tasks_no (task_no),
        INDEX idx_team_tasks_status (status, updated_at),
        INDEX idx_team_tasks_session (session_id, created_at),
        INDEX idx_team_tasks_user (user_id, created_at)
    ) DEFAULT CHARSET=utf8mb4",
)
.execute(&pool)
.await?;
```

- [ ] **1.2 `team_task_runs`**

注释里要写清 `UNIQUE KEY uk_team_run_role_round` 的用意：
它是并发防线，编排器重复派发同一角色同一轮会插入失败而不是起两个并发 run。

```rust
// 角色轮次：一个角色的一次执行。评审打回后重新开发会新增一行（round+1），
// 历史轮次全部保留，不覆盖。
// uk_team_run_role_round 是并发防线：编排器重复派发同一角色同一轮会插入失败，
// 而不是起两个并发 run（与进程内 TaskRegistry 名额互补，后者防同 session 并发）。
sqlx::query(
    "CREATE TABLE IF NOT EXISTS team_task_runs (
        id VARCHAR(36) PRIMARY KEY,
        task_id VARCHAR(36) NOT NULL,
        role VARCHAR(32) NOT NULL,
        round INT NOT NULL DEFAULT 1,
        thread_id VARCHAR(128) DEFAULT NULL,
        status VARCHAR(16) NOT NULL DEFAULT 'running',
        verdict VARCHAR(16) DEFAULT NULL,
        handoff JSON DEFAULT NULL,
        summary MEDIUMTEXT DEFAULT NULL,
        dirty_files INT DEFAULT NULL,
        error TEXT DEFAULT NULL,
        started_at DATETIME NOT NULL DEFAULT NOW(),
        finished_at DATETIME DEFAULT NULL,
        UNIQUE KEY uk_team_run_role_round (task_id, role, round),
        INDEX idx_team_runs_task (task_id, started_at),
        FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
    ) DEFAULT CHARSET=utf8mb4",
)
.execute(&pool)
.await?;
```

- [ ] **1.3 `team_task_events`**

```rust
// 任务级时间线：只记角色边界与人工决策，一个任务通常十几行。
// 与 agent_events（单 run 内的细粒度事件）分开，避免看板读一次时间线拉出上万行。
sqlx::query(
    "CREATE TABLE IF NOT EXISTS team_task_events (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        task_id VARCHAR(36) NOT NULL,
        kind VARCHAR(32) NOT NULL,
        role VARCHAR(32) DEFAULT NULL,
        round INT DEFAULT NULL,
        operator VARCHAR(64) DEFAULT NULL,
        detail TEXT DEFAULT NULL,
        created_at DATETIME NOT NULL DEFAULT NOW(),
        INDEX idx_team_events_task (task_id, id),
        FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
    ) DEFAULT CHARSET=utf8mb4",
)
.execute(&pool)
.await?;
```

### 步骤 2：数据结构

放在 `AgentInteraction` / `NewInteraction` 那一组结构体附近（约 175–230 行区域之后），
沿用同款 derive：`#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]`。

- [ ] **2.1 `TeamTask`**

```rust
/// 团队任务：串起开发/评审/测试多角色轮次的任务主体。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TeamTask {
    pub id: String,
    /// 人类可读短编号 tsk_xxx_xxxx，飞书卡片与看板深链都用它
    pub task_no: String,
    pub session_id: String,
    pub user_id: String,
    /// feishu / dashboard（后续接 issue 巡检时再加取值）
    pub source: String,
    /// 外部 issue 标识，如 IK5MOR；本期不巡检，仅从飞书原文 #KEY 解析或看板手填
    pub issue_key: Option<String>,
    pub title: String,
    pub goal: Option<String>,
    /// 分析轮产出的四段 markdown。与 agent_interactions.analysis 同源冗余一份，
    /// 便于看板与后续角色 prompt 直接取用，不必回查交互单。
    pub analysis: Option<String>,
    /// pending_confirm / running_developer / pending_review_gate / running_reviewer
    /// / pending_dev_gate / running_tester / done / failed / cancelled
    pub status: String,
    /// developer / reviewer / tester；终态时为 None
    pub current_role: Option<String>,
    /// 开发轮已用轮次，用于 max_dev_rounds 上限判定
    pub dev_rounds: i32,
    /// 执行后端与节点在整条流水线内固定，中途不换节点
    pub backend: String,
    pub exec_client_id: Option<String>,
    pub agent_kind: String,
    pub account_id: Option<String>,
    pub chat_id: Option<String>,
    pub topic_id: Option<String>,
    /// 飞书任务主卡：跨角色复用同一张卡片原地刷新
    pub card_message_id: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
```

- [ ] **2.2 `TeamTaskRun`** — 字段与 §5.2 建表一一对应，
  `round` / `dirty_files` 用 `i32` / `Option<i32>`，`handoff` / `summary` 用 `Option<String>`。
  注释说明 `verdict` 取值 `pass` / `reject` / `failed` / `unknown`，
  且 `unknown` 表示「模型输出没解析出结论，需人工介入」。

- [ ] **2.3 `TeamTaskEvent`** — `id: i64`，其余对应建表。
  注释列出 `kind` 取值：`role_started` / `role_finished` / `gate_opened` /
  `gate_answered` / `rejected` / `status_changed` / `cancelled`。

- [ ] **2.4 `NewTeamTask<'a>`** — 创建入参，仿照既有 `NewInteraction<'a>`
  用 `#[derive(Debug, Clone)]` + 借用字段，避免位置参数触发 `too_many_arguments`：

```rust
/// 创建团队任务的入参（字段多，避免位置参数触发 too_many_arguments）。
#[derive(Debug, Clone)]
pub struct NewTeamTask<'a> {
    pub session_id: &'a str,
    pub user_id: &'a str,
    pub source: &'a str,
    pub issue_key: Option<&'a str>,
    pub title: &'a str,
    pub goal: Option<&'a str>,
    pub analysis: Option<&'a str>,
    pub backend: &'a str,
    pub exec_client_id: Option<&'a str>,
    pub agent_kind: &'a str,
    pub account_id: Option<&'a str>,
    pub chat_id: Option<&'a str>,
    pub topic_id: Option<&'a str>,
}
```

### 步骤 3：`task_no` 生成（纯函数 + 单测）

- [ ] **3.1** 在 `crates/hank-db/src/lib.rs` 文件末尾的自由函数区
  （`extract_preview` 附近）加：

```rust
/// 生成人类可读的任务短编号：`tsk_{base36 毫秒时间戳}_{4 位 base36 随机}`。
///
/// 时间戳部分保证大致有序（便于人眼排序与肉眼判断新旧），随机部分避免同毫秒撞车。
/// 真正的唯一性由 team_tasks.uk_team_tasks_no 兜底，调用方撞键时重试一次即可。
pub fn generate_task_no(now_ms: u64, rand_seed: u64) -> String {
    fn base36(mut n: u64, width: usize) -> String {
        const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut buf = Vec::new();
        while n > 0 {
            buf.push(DIGITS[(n % 36) as usize]);
            n /= 36;
        }
        while buf.len() < width {
            buf.push(b'0');
        }
        buf.reverse();
        String::from_utf8(buf).expect("base36 digits are ascii")
    }
    format!("tsk_{}_{}", base36(now_ms, 1), base36(rand_seed % (36 * 36 * 36 * 36), 4))
}
```

注意 `rand_seed` 作为参数传入而非内部取随机，是为了单测可控。生产调用点用
`Uuid::new_v4().as_u128() as u64` 或 `std::time::SystemTime` 纳秒位作为种子——
本 crate 已依赖 `uuid`，**不要**为此新增 `rand` 依赖。

- [ ] **3.2** 在文件末尾新增 `#[cfg(test)] mod tests`（本文件目前没有测试模块，
  需要新建），覆盖：
  - 格式形如 `tsk_<非空>_<4 位>`，总长 ≤ 32（受 `VARCHAR(32)` 约束）
  - 同一 `now_ms` 不同 `rand_seed` 产出不同编号
  - `rand_seed = 0` 时随机段为 `0000`（补零逻辑正确）
  - 时间戳递增时编号的字典序也递增（同一位数下）

### 步骤 4：CRUD 方法

全部加在 `impl Database` 内，放在 `supersede_pending_task_gates`（约 4469 行）
之后、`// 渠道消息留档` 注释之前，形成一个「团队任务」区块并加区块注释。

**统一要求**：
- 所有 DB 调用包 `db_retry!`（与文件内其余方法一致）
- id 用 `Uuid::new_v4().to_string()`
- 时间用 `Utc::now()`
- `SELECT` 显式列出字段名，不用 `SELECT *`（与 `list_interactions` 一致）
- 可选筛选用 `? IS NULL OR col = ?` 双绑定，不拼 SQL 字符串

- [ ] **4.1 `create_team_task`**

```rust
/// 创建团队任务。task_no 撞唯一键时用新随机种子重试一次；仍失败则返回错误
/// （连撞两次说明不是巧合，静默重试到成功会掩盖真实问题）。
pub async fn create_team_task(&self, input: NewTeamTask<'_>) -> Result<TeamTask>
```

实现要点：生成 `task_no` → INSERT（status 默认 `pending_confirm`，`dev_rounds` 0）→
若报唯一键冲突则换种子再试一次 → 成功后 `get_team_task(&id)` 回读返回。

- [ ] **4.2 `get_team_task`** — 按 `id` 查，返回 `Result<Option<TeamTask>>`。

- [ ] **4.3 `get_team_task_by_no`** — 按 `task_no` 查（看板深链用），
  返回 `Result<Option<TeamTask>>`。

- [ ] **4.4 `get_team_task_by_session`** — 按 `session_id` 查**最新一条未终态**任务
  （`status NOT IN ('done','failed','cancelled')` ORDER BY `created_at` DESC LIMIT 1）。
  编排器要从 run 终态反查任务时用。返回 `Result<Option<TeamTask>>`。

- [ ] **4.5 `list_team_tasks`** — 分页 + 可选筛选，签名仿 `list_interactions`：

```rust
pub async fn list_team_tasks(
    &self,
    status: Option<&str>,
    user_id: Option<&str>,
    issue_key: Option<&str>,
    page: u32,
    per_page: u32,
) -> Result<(Vec<TeamTask>, i64)>
```

- [ ] **4.6 `update_team_task_status`**

```rust
/// 推进任务状态。current_role 传 None 表示清空（终态）。
/// 终态（done/failed/cancelled）自动写 finished_at；非终态不动它。
pub async fn update_team_task_status(
    &self,
    task_id: &str,
    status: &str,
    current_role: Option<&str>,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<()>
```

`finished_at` 用 SQL 条件写入：`finished_at = IF(? IN ('done','failed','cancelled'), NOW(), finished_at)`。

- [ ] **4.7 `set_team_task_card`** — 写 `card_message_id`（仿 `set_interaction_card`）。

- [ ] **4.8 `bump_team_task_dev_rounds`** — `dev_rounds = dev_rounds + 1`，
  返回递增后的值（`Result<i32>`），供 `max_dev_rounds` 上限判定用。
  用一条 UPDATE + 一条 SELECT，不必事务。

- [ ] **4.9 `insert_team_run`**

```rust
/// 插入角色轮次行。唯一键 (task_id, role, round) 冲突时返回 Ok(None)——
/// 这是编排器重复派发的正常防线，不是错误，调用方据此跳过派发。
pub async fn insert_team_run(
    &self,
    task_id: &str,
    role: &str,
    round: i32,
) -> Result<Option<TeamTaskRun>>
```

唯一键冲突判定：`sqlx::Error::Database(e)` 且 `e.code()` 为 `"23000"`（MySQL 重复键）
时返回 `Ok(None)`，其他错误照常 `Err`。**注意 `db_retry!` 会把错误转成 `anyhow::Error`**，
这里需要不经 `db_retry!` 直接 `match` sqlx 错误，或先用 `downcast_ref::<sqlx::Error>()`
判定——任选一种，但要在注释里说明为什么这个方法的错误处理与其他方法不同。

- [ ] **4.10 `set_team_run_thread`** — 写某 run 行的 `thread_id`
  （run 的首个事件回来后由编排器调用）。

- [ ] **4.11 `finish_team_run`**

```rust
/// 角色轮次收尾：写状态、verdict、交接产物、摘要、改动文件数、错误与 finished_at。
/// status 取 finished / failed / cancelled。
#[allow(clippy::too_many_arguments)]
pub async fn finish_team_run(
    &self,
    run_id: &str,
    status: &str,
    verdict: Option<&str>,
    handoff: Option<&str>,
    summary: Option<&str>,
    dirty_files: Option<i32>,
    error: Option<&str>,
) -> Result<()>
```

- [ ] **4.12 `list_team_runs`** — 按 `task_id` 查全部轮次，`ORDER BY started_at ASC`，
  返回 `Result<Vec<TeamTaskRun>>`。

- [ ] **4.13 `latest_team_run`** — 按 `task_id` 查最新一行
  （`ORDER BY started_at DESC LIMIT 1`），返回 `Result<Option<TeamTaskRun>>`。

- [ ] **4.14 `append_team_event`**

```rust
/// 记一条任务时间线事件。旁路日志语义：写失败不应影响任务本身，
/// 调用方通常 `let _ =` 或只 warn，不向上传播。
pub async fn append_team_event(
    &self,
    task_id: &str,
    kind: &str,
    role: Option<&str>,
    round: Option<i32>,
    operator: Option<&str>,
    detail: Option<&str>,
) -> Result<()>
```

- [ ] **4.15 `list_team_events`** — 按 `task_id` 查，`ORDER BY id ASC`，
  返回 `Result<Vec<TeamTaskEvent>>`。

- [ ] **4.16 `fail_stale_team_tasks`**

```rust
/// 进程启动时收尾遗留的运行中任务：server 重启后 CLI thread 已不可信，
/// 一律标失败而不是尝试续跑（仿 scheduler 对遗留 running job_run 的收尾）。
/// 返回被收尾的任务数。
pub async fn fail_stale_team_tasks(&self) -> Result<u64>
```

把 `status LIKE 'running_%'` 的任务标 `failed`（error 写「server 重启，运行中任务已中断」、
写 `finished_at`、清 `current_role`），同时把这些任务下 `status='running'` 的 run 行
标 `failed`。**本任务只提供方法，不在 `main.rs` 调用**——接入放在后续任务。

## 明确边界

**不许碰的文件/模块**：
- `server/` 下所有文件（含 `main.rs`、`cli_agent.rs`、`interaction_flow.rs`、`feishu/`）
- `admin/`、`client/`、`quant/`、`cli/`
- `crates/code-agent/`、`crates/code-tools/`、`crates/hank-provider/`、`crates/hank-a2a-client/`
- `config.toml`、`Cargo.toml`（**不要新增任何依赖**，`uuid` / `sqlx` / `chrono` / `serde` 已够用）
- `CLAUDE.md`、`docs/feishu.md`

**不许做的事**：
- 不要改动 `agent_interactions` 表结构或它的任何方法（后续任务才加 `team_gate` kind）
- 不要新建 `server/src/team_task/` 目录（第 2、4 步任务的事）
- 不要写状态机流转逻辑、prompt、卡片构造（后续任务）
- 不要 `ALTER TABLE` 任何既有表
- 不要删除或重命名任何既有方法

**保留工作区原有改动**：`docs/feature/team-task-pipeline.md` 是未提交的新文件，
是本任务的设计依据，**不要删除、不要修改它**。除本任务涉及的文件外不要 `git checkout`
或回退任何内容。

## 验收标准

依次执行，全部通过才算完成：

```bash
# 1. 编译（必须零错误零警告）
cargo build --workspace

# 2. clippy（本项目对 too_many_arguments 敏感，注意用入参结构或 allow 属性）
cargo clippy -p hank-db --all-targets

# 3. 单测（generate_task_no 的测试）
cargo test -p hank-db

# 4. 确认没有误改其他文件——应当只有 crates/hank-db/src/lib.rs 一处改动
#    （docs/feature/team-task-pipeline.md 是本任务之前就存在的未跟踪文件，属正常）
git status --short
git diff --stat
```

期望结果：
- `cargo build --workspace` 成功，无新增 warning
- `cargo clippy -p hank-db --all-targets` 无 error、无新增 warning
- `cargo test -p hank-db` 全绿，`generate_task_no` 的 4 项断言通过
- `git diff --stat` 只列出 `crates/hank-db/src/lib.rs`

> 说明：本任务不要求连真实 MySQL 跑集成测试。建表语句的正确性由后续任务
> 首次启动 server 时验证；本任务只需保证编译与纯函数测试通过。

## 约定

遵循 `CLAUDE.md`：

- **中文注释**：所有新增结构体字段、方法、建表语句都要有中文注释，
  说明「为什么这样设计」而不只是「这是什么」。参考本文件给出的注释范例，
  以及 `crates/hank-db/src/lib.rs` 里 `AgentInteraction` 的注释密度。
- **中文 commit message**：形如
  `feat(team-task): 团队任务流水线数据层，三张表与 CRUD`
- 后端错误处理用 `anyhow::Result`，与文件内既有风格一致
- 命名与既有表保持对称：`team_` 前缀、`uk_` / `idx_` 索引前缀、
  方法名沿用 `create_ / get_ / list_ / update_ / set_ / append_` 动词习惯
- 不要引入新的抽象层或 trait，这一层就是直白的 SQL + 结构体映射
