# 任务 02：团队任务流水线 — 状态机纯函数 + 配置段

> 本任务是 `docs/feature/team-task-pipeline.md` 实施顺序的**第 2 步**，共 9 步。
> 第 1 步（数据层三张表 + hank-db CRUD）已完成并合入工作区。
>
> 本任务只写**纯函数与配置结构**，**不接任何现有链路**：不派发 run、不发卡片、
> 不读写数据库、不改任何既有代码路径。做完后编译通过、新增单测全绿即可验收。
> 后续任务（角色 prompt、编排器、飞书主卡、REST、看板）会有独立文档，
> 本任务**不要**提前实现它们。

## 背景与目标

### 背景

飞书渠道现有的代码任务是**单角色两阶段**：第一轮只读分析 → 落 `task_gate` 交互单 →
用户点「开始修」→ 在同一 CLI thread 上 resume 第二轮执行。

现在要把「开始修」之后的执行扩展成**开发 → 评审 → 测试三角色流水线**，
每个角色一次独立的 hank-cli run（各自独占 CLI thread）。

完整设计见 `docs/feature/team-task-pipeline.md`，**动手前必须通读第 4 节（状态机）
与第 6.3 节（交接产物）**。第 1 步产出的数据结构见 `crates/hank-db/src/lib.rs`
里的 `TeamTask` / `TeamTaskRun` / `TeamTaskEvent` / `NewTeamTask`。

### 本任务目标

1. 新增 `[team_task]` 配置段（全字段带默认值，不写该段也能启动）。
2. 新建 `server/src/team_task/mod.rs`，提供：
   - 角色注册表 `ROLE_DEFS` 与查表辅助函数
   - 状态常量与判定辅助
   - `Verdict` 枚举与宽松解析
   - **状态机核心纯函数 `decide_next`**
   - **交接产物解析纯函数 `parse_handoff`**
3. 为上述纯函数写完整单测。

### 做完之后的可观察效果

1. `cargo build --workspace` 通过，无新增 warning。
2. `cargo test -p hank-server` 通过，新增的 `team_task` 与 `config` 单测全绿。
3. 现有功能行为**完全不变**——`decide_next` 尚无调用方，`[team_task].enabled`
   默认 `false`。
4. `config.toml` **不需要改动**也能正常启动 server。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `server/src/config.rs` | 新增 `TeamTaskConfig` 结构 + `Config.team_task` 字段 + 默认值函数 + 新增 `#[cfg(test)] mod tests` 覆盖 TOML 反序列化默认值 |
| `server/src/team_task/mod.rs` | **新建**。角色注册表、状态常量、`Verdict`、`decide_next`、`parse_handoff` 及其单测 |
| `server/src/main.rs` | **只加一行** `mod team_task;`（按字母序插在 `mod task_state;` 之后、`mod termshot;` 之前）|

**没有其他文件需要改。**

## 实现步骤

### 步骤 1：配置段

- [ ] **1.1** 在 `server/src/config.rs` 的 `Config` 结构里加字段（放在 `quant_a2a` 之后）：

```rust
    /// 团队任务流水线（开发→评审→测试多角色编排）。默认关闭。
    #[serde(default)]
    pub team_task: TeamTaskConfig,
```

- [ ] **1.2** 新增结构体（放在 `QuantA2aConfig` 之后，`ServerConfig` 之前）：

