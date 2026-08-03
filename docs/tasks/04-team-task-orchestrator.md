# 任务 04：团队任务流水线 — 编排器

> 本任务是 `docs/feature/team-task-pipeline.md` 实施顺序的**第 4 步**，共 9 步。
> 第 1–3 步（数据层、状态机纯函数+配置、角色 prompt）已完成并合入工作区。
>
> **这是第一个真正做 IO 的任务**：读写 DB、派发 run、抢 `TaskRegistry` 名额。
> 但仍**不接飞书链路**——`advance` 写完之后没有调用方，飞书链路接入是第 5 步。
> 因此本任务做完，现有行为依然**一行不变**。
>
> 动手前必须通读 `docs/feature/team-task-pipeline.md` 的 §6.4、§6.5、§10。

## 背景与目标

### 背景

前三步已备好零件，但没有任何东西把它们串起来：

| 已有 | 位置 |
|------|------|
| 三张表 + 16 个 CRUD | `crates/hank-db/src/lib.rs` |
| `decide_next` 状态机纯函数 | `server/src/team_task/mod.rs` |
| `parse_handoff` 交接解析 | `server/src/team_task/mod.rs` |
| 三个角色 prompt | `server/src/team_task/roles.rs` |
| `[team_task]` 配置段 | `server/src/config.rs` |

编排器是把它们串起来的那一层：拿 `decide_next` 的判定结果去真的插表、
写 session metadata、调 `cli_agent::run_cli_turn`、起 pusher。

### 本任务目标

新建 `server/src/team_task/orchestrator.rs`，实现单一入口 `advance`，
并顺带修掉两处已记录在设计文档 §6.4 的遗留问题。

### 做完之后的可观察效果

1. `cargo build --workspace` 通过，无新增 warning。
2. `cargo test -p hank-server team_task` 全绿，**第 1–3 步的 50 项一个都不能挂**。
3. `advance` 可被调用但尚无调用方（第 5 步才接）。
4. 现有功能行为**完全不变**。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `server/src/team_task/orchestrator.rs` | **新建**。`Trigger` / `RunOutcome` 适配、`advance` 及其分支处理、单测 |
| `server/src/team_task/mod.rs` | 加 `pub mod orchestrator;`；修 `dispatch_from_pending_gate` 的兜底口径（见步骤 6） |
| `server/src/cli_agent.rs` | **仅两处**：`should_gate_turn` 加一个参数并在调用点传值；见步骤 5。**不要改动该文件其他任何内容** |

## 实现步骤

### 步骤 1：模块骨架与类型

- [ ] **1.1** 新建 `server/src/team_task/orchestrator.rs`：

```rust
//! 团队任务编排器：把 decide_next 的判定变成真实的派发 / 开闸门 / 收尾。
//!
//! 单一入口 `advance`。所有状态推进都必须从这里走，不要在别处直接改
//! team_tasks.status——状态机分支多，两个写入点必然漂移。
//!
//! 幂等性由三重防线保证：
//! 1. `decide_next` 对重复触发与终态任务返回 `Decision::Ignore`
//! 2. `team_task_runs` 的 (task_id, role, round) 唯一键，重复派发插入失败
//! 3. `TaskRegistry` 名额，同一 session 同时只有一个 run
```

- [ ] **1.2** 编排器层的 `Trigger`（**注意**：与 `mod.rs` 里 `decide_next` 用的
  借用版 `Trigger<'a>` 不是同一个类型）：

```rust
/// 编排器入口的触发源。持有 owned String，便于跨 await 传递；
/// 内部转换成 mod.rs 的借用版 Trigger<'_> 交给 decide_next。
#[derive(Debug, Clone)]
pub enum Trigger {
    /// 闸门被应答（飞书按钮 / admin 手动应答共用）
    GateAnswered { interaction_id: String, answer: String },
    /// 某角色 run 走到终态
    RunFinished { role: String, round: i32, outcome: super::RunOutcome },
    /// 看板 / 飞书 /stop 取消
    Cancelled { operator: String },
}
```

### 步骤 2：`advance` 主流程

- [ ] **2.1**

```rust
/// 推进任务到下一步。
///
/// 幂等：重复调用同一状态不会重复派发。返回 Err 只表示「推进过程本身出错」
/// （读不到任务、DB 挂了），任务被判为 Ignore 不是错误。
pub async fn advance(state: &Arc<AppState>, task_id: &str, trigger: Trigger) -> Result<()>;
```

