# 任务 05：团队任务流水线 — 接入飞书链路

> 本任务是 `docs/feature/team-task-pipeline.md` 实施顺序的**第 5 步**，共 9 步。
> 第 1–4 步（数据层、状态机、角色 prompt、编排器）已完成。
>
> ⚠️ **这是唯一一个会改变现有行为的任务**。前四步都是纯新增、无调用方；
> 本任务把编排器接到飞书链路上，让流水线真正跑起来。
>
> 保护措施：所有新行为都由 `[team_task].enabled` 守着，**默认 `false`**。
> 开关关闭时必须与今天的行为**完全一致**（这是本任务最重要的验收项）。
>
> 动手前必须通读 `docs/feature/team-task-pipeline.md` 的 §6.5、§6.6、§6.7、§9、§10。

## 背景与目标

### 背景

现在编排器 `advance` 已经能派发角色、开闸门、走终态，但**没有任何调用方**：

- 分析轮结束后落的是 `task_gate` 交互单，没有建 `team_tasks` 行
- 用户点「开始修」走的是旧的 `resume_task_gate`（直接 resume 第二轮），不进编排器
- 角色 run 结束后没有人通知编排器推进下一步

本任务补上这三条连线。

### 本任务目标

1. **分析轮建任务行**：`finish_as_task_gate` 额外创建 `team_tasks` 行。
2. **闸门应答分三路**：`answer_and_resume` 按 `kind` + 开关分派到编排器或旧路径。
3. **run 终态回调编排器**：`execute_remote_turn` 收尾时通知 `advance`。
4. **配置校验**：`enabled = true` 但 `task_gate_enabled = false` 时启动失败。
5. **启动收尾僵尸任务**：调用第 1 步就写好但一直没接的 `fail_stale_team_tasks`。
6. **派发角色时起 pusher**：让飞书卡片能看到角色 run 的进度。

### 做完之后的可观察效果

**开关关闭时（默认）**：行为与今天完全一致，回归测试全绿。

**开关打开时**：飞书派一个代码任务 → 只读分析 → 闸门卡片 →
点「开始修」→ 开发角色 run → 评审角色 run → 测试角色 run → 任务 `done`，
`team_tasks` / `team_task_runs` / `team_task_events` 三张表有完整记录。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `server/src/cli_agent.rs` | `finish_as_task_gate` 建 `team_tasks` 行；`execute_remote_turn` 收尾回调编排器；`finish_as_task_gate` 与 gate 判定处的少量配合 |
| `server/src/interaction_flow.rs` | `answer_and_resume` 按 kind 分三路；`team_gate` 的卡片终态文案 |
| `server/src/team_task/orchestrator.rs` | `dispatch_role` 起 pusher；`advance` 暴露给外部调用所需的小调整 |
| `server/src/team_task/mod.rs` | 去掉 `#![allow(dead_code)]`（接入后不再需要） |
| `server/src/config.rs` | 新增 `TeamTaskConfig::validate`，在 `Config::load` 里调用 |
| `server/src/main.rs` | 启动时调 `fail_stale_team_tasks` |
| `server/src/interactions.rs` | `KINDS` 白名单加 `team_gate` |
| `config.toml` | 新增 `[team_task]` 段（`enabled = false`），带注释说明 |

## 实现步骤

### 步骤 1：配置校验

- [ ] **1.1** 在 `server/src/config.rs` 给 `TeamTaskConfig` 加：

```rust
impl TeamTaskConfig {
    /// 启动时校验。团队任务依赖闸门产出分析（分析轮是流水线的入口），
    /// 所以 enabled=true 且 task_gate_enabled=false 是无效组合——
    /// 静默降级会让用户以为流水线在跑而实际没跑。
    pub fn validate(&self, task_gate_enabled: bool) -> Result<()>;
}
```

校验项：
- `enabled && !task_gate_enabled` → `bail!`，错误信息说清要把
  `[server_agent].task_gate_enabled` 设为 true