```rust
/// 团队任务流水线配置。所有字段都有默认值，因此 config.toml 不写 [team_task]
/// 段也能启动（此时 enabled = false，行为与未接入前完全一致）。
#[derive(Debug, Clone, Deserialize)]
pub struct TeamTaskConfig {
    /// 总开关。关闭时 task_gate 走原来的单角色两阶段路径，行为与今天一致。
    #[serde(default)]
    pub enabled: bool,
    /// 参与流水线的角色，按数组顺序流转。可裁剪成 ["developer"] 只跑单角色。
    /// 未知角色名在启动校验时拒绝（见 validate），不静默丢弃。
    #[serde(default = "default_team_roles")]
    pub roles: Vec<String>,
    /// 需要人工确认的边界。默认只保留现有的开发前闸门，其余自动流转——
    /// 一上来每个角色边界都弹卡片，四次点击才跑完一个任务，体验会劝退。
    #[serde(default = "default_team_gates")]
    pub gates: Vec<String>,
    /// 评审打回后最多重新开发几轮，超出即 failed。
    /// 没有上限的话，评审和开发能在一个错误理解上互相打回到 token 烧穿。
    #[serde(default = "default_max_dev_rounds")]
    pub max_dev_rounds: i32,
    /// 看板外部可访问地址，用于飞书卡片深链。留空则卡片不渲染看板链接行，
    /// 而不是拼出一个坏链接（与 server.admin_base_url 同款约定）。
    #[serde(default)]
    pub dashboard_base_url: Option<String>,
}

impl Default for TeamTaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roles: default_team_roles(),
            gates: default_team_gates(),
            max_dev_rounds: default_max_dev_rounds(),
            dashboard_base_url: None,
        }
    }
}

fn default_team_roles() -> Vec<String> {
    vec![
        "developer".to_string(),
        "reviewer".to_string(),
        "tester".to_string(),
    ]
}

fn default_team_gates() -> Vec<String> {
    vec!["dev_start".to_string()]
}

fn default_max_dev_rounds() -> i32 {
    3
}
```

- [ ] **1.3** 在 `server/src/config.rs` 末尾新增 `#[cfg(test)] mod tests`
  （该文件目前没有测试模块，需新建），用 `toml::from_str::<Config>` 覆盖：
  - 完全不写 `[team_task]` 段 → `enabled == false`、`roles` 三个、`gates` 一个、
    `max_dev_rounds == 3`、`dashboard_base_url == None`
  - 只写 `[team_task]\nenabled = true` → 其余字段仍是默认值
  - 显式覆盖 `roles = ["developer"]` → 只有一个角色，且 `gates` 不受影响

  测试用的最小 TOML 需要带 `[server]` 段必填字段（`host` / `port` / `jwt_secret`
  / `database_url`），照 `config.toml` 现有字段名写即可。

### 步骤 2：新建模块骨架

- [ ] **2.1** 新建目录与文件 `server/src/team_task/mod.rs`，文件头写模块级注释：

```rust
//! 团队任务流水线：开发 → 评审 → 测试的多角色编排。
//!
//! 本模块只放**纯函数与类型**，不做任何 IO：
//! - `decide_next`：状态机唯一判定点。分支多，必须能单测；走 DB 测既慢又覆盖不全。
//! - `parse_handoff`：从角色输出正文里提取结构化交接产物。
//!
//! 派发 run、发卡片、读写 DB 都在后续的 `orchestrator` 里，不要写进这里。
```

- [ ] **2.2** 在 `server/src/main.rs` 加 `mod team_task;`（按字母序，`mod task_state;`
  之后、`mod termshot;` 之前）。**不要**动 `main.rs` 的其他任何内容——
  不加路由、不改 `AppState`、不调用任何新函数。

### 步骤 3：角色注册表与状态常量

- [ ] **3.1** 角色注册表。沿用 `scheduler::JOB_DEFS` 的「定义在代码、状态在 DB」约定：