固定顺序，不要重排：

1. `state.db.get_team_task(task_id)` → 读不到则 `bail!("团队任务不存在")`
2. 若 `Trigger::RunFinished`，先**收尾 run 行**（见步骤 3），拿到 verdict
3. 构造 `DecideInput`，调 `decide_next(&input, &state.config.team_task)`
4. 按 `Decision` 分派：
   - `Ignore { reason }` → `tracing::info!` 记一行后返回 `Ok(())`。
     **不要**记 `team_task_events`——Ignore 是正常的重复触发，记下来会刷屏
   - `OpenGate { boundary }` → 步骤 4
   - `DispatchRole { role, round }` → 步骤 5
   - `Finish { status, reason }` → 步骤 6

- [ ] **2.2** 每个非 Ignore 分支都往 `team_task_events` 记一行
  （`append_team_event` 是旁路日志语义，用 `let _ =` 或 `warn`，**不要**向上传播）。

### 步骤 3：`RunFinished` 的 run 行收尾

- [ ] **3.1** 在调 `decide_next` **之前**收尾 run 行，因为 `decide_next` 要用 verdict：

```rust
/// 收尾某角色的 run 行，返回归一后的 verdict 供 decide_next 使用。
///
/// verdict 归一规则（重要）：
/// - handoff.verdict == None（模型没写 verdict 行）→ Unknown
/// - handoff.verdict == Some(Unknown)（写了但解析不出来）→ Unknown
/// - 不需要 verdict 的角色（开发）→ 强制 Pass，忽略 handoff 里的 verdict
///
/// 前两种都归到 Unknown，按状态机规则任务会 failed。这是刻意的：
/// 见设计文档 §6.3——猜 pass 会让评审形同虚设，猜 reject 会无谓返工。
async fn finalize_run(
    state: &Arc<AppState>,
    task: &TeamTask,
    role: &str,
    round: i32,
    outcome: &RunOutcome,
    final_text: Option<&str>,
) -> Result<Verdict>;
```

要点：
- 用 `list_team_runs` 找到 `(role, round)` 对应的行取 `run_id`；找不到就 warn 后
  按 `outcome` 直接返回（run 行丢了不该让整个推进崩掉）
- `final_text` 走 `parse_handoff` 得到 `Handoff`
- 开发角色（`needs_verdict == false`）的 `changed_files` 若为 `None`，
  **不要**在这里补 git diff——那要远程 shell 调用，属第 5 步接入时的事，
  这里保持 `None` 落库
- 调 `finish_team_run(run_id, status, verdict, handoff_json, summary, dirty_files, error)`，
  其中 `status` 为 `finished` / `failed`，`handoff_json` 是 `serde_json::to_string(&handoff)`

> **注意 `final_text` 从哪来**：本任务**不实现**它的获取，签名里留成
> `Option<&str>` 由调用方传入。第 5 步接入 `execute_remote_turn` 时才有真实文本。
> 单测里直接构造字符串传进去。

### 步骤 4：`OpenGate`

- [ ] **4.1**

```rust
/// 开人工闸门：落 team_gate 交互单 + emit AskUser 让 pusher 出卡片，
/// 任务状态改 pending_*_gate。
async fn open_gate(
    state: &Arc<AppState>,
    task: &TeamTask,
    boundary: GateBoundary,
) -> Result<()>;
```

要点：
- 交互单用 `NewInteraction`，`kind: "team_gate"`，
  `resume_ref` 写 `{"team_task_id": ..., "boundary": "review_start", "round": n}`
  （第 5 步的 `answer_and_resume` 要靠它找回任务）
- `options` 按边界给按钮文案：
  - `ReviewStart` → `["继续评审", "终止"]`
  - `DevRestart` → `["重新开发", "终止"]`
  - `TestStart` → `["继续测试", "终止"]`
  - `DevStart` 不走这里（它是现有 `task_gate`，第 5 步处理）