- `enabled && roles.is_empty()` → `bail!`
- `roles` 里出现未知角色名（不在 `ROLE_DEFS` 里）→ `bail!` 列出合法值。
  **不要静默丢弃**——配置写错了却照跑，用户会以为角色生效了
- `gates` 里出现未知边界名 → `bail!`（合法值 `dev_start` / `review_start`
  / `dev_restart` / `test_start`）
- `enabled && max_dev_rounds < 1` → `bail!`

> 注意 `config.rs` 不能直接依赖 `team_task::ROLE_DEFS`（会形成
> `config → team_task → config` 的循环引用）。把合法值列表在
> `config.rs` 里写成本地常量，并加注释指明与 `team_task::ROLE_DEFS` 保持同步。

- [ ] **1.2** 在 `Config::load` 返回前调用：
  `cfg.team_task.validate(cfg.server_agent.task_gate_enabled)?`

- [ ] **1.3** 补单测：合法组合通过；`enabled=true + task_gate_enabled=false` 报错；
  未知角色名报错；未知 gate 名报错；`enabled=false` 时**即使其他字段非法也通过**
  （关闭状态不该拦启动）。

### 步骤 2：`config.toml` 加配置段

- [ ] **2.1** 在 `config.toml` 末尾追加，**`enabled` 必须是 `false`**：

```toml
# 团队任务流水线：开发 → 评审 → 测试多角色编排。
# 依赖 [server_agent].task_gate_enabled = true（分析轮是流水线入口）。
[team_task]
enabled = false
# 参与流水线的角色，按顺序流转。可裁剪成 ["developer"] 只跑单角色。
roles = ["developer", "reviewer", "tester"]
# 需要人工确认的边界：dev_start / review_start / dev_restart / test_start
# 默认只保留开发前闸门，其余自动流转
gates = ["dev_start"]
# 评审打回后最多重新开发几轮，超出即 failed
max_dev_rounds = 3
# 看板地址，用于飞书卡片深链；留空则不渲染看板链接行
# dashboard_base_url = "http://127.0.0.1:18789"
```

**不要改动 `config.toml` 的任何既有内容**，只追加这一段。

### 步骤 3：分析轮建 `team_tasks` 行

- [ ] **3.1** 改 `server/src/cli_agent.rs` 的 `finish_as_task_gate`（约 2345 行）。
  在创建 `task_gate` 交互单**之后**，若 `state.config.team_task.enabled`：

  1. `state.db.create_team_task(NewTeamTask { ... })`，字段来源：
     - `session_id` / `user_id`：现有参数
     - `source: "feishu"`
     - `issue_key`：从 `user_text` 用 `#([A-Z0-9]{4,12})` 解析首个匹配，
       无匹配则 `None`。**抽成纯函数 `parse_issue_key` 并单测**
     - `title`：`user_text` 前 50 字符（与 `update_session_title` 口径一致）
     - `goal`：现有的 `goal` 变量（已截断 2000 字符）
     - `analysis`：现有的 `analysis` 参数
     - `backend` / `exec_client_id` / `agent_kind`：现有参数
     - `account_id` / `chat_id` / `topic_id`：现有的 feishu_chat 反查结果
  2. 把 `team_task_id` 写进 `sessions.metadata`
  3. 把 `team_task_id` 加进交互单的 `resume_ref` JSON
     （`answer_and_resume` 要靠它找回任务）

- [ ] **3.2** 建表失败时**不要**让整轮失败：`warn` 后继续走旧的 `task_gate` 路径。
  理由：分析已经跑完了，因为建一行记录失败就把用户的分析结果丢掉不合理。
  注释写明这个降级是刻意的。

- [ ] **3.3** `resume_ref` 里已有的 `backend` / `thread_id` / `exec_client_id`
  / `agent_kind` / `dirty_files` 字段**全部保留**——旧路径还要用。

### 步骤 4：闸门应答分三路

- [ ] **4.1** 改 `server/src/interaction_flow.rs` 的 `answer_and_resume`。
  当前在 ③ 原子应答之后按 `kind` 分两路（`task_gate` / 其他）。改成三路：