```rust
/// 角色定义。加第四个角色（如「文档」）只需往 ROLE_DEFS 加一行 + 写 prompt 函数，
/// 流转顺序由配置的 roles 数组顺序决定。
pub struct RoleDef {
    pub id: &'static str,
    /// 卡片与看板展示用中文名
    pub label: &'static str,
    /// 该角色运行中对应的 team_tasks.status
    pub running_status: &'static str,
    /// 该角色是否要求结构化 verdict（评审/测试要，开发不要）
    pub needs_verdict: bool,
}

pub const ROLE_DEFS: &[RoleDef] = &[
    RoleDef { id: "developer", label: "开发", running_status: "running_developer", needs_verdict: false },
    RoleDef { id: "reviewer",  label: "评审", running_status: "running_reviewer",  needs_verdict: true  },
    RoleDef { id: "tester",    label: "测试", running_status: "running_tester",    needs_verdict: true  },
];

/// 按 id 查角色定义；未知 id 返回 None（调用方转成用户可见错误，不 panic）。
pub fn role_def(id: &str) -> Option<&'static RoleDef>;

/// 按配置的 roles 顺序取下一个角色；已是最后一个返回 None。
/// 注意用**配置顺序**而非 ROLE_DEFS 顺序——配置可裁剪成 ["developer"]。
pub fn next_role(roles: &[String], current: &str) -> Option<String>;

/// 配置里的第一个角色（流水线入口）。roles 为空返回 None。
pub fn first_role(roles: &[String]) -> Option<String>;
```

- [ ] **3.2** 状态常量。用 `&'static str` 常量而非枚举——
  这些值要在 DB、卡片、REST 三处流转，枚举会在每个边界多一次转换：

```rust
pub const STATUS_PENDING_CONFIRM: &str = "pending_confirm";
pub const STATUS_PENDING_REVIEW_GATE: &str = "pending_review_gate";
pub const STATUS_PENDING_DEV_GATE: &str = "pending_dev_gate";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";

/// 是否终态。终态任务收到任何 Trigger 都应 Ignore。
pub fn is_terminal(status: &str) -> bool;
/// 是否某角色运行中（status 形如 running_*）。
pub fn is_running(status: &str) -> bool;
```

### 步骤 4：`Verdict` 与宽松解析

- [ ] **4.1**

```rust
/// 角色自评结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Reject,
    /// 模型输出没解析出结论。**不是**「需要人工确认」——见 decide_next 的处理。
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str;
    /// 宽松解析：大小写不敏感，容忍首尾空白与结尾标点（句号/逗号/分号，全角半角）。
    /// 识别 pass/通过/approved 与 reject/打回/不通过/rejected；其余一律 Unknown。
    pub fn parse(raw: &str) -> Self;
}
```

### 步骤 5：`decide_next`（核心）

- [ ] **5.1** 类型定义：

```rust
/// decide_next 的输入快照（字段多，避免位置参数）。
#[derive(Debug, Clone)]
pub struct DecideInput<'a> {
    pub status: &'a str,
    /// 当前角色；终态或待确认时可为 None
    pub current_role: Option<&'a str>,
    /// 已用开发轮次（team_tasks.dev_rounds）
    pub dev_rounds: i32,
    pub trigger: Trigger<'a>,
}

#[derive(Debug, Clone)]
pub enum Trigger<'a> {
    /// 闸门被应答（飞书按钮 / admin 手动应答共用）
    GateAnswered { answer: &'a str },
    /// 某角色 run 走到终态
    RunFinished { role: &'a str, round: i32, outcome: RunOutcome },
    /// 看板或飞书 /stop 取消
    Cancelled { operator: &'a str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// run 正常结束，带角色自评结论（无 verdict 的角色传 Pass）
    Finished(Verdict),
    /// run 本身失败（节点离线、超时、CLI 报错）
    Failed,
}

/// 状态机判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 派发某角色的某一轮
    DispatchRole { role: String, round: i32 },
    /// 开人工闸门
    OpenGate { boundary: GateBoundary },
    /// 走终态
    Finish { status: &'static str, reason: Option<String> },
    /// 什么都不做（重复触发、陈旧回调、终态被再次推进）
    Ignore { reason: String },
}

/// 人工闸门边界。四个变体都是「进入下一个角色」语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateBoundary {
    DevStart,
    ReviewStart,
    DevRestart,
    TestStart,
}

impl GateBoundary {
    /// 配置 gates 数组里的取值：dev_start / review_start / dev_restart / test_start
    pub fn as_str(self) -> &'static str;
    /// 该边界对应的等待状态（pending_*_gate）
    pub fn pending_status(self) -> &'static str;
}
```