- `expires_at: None`（飞书渠道不过期，与现有 `task_gate` 一致）
- `account_id` / `chat_id` / `topic_id` 从 `task` 上取
- **`current_role` 保持不变**，只改 `status`：
  `update_team_task_status(task_id, boundary.pending_status(), task.current_role.as_deref(), None, None)`

  > ⚠️ 这条是设计文档 §6.4 明确记录的约定，**不要**顺手清空 `current_role`。
  > `decide_next` 在 `pending_*_gate` 分支要靠它算下一个角色
  > （`next_role(cfg.roles, current)`）；清掉之后
  > `dispatch_from_pending_gate` 的兜底会静默派发错角色。
- emit `AgentEvent::AskUser`，`kind: Some("team_gate".to_string())`，
  `tool_use_id: format!("team_gate:{}", interaction_id)`
  （对齐现有 `finish_as_task_gate` 的写法）

### 步骤 5：`DispatchRole`

这是最复杂的分支，也是唯一需要碰 `cli_agent.rs` 的原因。

- [ ] **5.1 先修一个会导致无限循环的问题。**

  `cli_agent::run_remote_cli_turn` 里有：

  ```rust
  let gate_mode = should_gate_turn(
      state.config.server_agent.task_gate_enabled,
      agent_kind,
      source,
      existing_thread,   // ← 为空时返回 true
  );
  ```

  编排器派发角色时走的也是 `run_cli_turn`。新角色的 thread 是空的
  （每个角色独占 thread），于是 `should_gate_turn` 会**再次返回 true**，
  又跑一轮只读分析、又落一张 `task_gate` 闸门单——流水线永远走不到真正的执行。

  修法：给 `should_gate_turn` 加第五个参数：

  ```rust
  fn should_gate_turn(
      task_gate_enabled: bool,
      agent_kind: &str,
      source: Option<&str>,
      existing_thread_id: Option<&str>,
      /// 会话已挂在团队任务流水线上（metadata.team_task_id 非空）。
      /// 此时不再弹闸门——闸门是流水线的入口，入口已经过了，
      /// 后续每个角色的 thread 都是空的，再判会无限循环。
      in_team_pipeline: bool,
  ) -> bool {
      if in_team_pipeline {
          return false;
      }
      // ……其余逻辑一行不改
  }
  ```

  调用点（`server/src/cli_agent.rs:487` 附近）传
  `metadata["team_task_id"].as_str().is_some_and(|s| !s.is_empty())`。

  **`cli_agent.rs` 只允许这两处改动**：函数签名加参数 + 调用点传值。
  另外该文件现有 14 处 `should_gate_turn(...)` 单测调用要补第五个参数
  （全部传 `false`，保持原语义），并**新增一项**单测：
  `in_team_pipeline = true` 时恒返回 `false`。

- [ ] **5.2** 派发本体：

```rust
/// 派发某角色的某一轮。
///
/// 顺序不能重排：抢名额 → 插 run 行（唯一键防重）→ 写 thread → 派发。
/// 先抢名额是因为 active_tasks 要等 run_cli_turn 走完准备工作才登记，
/// 中间有秒级空窗（见 task_state.rs 模块注释）。
async fn dispatch_role(
    state: &Arc<AppState>,
    task: &TeamTask,
    role: &str,
    round: i32,
) -> Result<()>;
```

顺序：

1. **抢名额**：`state.tasks.try_acquire(&task.session_id)`。
   拿不到 → 记 event「已有在途派发，跳过」后返回 `Ok(())`（不是错误）。
   拿到后再查 `state.active_tasks.read().await.contains_key(&session_id)`，
   若有则 `guard.release().await` 后同样跳过（对齐 `answer_and_resume` 的双检）
2. **插 run 行**：`insert_team_run(task_id, role, round)`。
   返回 `Ok(None)` 表示唯一键冲突（已派发过）→ 释放名额后返回 `Ok(())`
3. **写 thread**：把该角色本轮的 thread 写进 `sessions.metadata.agent_thread_id`。
   - 新角色新轮次 → 写 `null`（让 CLI 开新 thread）
   - **同时写 `metadata.team_task_id = task.id`**（步骤 5.1 的开关靠它）
   - 用一个本模块私有的 helper 做，不要去调 `cli_agent` 的私有函数
     （`parse_metadata` / `persist_thread_id` 都是私有的）
4. **构造 prompt**：用 `roles::RolePromptInput` + `super::role_prompt(role, &input)`。
   `upstream` 从 `list_team_runs` 里找上一个已完成的角色轮次填充
   （评审看开发的、测试看评审的、打回后的开发看评审的）