```rust
// ⑤ 派发。task_gate 在开关打开且有 team_task_id 时交给编排器，
// 否则走原来的单角色 resume；team_gate 一律交给编排器。
let team_task_id = answered_row
    .resume_ref
    .as_deref()
    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    .and_then(|v| v["team_task_id"].as_str().map(str::to_string))
    .filter(|s| !s.is_empty());

match answered_row.kind.as_str() {
    "task_gate" | "team_gate"
        if state.config.team_task.enabled && team_task_id.is_some() => { /* 编排器 */ }
    "task_gate" => { /* 旧路径 resume_task_gate，一行不改 */ }
    "team_gate" => { /* 有 team_gate 但拿不到 task_id：异常，标 failed 并提示 */ }
    _ => { /* quant_confirm / ask_user 原路径 */ }
}
```

要点：
- 编排器分支里调
  `team_task::orchestrator::advance(state, &task_id, Trigger::GateAnswered { interaction_id, answer })`
- **`dispatch_guard` 的归属**：编排器的 `dispatch_role` 内部会自己抢名额。
  所以进编排器**之前必须先释放** `answer_and_resume` 持有的 guard，
  否则 `try_acquire` 拿不到名额，任务会被静默跳过。
  这一点务必在注释里写清，它是最容易出错的地方
- 编排器返回 `Err` 时：把交互单强制回 `pending`
  （复用现有 `force_interaction_pending`）、恢复卡片，与旧路径的失败处理对齐

- [ ] **4.2** ④ 改终态卡那一段：`kind == "team_gate"` 时标题用「团队任务闸门」，
  question 取 `goal`。现有 `task_gate` 的分支一行不改。

- [ ] **4.3** `server/src/interactions.rs` 的 `KINDS` 白名单加 `"team_gate"`，
  否则 admin 交互单页按 kind 筛选时会拒绝这个值。

### 步骤 5：run 终态回调编排器

- [ ] **5.1** 改 `server/src/cli_agent.rs` 的 `execute_remote_turn` 收尾段。
  现有代码在多处调 `finalize_open_task_gate(state, session_id, status, ...)`
  （约 554 / 779 / 832 / 852 / 866 / 968 / 1024 行）。

  **不要在这 7 处各加一次回调**。改为在 `execute_remote_turn` 的**最外层出口**
  统一加一次：函数结束前，若 `metadata.team_task_id` 非空，则调用编排器。

  实现方式：把现有函数体包一层，或在返回前插入一段。要点是**只有一个回调点**，
  否则一次 run 会推进多次状态机。

- [ ] **5.2** 回调需要三样东西：
  - `role`：从 `team_tasks.current_role` 读（编排器派发时已写）
  - `round`：从 `latest_team_run` 读
  - `outcome`：`RunOutcome::Finished(_)` 或 `RunOutcome::Failed`。
    verdict 由编排器的 `finalize_run` 从 `final_text` 解析，这里只给成功/失败
  - `final_text`：成功时的最终文本（用于 `parse_handoff`）

  签名形如：

```rust
/// run 走到终态后通知团队任务编排器推进下一步。
///
/// 只在 execute_remote_turn 的最外层出口调用一次——在每个 finalize_open_task_gate
/// 旁边各加一次会让一次 run 推进多次状态机。
async fn notify_team_task_if_any(
    state: &Arc<AppState>,
    session_id: &str,
    succeeded: bool,
    final_text: Option<&str>,
);
```

- [ ] **5.3** **闸门轮不回调**。`gate_mode == true` 且成功落了闸门单时，
  任务状态是 `pending_confirm`，等用户点按钮，不该由 run 终态推进。
  `decide_next` 对 `pending_confirm` + `RunFinished` 会返回 `Ignore`，
  所以即使误调也不会出错——但仍应显式跳过，省一次 DB 往返。

- [ ] **5.4** 回调失败只 `warn`，**不要**影响 run 本身的终态。
  run 已经跑完了，编排失败不该让用户看到「任务失败」。

### 步骤 6：派发角色时起 pusher