- [ ] **5.2** 主函数签名。**必须是纯函数**：不接 `AppState`、不接 DB、不 `async`：

```rust
/// 状态机唯一判定点。纯函数：同样的输入永远给同样的输出，可单测全分支。
pub fn decide_next(input: &DecideInput<'_>, cfg: &TeamTaskConfig) -> Decision;
```

- [ ] **5.3** 流转规则，按此顺序判定：

**规则 A — 终态与取消**
1. `is_terminal(status)` → `Ignore`（reason 说明「任务已是终态 xxx」）
2. `Trigger::Cancelled` → `Finish { cancelled, reason: "由 {operator} 取消" }`

**规则 B — 待确认（pending_confirm，即现有 task_gate）**
3. `status == pending_confirm` + `GateAnswered`：
   - 肯定应答 → `DispatchRole { first_role(cfg.roles), round: 1 }`；
     `roles` 为空则 `Finish { failed, reason: "未配置任何角色" }`
   - 否定应答 → `Finish { cancelled, reason: "用户跳过" }`
   - 无法识别 → `Ignore`
4. `status == pending_confirm` + 其他 Trigger → `Ignore`

**规则 C — 角色运行中收到 RunFinished**
5. `RunFinished.role != current_role` → `Ignore`（陈旧回调，reason 写清两者）
6. `RunOutcome::Failed` → `Finish { failed, reason: "{label} 角色执行失败" }`
7. `RunOutcome::Finished(Verdict::Unknown)` →
   **`Finish { failed, reason: "{label} 结论无法解析" }`**

   > ⚠️ 这条是本任务最容易写错的地方。**不要**开人工闸门。理由见设计文档 §6.3：
   > (a) `gates = []` 语义是全自动无人值守，而飞书交互单不过期，开闸门会让任务
   > 永远挂着等一个不会有人点的按钮，僵尸态比 failed 更糟；
   > (b) 最后一个角色（tester）返回 Unknown 时**没有边界可开**——`GateBoundary`
   > 四个变体全是「进入下一个角色」语义，tester 之后是 `Finish { done }`，
   > 不存在「最终验收」边界；
   > (c) Unknown 是异常路径，加人工兜底会掩盖 prompt 的问题。
   >
   > 三个角色的 Unknown 都走这一条，**不分角色特例**。

8. `RunOutcome::Finished(Verdict::Pass)`：
   - `next_role(cfg.roles, current)` 有下一个角色 →
     该边界在 `cfg.gates` 里则 `OpenGate`，否则 `DispatchRole { next, round: 1 }`
   - 没有下一个角色 → `Finish { done, reason: None }`
9. `RunOutcome::Finished(Verdict::Reject)`：
   - 当前角色 `needs_verdict == false`（即开发角色）→
     `Finish { failed, reason: "开发角色返回 reject，prompt 或解析异常" }`。
     开发不该有 reject 语义，静默当打回会掩盖真 bug。
   - `dev_rounds >= cfg.max_dev_rounds` →
     `Finish { failed, reason: "已达最大返工轮次 {n}，请人工接手" }`
   - 否则 → `dev_restart` 在 `gates` 里则 `OpenGate { DevRestart }`，
     否则 `DispatchRole { "developer", round: dev_rounds + 1 }`

**规则 D — 等待闸门（pending_*_gate）收到 GateAnswered**
10. 肯定应答 → 按当前 pending 状态派发对应角色：
    - `pending_review_gate` → `next_role` 之后的角色，round 1
    - `pending_dev_gate` → `developer`，round `dev_rounds + 1`
11. 否定应答 → `Finish { cancelled, reason: "用户在 {边界} 终止" }`
12. 无法识别 → `Ignore`

**规则 E — 兜底**
13. 任何其他 status 与 Trigger 的组合 → `Ignore`，reason 里写清
    「status=xxx 不接受 trigger=yyy」，便于排查。

- [ ] **5.4** 闸门应答语义解析：