5. **改任务状态**：`update_team_task_status(task_id, role_def.running_status, Some(role), None, None)`
6. **派发**：`cli_agent::run_cli_turn(state, &session_id, session, content, &backend)`
7. **失败回滚**：派发返回 `Err` 时——
   - `finish_team_run(run_id, "failed", ...)` 写错误
   - `update_team_task_status(task_id, "failed", None, None, Some(&err))`
   - `state.tasks.clear_progress(&session_id).await`
   - 释放名额
   - 记 event

   **不做自动重试**（设计文档 §6.4）：在一个可能已改了半个仓库的工作区上重跑，
   比让人看一眼再决定更危险。看板提供「从当前角色重试」。
8. 成功时 `dispatch_guard.release().await`

> **本任务不起 pusher**。`pusher::spawn` 需要飞书 `api` / `message_id`，
> 那是第 5、6 步的事。这里只 `run_cli_turn`，拿到的 `ChatTurnHandle` 先丢掉，
> 并在注释里写明「pusher 由第 5 步接入时补」。

### 步骤 6：`Finish` 与遗留修复

- [ ] **6.1**

```rust
/// 走终态：写状态与 finished_at、清 current_role、清进度快照。
async fn finish_task(
    state: &Arc<AppState>,
    task: &TeamTask,
    status: &str,
    reason: Option<&str>,
) -> Result<()>;
```

要点：
- `update_team_task_status(task_id, status, None, None, reason)`
  ——终态**才**清 `current_role`（传 `None`）
- `state.tasks.clear_progress(&task.session_id).await`
- 从 `sessions.metadata` 里**清掉 `team_task_id`**，
  否则该 session 后续的普通任务会被步骤 5.1 的开关误判成「在流水线里」而不弹闸门
- 记 event

- [ ] **6.2 修 `dispatch_from_pending_gate` 的兜底口径不一致**
  （设计文档 §6.4 已记录）。`server/src/team_task/mod.rs` 里
  `STATUS_PENDING_TEST_GATE` 分支目前是：

  ```rust
  None => Decision::DispatchRole { role: "tester".to_string(), round: 1 },
  ```

  改成与 `pending_review_gate` 分支一致：

  ```rust
  None => Decision::Finish {
      status: STATUS_FAILED,
      reason: Some("没有下一个角色可进入测试".to_string()),
  },
  ```

  理由：硬编码会派发一个可能不在 `cfg.roles` 里的角色。补一项单测锁定。

- [ ] **6.3** 在 `mod.rs` 加 `pub mod orchestrator;`。

### 步骤 7：单测

编排器有 IO，不能像前三步那样全纯函数测。**不要为此引入 DB 测试框架或 mock 层**
（项目没有这个基建，硬造一套是本任务范围外的大工程）。改为测可提纯的部分：

- [ ] **7.1** `verdict` 归一逻辑抽成纯函数并单测：

```rust
/// 把 parse_handoff 的结果归一成 decide_next 要的 Verdict。
/// 抽成纯函数是为了能单测——这段规则错了会让评审形同虚设。
fn normalize_verdict(handoff_verdict: Option<Verdict>, needs_verdict: bool) -> Verdict;
```

用例：`(None, true) → Unknown`、`(Some(Unknown), true) → Unknown`、
`(Some(Pass), true) → Pass`、`(Some(Reject), true) → Reject`、
`(None, false) → Pass`、`(Some(Reject), false) → Pass`（开发角色强制 Pass）

- [ ] **7.2** 闸门按钮文案映射抽成纯函数并单测：
  `gate_options(GateBoundary) -> Vec<String>`，四个边界各一条。

- [ ] **7.3** `upstream` 选取逻辑抽成纯函数并单测：

```rust
/// 从已完成的 run 列表里挑出给本角色做输入的那一轮（最近一个已完成的上游角色）。
fn pick_upstream_run<'a>(runs: &'a [TeamTaskRun], for_role: &str, cfg_roles: &[String])
    -> Option<&'a TeamTaskRun>;
```

用例：评审取开发最近一轮、测试取评审最近一轮、开发首轮无上游、
开发第 2 轮取评审那轮（打回场景）、run 列表为空返回 None。

- [ ] **7.4** `mod.rs` 补步骤 6.2 的回归测试：
  `pending_test_gate` + 无下一个角色 → `Finish { failed }`。