- [ ] **6.1** 改 `server/src/team_task/orchestrator.rs` 的 `dispatch_role`。
  第 4 步留了 TODO：拿到 `ChatTurnHandle` 后直接丢掉，没起 pusher，
  所以飞书那边看不到角色 run 的进度。

  现在补上。pusher 需要飞书 `api` / `message_id` / `chat_id` / `topic_id`：
  - `chat_id` / `topic_id` 从 `team_tasks` 行读
  - `account_id` 从 `team_tasks` 行读，据此 `state.db.get_feishu_account` +
    `FeishuApi::new_archived`
  - `message_id`：用 `team_tasks.card_message_id`。为空时说明还没有主卡，
    此时**跳过 pusher**（`warn` 一行），不要因为没有卡片就让派发失败

- [ ] **6.2** 账号被停用 / 查不到时同样跳过 pusher 并 `warn`，不影响派发。
  注释说明：进度卡是附加功能，缺它不该阻断任务执行。

> 团队任务**主卡**（`build_team_stage_card`）是第 6 步的事。本任务只把
> 现有的 `pusher::spawn` 接上，让角色 run 的进度有地方去。

### 步骤 7：启动收尾僵尸任务

- [ ] **7.1** 第 1 步写好的 `fail_stale_team_tasks` 一直没有调用方。
  在 `server/src/main.rs` 启动流程里调用，位置紧邻现有的
  `scheduler::start(state.clone())`（约 236 行）。

  仿 `scheduler::start` 的写法（`server/src/scheduler/mod.rs:92`）：

```rust
// 进程重启后 CLI thread 已不可信，一律标失败而不是尝试续跑
// （与 scheduler 收尾遗留 running job_run 同一模式）。
if state.config.team_task.enabled {
    let s = state.clone();
    tokio::spawn(async move {
        match s.db.fail_stale_team_tasks().await {
            Ok(0) => {}
            Ok(n) => tracing::warn!("team_task: 收尾 {n} 条进程重启遗留的运行中任务"),
            Err(e) => tracing::warn!("team_task: 收尾僵尸任务失败: {e:#}"),
        }
    });
}
```

### 步骤 8：清理

- [ ] **8.1** 去掉 `server/src/team_task/mod.rs` 顶部的 `#![allow(dead_code)]`。
  接入后大部分函数都有调用方了。如果去掉后出现 `never used` warning，
  说明确实有函数没接上——**逐个检查是漏接还是真的多余**，不要直接加回 allow。
  确实暂时用不到的（如第 7–8 步才用的 REST 相关查询）可以单独标
  `#[allow(dead_code)]` 并注释说明留给哪一步。

## 单测

- [ ] **9.1** `parse_issue_key`：`"「IK5MOR」优化"` → `Some("IK5MOR")`；
  `"修个 bug"` → `None`；多个匹配取第一个；小写不匹配；
  长度边界（3 位不匹配、13 位不匹配）。

- [ ] **9.2** `TeamTaskConfig::validate`：步骤 1.3 列出的五种情况。

- [ ] **9.3** **开关关闭的回归测试**（本任务最重要的一项）：
  `should_gate_turn` 与现有 `task_gate` 相关的所有既有单测必须原样通过。
  不需要新增，但要确认 `cargo test -p hank-server` 全绿。

- [ ] **9.4** 第 1–4 步的 62 项 `team_task` 单测一项都不能挂。

## 手工验证清单

编排器的分支编排没有自动化测试（项目无 DB mock 基建，这是第 4 步就定下的取舍）。
本任务接通后必须手工走一遍。**每一项都要实际执行并记录结果**：

**A. 开关关闭（回归，最重要）**
- [ ] `[team_task].enabled = false` + `task_gate_enabled = true`，
  飞书派一个代码任务 → 分析 → 闸门卡片 → 点「开始修」→ 正常执行第二轮。
  确认 `team_tasks` 表**没有新行**，行为与今天一致
- [ ] `task_gate_enabled = false` 时派任务 → 直接执行，不弹闸门

**B. 配置校验**
- [ ] `enabled = true` + `task_gate_enabled = false` → server **启动失败**，
  错误信息清楚