```rust
/// 闸门应答是肯定还是否定。比较前 trim()。
/// 肯定：开始修 / 继续 / 继续评审 / 重新开发 / 继续测试 / 确认 / 是
/// 否定：跳过 / 终止 / 取消 / 否
/// 其余返回 None（调用方转成 Ignore，不猜）
pub fn gate_answer_is_yes(answer: &str) -> Option<bool>;
```

### 步骤 6：`parse_handoff`

- [ ] **6.1**

```rust
/// 角色输出末尾的结构化交接产物。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handoff {
    pub verdict: Option<Verdict>,
    pub changed_files: Option<i32>,
    pub summary: Option<String>,
    /// 阻塞项；原文写 none / 无 / 空 一律归一成 None
    pub blocking: Option<String>,
}

/// 从 run 的最终文本里宽松提取「## 交接」段。
///
/// 宽松是刻意的——模型常加前言、改标题层级（##/###）、漏字段、用全角冒号。
/// 解析失败**不算 run 失败**：全文找不到任何可识别字段时返回 Handoff::default()，
/// 由调用方按角色兜底（开发角色用 git diff 补 changed_files，
/// 评审/测试角色的 verdict 为 None 时视为 Verdict::Unknown → 任务 failed）。
pub fn parse_handoff(text: &str) -> Handoff;
```

- [ ] **6.2** 解析规则：
  - 定位标题：`##` 或 `###` 开头且含「交接」的行；找不到就退化为全文扫 `key: value`
  - 键名大小写不敏感，容忍半角 `:` 与全角 `：`
  - `changed_files` 非数字 → `None`（不 panic、不当 0）
  - `summary` 截断到 500 字符（按 `chars()` 计，不按字节，避免切坏 UTF-8）
  - `blocking` 值为 `none` / `无` / 空白 → `None`
  - 同一键出现多次取**第一次**

### 步骤 7：单测

- [ ] **7.1** `decide_next`（在 `team_task/mod.rs` 的 `#[cfg(test)] mod tests` 里）。
  用两套 config：`auto`（`gates = []`）与 `all_gates`（四个边界全开）。

  - 全自动：pending_confirm+肯定 → developer；developer Pass → reviewer 直接派发；
    reviewer Pass → tester；tester Pass → `Finish { done }`
  - 四闸门：每个边界都返回 `OpenGate`，且 `boundary` 正确
  - reviewer Reject 且 `dev_rounds=1 < 3` → `DispatchRole { developer, round: 2 }`
  - reviewer Reject 且 `dev_rounds=3 == max` → `Finish { failed }`
  - **developer 返回 Unknown → `Finish { failed }`**（不是 OpenGate）
  - **reviewer 返回 Unknown → `Finish { failed }`**
  - **tester 返回 Unknown → `Finish { failed }`**（这条必须有，是边界洞的回归测试）
  - developer 返回 Reject → `Finish { failed }`（开发无 reject 语义）
  - `RunOutcome::Failed` → `Finish { failed }`
  - 陈旧回调：`current_role = "reviewer"` 但 `RunFinished.role = "developer"` → `Ignore`
  - 终态任务收到任何 Trigger → `Ignore`
  - `Cancelled` → `Finish { cancelled }`
  - 单角色配置 `roles = ["developer"]`：developer Pass → `Finish { done }`
  - `roles = []` + pending_confirm 肯定应答 → `Finish { failed }`
  - 否定应答 → `Finish { cancelled }`；无法识别的应答 → `Ignore`

- [ ] **7.2** `gate_answer_is_yes`：肯定集、否定集、带空白、无法识别返回 None。

- [ ] **7.3** `parse_handoff`：
  - 标准四段全中
  - 全角冒号 `verdict：pass`
  - `###` 标题层级
  - 模型加了前言段落
  - `changed_files: 很多` → `None`
  - `blocking: none` / `blocking: 无` → `None`
  - 完全没有交接段 → `Handoff::default()`
  - 超长 summary 按字符截断到 500 且不切坏中文

- [ ] **7.4** `next_role` / `first_role` / `role_def` / `is_terminal` / `is_running`
  的基本用例，含未知角色名、空数组。