- [ ] **7.5** `cli_agent.rs` 补步骤 5.1 的单测：
  `should_gate_turn(true, "trace_code", Some("feishu"), None, true) == false`。

## 明确边界

**不许碰的文件/模块**：
- `crates/` 下所有 crate（第 1 步的 `hank-db` 已完成，**不要再改**）
- `admin/`、`client/`、`quant/`、`cli/`
- `server/src/interaction_flow.rs`、`server/src/interactions.rs`、
  `server/src/feishu/`、`server/src/scheduler/`、`server/src/chat.rs`、
  `server/src/config.rs`、`server/src/main.rs`（除加 `mod` 声明外无需改动，
  而 `mod team_task;` 第 2 步已加好）
- `config.toml`、`Cargo.toml`（**不要新增依赖**）
- `CLAUDE.md`、`docs/`

**`cli_agent.rs` 只允许三处改动**：`should_gate_turn` 加第五个参数、
调用点传值、现有 14 处单测调用补参数 + 新增 1 项单测。
**不要**改 `finish_as_task_gate`、`resume` 相关逻辑、`execute_remote_turn`
或该文件任何其他函数——那些是第 5 步的事。

**`mod.rs` 只允许两处改动**：加 `pub mod orchestrator;`、
修 `dispatch_from_pending_gate` 的 `pending_test_gate` 兜底。
**不要**改 `decide_next` 主体、`parse_handoff`、状态常量、`Verdict`、
`RoleDef`、`ROLE_DEFS`、`roles.rs`，也不要删改第 1–3 步的任何单测。

**不许做的事**：
- 不要接飞书链路（`answer_and_resume` 分三路、`execute_remote_turn` 回调
  都是第 5 步）
- 不要起 pusher、不要发卡片、不要写 REST、不要动看板
- 不要为编排器引入 DB mock / 测试容器 / `sqlx::test`
- 不要做自动重试

**保留工作区原有改动**：
- `crates/hank-db/src/lib.rs`（657 行）、`server/src/config.rs`（112 行）、
  `server/src/main.rs`（1 行）、`server/src/team_task/mod.rs`（1485 行）、
  `server/src/team_task/roles.rs`（552 行）
- `docs/feature/team-task-pipeline.md` 与 `docs/tasks/`
- 除本任务涉及的三个文件外不要 `git checkout` 或回退任何内容

## 验收标准

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test -p hank-server should_gate_turn
cargo test --workspace
git status --short
git diff --stat
```

期望结果：
- 编译成功。`server/src/deployment.rs` 那 5 个既有 `never used` warning 属正常
- clippy 基线 **46** 个 warning，无新增
  （`advance` 与 `dispatch_role` 参数较多，必要时加 `#[allow(clippy::too_many_arguments)]`
  并在注释说明，与文件内既有做法一致）
- `cargo test -p hank-server team_task` **≥ 50 + 本任务新增**，全绿。
  第 1–3 步的 50 项一个都不能挂
- `cargo test -p hank-server should_gate_turn` 全绿（含新增的 `in_team_pipeline` 一项）
- `cargo test --workspace` 全绿，server 那组 ≥ 177 + 新增
- `git diff --stat` 只列出 `server/src/cli_agent.rs` 与
  `server/src/team_task/mod.rs`；`orchestrator.rs` 是新文件。
  `crates/hank-db/src/lib.rs`、`server/src/config.rs`、`server/src/main.rs`
  的行数与本任务开始前**完全一致**

## 约定

遵循 `CLAUDE.md`：

- **中文注释**。以下五处必须写清「为什么」：
  1. `advance` 是唯一入口 —— 两个写入点必然漂移
  2. 抢名额在插 run 行之前 —— `active_tasks` 有秒级空窗
  3. `in_team_pipeline` 短路闸门 —— 否则每个角色的空 thread 都会再弹一次闸门，无限循环
  4. 开闸门不清 `current_role` —— `decide_next` 要靠它算下一个角色
  5. 不做自动重试 —— 工作区可能已被改了一半
- **中文 commit message**，形如
  `feat(team-task): 编排器，把状态机判定变成真实派发`
- `advance` 返回 `anyhow::Result<()>`；`Decision::Ignore` **不是** `Err`
- `append_team_event` 失败只 warn，不向上传播（旁路日志语义）
- 单测用同步 `#[test]`（本任务要测的都是提纯出来的纯函数）