- [ ] `roles = ["developer", "designer"]` → 启动失败，提示合法值

**C. 全自动流水线（`gates = []`）**
- [ ] 派任务 → 分析 → 闸门 → 点「开始修」→ 开发 run → 自动进评审 run →
  自动进测试 run → 任务 `done`
- [ ] 三张表记录完整：`team_tasks.status = done`、
  `team_task_runs` 三行且 verdict 正确、`team_task_events` 有完整流转

**D. 四闸门（`gates` 四个全开）**
- [ ] 每个边界都弹卡片，四次点击后走完
- [ ] 在任一闸门点「终止」→ 任务 `cancelled`

**E. 打回**
- [ ] 评审 reject → 回开发第 2 轮 → 再 pass → 进测试
- [ ] 连续 reject 到 `max_dev_rounds` → 任务 `failed`，卡片说明「已达最大返工轮次」

**F. 异常**
- [ ] 开发轮中途关掉 hank-cli → 任务 `failed`，卡片说明节点离线
- [ ] server 重启后遗留的 `running_*` 任务被标 `failed`
- [ ] 评审输出不带交接段 → 任务 `failed`，理由「结论无法解析」

## 明确边界

**不许碰的文件/模块**：
- `crates/` 下所有 crate
- `admin/`、`client/`、`quant/`、`cli/`
- `server/src/feishu/`（主卡是第 6 步；本任务只复用现有 `pusher::spawn` 与
  `FeishuApi`，**不改 `feishu/` 下任何文件**）
- `server/src/scheduler/`、`server/src/chat.rs`、`server/src/admin.rs`
- `Cargo.toml`（**不要新增依赖**）
- `CLAUDE.md`、`docs/`

**不许做的事**：
- **不要改动 `enabled = false` 时的任何行为**。这是本任务的硬约束：
  所有新逻辑都必须在 `if state.config.team_task.enabled` 之内
- 不要删除或改写 `resume_task_gate`——旧路径要完整保留，
  它是开关关闭时的执行路径，也是回滚手段
- 不要在 7 处 `finalize_open_task_gate` 旁边各加一次编排器回调（步骤 5.1）
- 不要写团队任务主卡 `build_team_stage_card`（第 6 步）
- 不要写 REST / 看板（第 7–8 步）
- 不要为编排器引入 DB mock

**保留工作区原有改动**：第 1–4 步的成果全部保留。
除本任务涉及的 8 个文件外不要 `git checkout` 或回退任何内容。

## 验收标准

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test -p hank-server
cargo test --workspace
git diff --stat
```

期望结果：
- 编译成功。`server/src/deployment.rs` 那 5 个既有 `never used` warning 属正常。
  若去掉 `#![allow(dead_code)]` 后 `team_task` 出现新 warning，
  按步骤 8.1 逐个处理，不要直接加回 allow
- clippy 基线 **46** 个 warning，无新增
- `cargo test -p hank-server team_task` **≥ 62 + 本任务新增**，全绿
- `cargo test --workspace` 全绿，server 组 ≥ 190 + 新增
- 手工验证清单 A 组（开关关闭回归）**必须全部通过并在提交说明里记录**

## 约定

遵循 `CLAUDE.md`：

- **中文注释**。以下五处必须写清「为什么」：
  1. 建 `team_tasks` 行失败时降级走旧路径 —— 分析已跑完，不该因一行记录丢掉
  2. 进编排器前先释放 `dispatch_guard` —— 否则编排器 `try_acquire` 拿不到名额
  3. run 终态回调只在最外层出口一次 —— 多点回调会推进多次状态机
  4. 回调失败只 warn —— run 已跑完，编排失败不该让用户看到「任务失败」
  5. 配置校验对无效组合 `bail` 而非降级 —— 静默降级会让用户以为流水线在跑
- **中文 commit message**，形如
  `feat(team-task): 接入飞书链路，流水线可端到端运行`。
  提交说明里记录手工验证 A 组的结果
- 新逻辑一律包在 `if state.config.team_task.enabled` 内
- `advance` 的错误只 warn 或转成用户可见提示，不 panic