## 明确边界

**不许碰的文件/模块**：
- `crates/` 下所有 crate（第 1 步的 `hank-db` 改动已完成，**不要再改它**）
- `admin/`、`client/`、`quant/`、`cli/`
- `server/src/` 下除 `config.rs`、`team_task/mod.rs`、`main.rs` 之外的**任何文件**，
  特别是 `cli_agent.rs`、`interaction_flow.rs`、`interactions.rs`、`feishu/`、`scheduler/`
- `config.toml`（配置段全字段有默认值，不需要改它；改它是第 5 步接入时的事）
- `Cargo.toml`（**不要新增依赖**，`serde` / `toml` / `anyhow` 已够用）
- `CLAUDE.md`、`docs/`

**`main.rs` 只允许加一行 `mod team_task;`**，不要加路由、不要改 `AppState`、
不要调用本任务的任何函数。

**不许做的事**：
- 不要写编排器（`orchestrator.rs`）、角色 prompt（`roles.rs`）、卡片（`card.rs`）、
  REST（`routes.rs`）——都是后续任务
- `decide_next` / `parse_handoff` 里不要有任何 IO、DB、`async`、`AppState`
- 不要为 Unknown 新增 `GateBoundary` 变体（见步骤 5.3 第 7 条）
- 不要改 `agent_interactions` 相关的任何逻辑

**保留工作区原有改动**：
- `crates/hank-db/src/lib.rs` 有第 1 步的 657 行新增，**不要回退、不要修改**
- `docs/feature/team-task-pipeline.md` 与 `docs/tasks/` 是未提交的文档，
  **不要删除、不要修改**
- 除本任务涉及的三个文件外不要 `git checkout` 或回退任何内容

## 验收标准

```bash
# 1. 编译（零错误；warning 数量不应比改动前多）
cargo build --workspace

# 2. clippy
cargo clippy -p hank-server --all-targets

# 3. 单测
cargo test -p hank-server team_task
cargo test -p hank-server config

# 4. 全量回归
cargo test --workspace

# 5. 确认改动范围
git diff --stat
```

期望结果：
- `cargo build --workspace` 成功。`server/src/deployment.rs` 有 5 个既有的
  `never used` warning，与本任务无关，属正常
- `cargo clippy -p hank-server --all-targets` 无新增 warning
- `cargo test -p hank-server team_task` 全绿，覆盖步骤 7 列出的全部场景
- `cargo test --workspace` 全绿，测试总数只增不减
- `git diff --stat` 只列出 `server/src/config.rs`、`server/src/main.rs`
  与新增的 `server/src/team_task/mod.rs`；**`crates/hank-db/src/lib.rs` 不应出现
  在本次新增改动里**（它的 657 行是第 1 步的成果，保持原样）

## 约定

遵循 `CLAUDE.md`：

- **中文注释**。以下三处必须写清「为什么」而不只是「是什么」：
  1. 状态用 `&'static str` 常量而非枚举 —— 值要在 DB / 卡片 / REST 三处流转，
     枚举会在每个边界多一次转换
  2. `Verdict::Unknown` 走 `Finish { failed }` 而非开闸门 —— 三条理由见步骤 5.3 第 7 条
  3. 无法识别的闸门应答返回 `Ignore` 而非猜测 —— 猜错的代价是在错误的 thread 上派发
- **中文 commit message**，形如
  `feat(team-task): 团队任务状态机纯函数与配置段`
- 错误处理：`decide_next` 与 `parse_handoff` **不返回 `Result`**。
  它们的「失败」是 `Decision::Ignore` 与 `Handoff` 的 `None` 字段，不是 `Err`——
  纯判定函数返回 `Result` 会让调用方在正常分支上写 `?`
- 单测用同步 `#[test]`，不需要 `#[tokio::test]`（全是纯函数）
- 测试模块风格参考 `server/src/cli_agent.rs` 与 `server/src/feishu/card.rs` 的
  `#[cfg(test)] mod tests`
