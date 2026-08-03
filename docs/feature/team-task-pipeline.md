# 团队任务流水线（多角色编排 + team 看板）设计

## 1. 背景

现在飞书渠道的代码任务是**单角色两阶段**：第一轮只读分析 → 落 `task_gate` 交互单 →
用户点「开始修」→ 在同一 CLI thread 上 resume 第二轮执行。实现见
`server/src/cli_agent.rs`（`should_gate_turn` / `finish_as_task_gate`）、
`server/src/interaction_flow.rs`（`resume_task_gate`）、
`server/src/feishu/card.rs`（`build_task_gate_card`）。

要做的是把「开始修」之后的那一轮，从**一次执行**变成**开发 → 评审 → 测试的多角色流水线**，
每个角色一次独立的 hank-cli run（各自的 CLI thread），并给流水线配一个独立的
team 看板（截图里的 `http://127.0.0.1:18789/#team/{task_no}`）。

**本设计不接 Gitee / GitHub / Jira 巡检**。任务只来自飞书派单与看板手动创建，
`issue_key` 作为可空的展示字段保留（用户在飞书文本里写 `#IK5MOR` 时解析填入），
巡检入队留到后续，届时只需新增一个 scheduler job 往 `team_tasks` 插行。

### 与现有能力的关系

| 现有能力 | 在本设计中的角色 |
|---------|-----------------|
| `agent_interactions` + `interaction_flow::answer_and_resume` | 所有人工闸门（含角色边界闸门）继续走这条链路，不另造一套 |
| `task_gate` 交互单 | 流水线的**第一道**闸门，卡片加字段，应答后进编排器而不是直接 resume |
| `cli_agent::run_cli_turn` | 每个角色的执行单元，签名不变 |
| `task_state::TaskRegistry` | 继续保证「同一 session 同时只跑一个 run」 |
| `feishu/pusher.rs` | 继续负责单个 run 的进度卡片；跨角色的任务主卡由编排器维护 |
| `scheduler`（JOB_DEFS 模式） | 角色注册表沿用「定义在代码、状态在 DB」的同一约定 |

## 2. 目标形态

三张卡片与实现的对应：

| 卡片 | 触发时机 | 实现 |
|------|---------|------|
| 新任务 · 待确认是否开始修 | 分析轮结束 | 现有 `build_task_gate_card`，扩展任务编号/Issue/来源字段 |
| 团队任务 · 开发 · 开发 开始 | 每个角色启动 | 新增 `build_team_stage_card`，2s 节流原地刷新 |
| 团队任务 · 评审 → 开发（打回） | 评审判定不通过 | 同一张阶段卡改写 + 一条打回说明 |

## 3. 概念模型

```
team_tasks（任务）           1 ──── N   team_task_runs（角色轮次）
   task_no  tsk_xxx_xxxx                role      developer/reviewer/tester
   session_id（复用飞书话题会话）        round     同角色第几轮（打回后 +1）
   status   running_developer            thread_id  该角色独占的 CLI thread
   issue_key IK5MOR（可空）              verdict    pass/reject/failed
```

- **一个任务一个 session**：仍是飞书「话题 = 会话」，`feishu_chats` 映射不动。
  任务编排层只是在这个 session 上串行发起多次 run。
- **每个角色独占 CLI thread**：thread_id 存在 `team_task_runs.thread_id`，
  派发某角色前由编排器写入 `sessions.metadata.agent_thread_id`
  （新角色写 null，续轮写该角色上一轮的 thread）。这样 `run_cli_turn`
  与 `execute_remote_turn` 的读取逻辑一行不用改，thread 归属完全由编排层负责。
- **角色间用产物交接**，不共享上下文：开发轮产出 diff 摘要与自述，
  评审轮的 prompt 注入这些产物而不是 resume 开发的 thread。
  这是刻意的——评审要独立视角，共享 thread 会让评审顺着开发的思路走。

## 4. 状态机

`team_tasks.status` 取值（截图的 `running_developer` 即此列）：

```
                    ┌─────────────────────────────────────────┐
                    ▼                                         │
pending_confirm ──► running_developer ──► pending_review_gate ─┤
   （闸门单）              │                     │             │
       │ 跳过              │ 失败                │ 人工确认     │
       ▼                  ▼                     ▼             │
   cancelled           failed            running_reviewer      │
                                                │              │
                              ┌─────────────────┼──────────┐   │
                        verdict=reject     verdict=pass  failed │
                              │                 │          │   │
                              └── 回开发 ────────┘          │   │
                                （round+1）      ▼          ▼   │
                                          running_tester  failed│
                                                │              │
                                    ┌───────────┼──────────┐    │
                              verdict=pass  verdict=reject │    │
                                    │           └──────────┘────┘
                                    ▼
                                  done
```

状态取值表：

| status | 含义 | 下一步由谁推进 |
|--------|------|--------------|
| `pending_confirm` | 分析完，等飞书「开始修」 | 用户点按钮 |
| `running_developer` | 开发角色 run 在跑 | 编排器（run 终态事件） |
| `pending_review_gate` | 开发完，等人工放行进评审 | 用户点按钮（可配置为自动） |
| `running_reviewer` | 评审角色 run 在跑 | 编排器 |
| `pending_dev_gate` | 评审打回，等人工确认重开发 | 用户点按钮（可配置为自动） |
| `pending_test_gate` | 评审通过，等人工放行进测试 | 用户点按钮（可配置为自动） |
| `running_tester` | 测试角色 run 在跑 | 编排器 |
| `done` | 全流程通过 | 终态 |
| `failed` | 某角色 run 失败或超出最大打回轮数 | 终态 |
| `cancelled` | 用户「跳过」或看板取消 | 终态 |

**打回上限**：`max_dev_rounds`（默认 3）。第 3 轮评审仍 reject 直接进 `failed`，
终态卡片说明「已达最大返工轮次，请人工接手」。没有上限的话，评审和开发能在
一个错误理解上互相打回到 token 烧穿。

**闸门粒度可配**：`[team_task].gates` 配置哪些边界要人工确认，
取值 `["dev_start", "review_start", "dev_restart", "test_start"]`。
默认只开 `dev_start`（即现有 task_gate 行为），其余边界自动流转——
先让全自动流水线跑通，再按需要加闸门，而不是一上来每个边界都弹卡片。

## 5. 数据库设计

三张新表，全部 `team_` 前缀，建表语句加到
`crates/hank-db/src/lib.rs` 的 `migrate()` 里（沿用 `CREATE TABLE IF NOT EXISTS` 幂等风格，
MySQL / utf8mb4，与 `agent_interactions` 同款索引命名）。

### 5.1 `team_tasks`

```sql
CREATE TABLE IF NOT EXISTS team_tasks (
    id            VARCHAR(36) PRIMARY KEY,
    -- 人类可读短编号 tsk_{base36 时间戳}_{4 位随机}，卡片与看板深链都用它
    task_no       VARCHAR(32) NOT NULL,
    session_id    VARCHAR(36) NOT NULL,
    user_id       VARCHAR(36) NOT NULL,
    -- 任务来源：feishu / dashboard（后续巡检接入时加 gitee 等取值）
    source        VARCHAR(32) NOT NULL DEFAULT 'feishu',
    -- 外部 issue 标识，如 IK5MOR；本期不巡检，仅从文本 #KEY 解析或看板手填
    issue_key     VARCHAR(64)  DEFAULT NULL,
    title         VARCHAR(255) NOT NULL DEFAULT '',
    goal          TEXT         DEFAULT NULL,
    -- 分析轮产出的四段 markdown，与 agent_interactions.analysis 同源冗余一份，
    -- 便于看板与后续角色 prompt 直接取用，不必回查交互单
    analysis      MEDIUMTEXT   DEFAULT NULL,
    status        VARCHAR(32)  NOT NULL DEFAULT 'pending_confirm',
    -- 当前角色：developer / reviewer / tester；终态时为 NULL
    current_role  VARCHAR(32)  DEFAULT NULL,
    -- 开发轮已用轮次，用于 max_dev_rounds 上限判定
    dev_rounds    INT          NOT NULL DEFAULT 0,
    -- 执行节点与后端在整个流水线内固定，中途不换节点（与 client-only 约定一致）
    backend         VARCHAR(32)  NOT NULL,
    exec_client_id  VARCHAR(36)  DEFAULT NULL,
    agent_kind      VARCHAR(32)  NOT NULL DEFAULT 'general_task',
    -- 飞书任务主卡（跨角色复用同一张卡片原地刷新）
    account_id      VARCHAR(36)  DEFAULT NULL,
    chat_id         VARCHAR(128) DEFAULT NULL,
    topic_id        VARCHAR(128) DEFAULT NULL,
    card_message_id VARCHAR(256) DEFAULT NULL,
    -- 闸门卡片 message_id：主卡要 reply 一条已有消息，建任务行时还没有卡片，
    -- 由 pusher 发闸门卡成功后回填；编排器派发首个角色时 reply 它生成主卡
    origin_message_id VARCHAR(256) DEFAULT NULL,
    result        MEDIUMTEXT   DEFAULT NULL,
    error         TEXT         DEFAULT NULL,
    created_at    DATETIME NOT NULL DEFAULT NOW(),
    updated_at    DATETIME NOT NULL DEFAULT NOW(),
    finished_at   DATETIME DEFAULT NULL,
    UNIQUE KEY uk_team_tasks_no (task_no),
    INDEX idx_team_tasks_status (status, updated_at),
    INDEX idx_team_tasks_session (session_id, created_at),
    INDEX idx_team_tasks_user (user_id, created_at)
) DEFAULT CHARSET=utf8mb4
```

`task_no` 生成规则（`format!("tsk_{}_{}", base36(now_ms), rand4)`）：
时间戳保证大致有序便于人眼排序，4 位随机避免同毫秒撞车，`UNIQUE KEY` 兜底重试一次。

`session_id` **不加外键**：与 `channel_messages` 同理，session 清理后仍要保留任务审计快照。

### 5.2 `team_task_runs`

一个角色的一次执行。打回重开发会新增一行（`round` +1），历史轮次全部保留。

```sql
CREATE TABLE IF NOT EXISTS team_task_runs (
    id          VARCHAR(36) PRIMARY KEY,
    task_id     VARCHAR(36) NOT NULL,
    role        VARCHAR(32) NOT NULL,
    -- 同一角色的第几轮，从 1 开始
    round       INT NOT NULL DEFAULT 1,
    -- 该角色本轮独占的 CLI thread；派发前为空，首个事件回来后写入
    thread_id   VARCHAR(128) DEFAULT NULL,
    status      VARCHAR(16)  NOT NULL DEFAULT 'running',
    -- 角色自评结论：pass / reject / failed；开发角色恒为 pass 或 failed
    verdict     VARCHAR(16)  DEFAULT NULL,
    -- 结构化交接产物（下一个角色的 prompt 输入），schema 见 §6.3
    handoff     JSON         DEFAULT NULL,
    -- 该角色输出的正文摘要（看板展示，卡片截断展示）
    summary     MEDIUMTEXT   DEFAULT NULL,
    -- 本轮新增改动文件数，沿用 git_dirty_paths 差集口径；查不到为 NULL
    dirty_files INT          DEFAULT NULL,
    error       TEXT         DEFAULT NULL,
    started_at  DATETIME NOT NULL DEFAULT NOW(),
    finished_at DATETIME DEFAULT NULL,
    UNIQUE KEY uk_team_run_role_round (task_id, role, round),
    INDEX idx_team_runs_task (task_id, started_at),
    FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
) DEFAULT CHARSET=utf8mb4
```

`UNIQUE KEY (task_id, role, round)` 是并发防线：编排器重复派发同一角色同一轮会插入失败，
而不是起两个并发 run。这与 `task_state::TaskRegistry` 的进程内闸门互补——
后者防同 session 并发，前者防（多实例或重试导致的）同轮次重复。

### 5.3 `team_task_events`

任务级时间线，看板左侧时间轴与飞书终态卡片的「流转记录」都读它。
与 `agent_events`（单 run 内的细粒度事件）分开：这里只记角色边界与人工决策，
一个任务通常十几行，不会膨胀。

```sql
CREATE TABLE IF NOT EXISTS team_task_events (
    id         BIGINT AUTO_INCREMENT PRIMARY KEY,
    task_id    VARCHAR(36) NOT NULL,
    -- role_started / role_finished / gate_opened / gate_answered
    -- / rejected / status_changed / cancelled
    kind       VARCHAR(32) NOT NULL,
    role       VARCHAR(32) DEFAULT NULL,
    round      INT         DEFAULT NULL,
    -- 人工决策事件记录操作者；系统流转为 NULL
    operator   VARCHAR(64) DEFAULT NULL,
    detail     TEXT        DEFAULT NULL,
    created_at DATETIME NOT NULL DEFAULT NOW(),
    INDEX idx_team_events_task (task_id, id),
    FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
) DEFAULT CHARSET=utf8mb4
```

### 5.4 复用 `agent_interactions`，不新建闸门表

角色边界闸门继续写 `agent_interactions`，新增两个 `kind` 取值：

| kind | 边界 | options |
|------|------|---------|
| `task_gate` | dev_start（已有） | 开始修 / 跳过 |
| `team_gate` | review_start / dev_restart / test_start | 继续 / 终止 |

`resume_ref` 里带 `{"team_task_id": ..., "next_role": "reviewer", "round": 1}`，
`interaction_flow` 按此把应答交给编排器。

这样做的理由：抢名额 → claim 卡片 → 原子应答 → 派发 → 失败回滚这一整套顺序
已经在 `answer_and_resume` 里踩平了（重复投递、话题重建丢单、节点离线回滚），
另建一张闸门表等于把这些坑重踩一遍。代价是 `interactions.rs` 的 `KINDS`
白名单要加 `team_gate`，admin 交互单页会多一种 kind——可以接受。

### 5.5 迁移与兼容

- 三张表都是 `IF NOT EXISTS` 新建，无既有数据改写，**无破坏性迁移**。
- 老的单角色 `task_gate` 链路保留：`team_task_enabled = false` 时
  `resume_task_gate` 走原路径（直接 resume 第二轮），行为与今天完全一致。
  开关打开后才由编排器接管。这样回滚只需改配置，不用回退代码。
- `sessions.metadata` 新增 `team_task_id` 字段（可空），让 run 终态回调能反查任务；
  `active_task_gate_id` 保留不动。

## 6. 服务端实现

### 6.1 模块划分

```
server/src/team_task/
├── mod.rs          # TeamTask/Role/Verdict 类型、task_no 生成、状态机纯函数
├── roles.rs        # 角色注册表（ROLE_DEFS）+ 每个角色的 prompt 构造
├── orchestrator.rs # 编排器：advance() 单一入口，派发下一步
├── card.rs         # 团队任务主卡（build_team_stage_card）
└── routes.rs       # /api/team/* REST（看板用）
```

**为什么独立目录而不是塞进 `cli_agent.rs`**：`cli_agent.rs` 已 3339 行，
且它的职责是「跑一次 CLI run」。编排是它上面一层，混进去会让「一次 run」
和「一串 run」两种时间尺度的状态纠缠在一个文件里。

### 6.2 角色注册表

沿用 `scheduler::JOB_DEFS` 的「定义在代码、状态在 DB」约定：

```rust
pub struct RoleDef {
    pub id: &'static str,            // "developer"
    pub label: &'static str,         // "开发"
    pub running_status: &'static str, // "running_developer"
    /// 该角色是否要求结构化 verdict（评审/测试要，开发不要）
    pub needs_verdict: bool,
    /// 产出 prompt：注入目标、分析、上游交接产物
    pub prompt: fn(&RolePromptInput) -> String,
}

pub const ROLE_DEFS: &[RoleDef] = &[
    RoleDef { id: "developer", label: "开发", running_status: "running_developer",
              needs_verdict: false, prompt: roles::developer_prompt },
    RoleDef { id: "reviewer",  label: "评审", running_status: "running_reviewer",
              needs_verdict: true,  prompt: roles::reviewer_prompt },
    RoleDef { id: "tester",    label: "测试", running_status: "running_tester",
              needs_verdict: true,  prompt: roles::tester_prompt },
];
```

加第四个角色（如「文档」）只需往这个数组加一行 + 写一个 prompt 函数，
状态机的流转顺序由数组顺序决定（`next_role()` 就是取下一个元素）。

### 6.3 交接产物（handoff）

每个角色的输出要能被下一个角色消费，所以 prompt 要求角色输出固定尾段：

```markdown
## 交接
verdict: pass            # 仅 needs_verdict 的角色
changed_files: 3
summary: 一句话说明本轮做了什么 / 判定理由
blocking: 阻塞项，没有则写 none
```

`orchestrator` 用一个宽松的解析器（`parse_handoff`）从 run 的最终文本里提取这段，
解析结果落 `team_task_runs.handoff`。**解析失败不算 run 失败**：
- 开发角色解析失败 → 用 `git_dirty_paths` 差集兜底 `changed_files`，`summary` 取正文末尾摘要。
- 评审/测试角色解析失败 → `verdict` 记 `unknown`，**任务直接进 `failed`**
  （error 写「评审结论无法解析」），由人在看板点重试。
  把「没读懂模型输出」默认成 pass 会让评审形同虚设，默认成 reject 会无谓返工。

  这里**不开人工闸门**，理由有三条，都是踩过的：
  1. 与 `gates` 配置冲突。`gates = []` 的语义是「全自动无人值守」，而飞书交互单的
     `expires_at` 是 NULL（不过期，飞书渠道既有约定），Unknown 强行开闸门会让任务
     永远挂在 `pending_*_gate` 等一个不会有人点的按钮——僵尸态既不推进也不告警，
     比 failed 更糟。
  2. 最后一个角色（tester）返回 Unknown 时**没有边界可开**。`GateBoundary` 四个变体
     全是「进入下一个角色」语义，tester 之后是 `Finish { done }`，不存在「最终验收」
     这个边界。硬要开闸门就得为此新增一个枚举变体 + 一条特例分支。
  3. Unknown 本身是异常路径，不该为它设计得太顺滑。正确的修法是把角色 prompt 调对，
     让它别输出解析不了的东西；给异常路径加人工兜底会掩盖 prompt 的问题。

  代价是浪费一轮已完成的开发产出。可接受——`team_task_runs` 保留了全部轮次记录，
  看板的「从当前角色重试」不会丢掉上一轮的工作。

### 6.4 编排器

单一入口，所有状态推进都从这里走：

```rust
/// 推进任务到下一步。幂等：重复调用同一状态不会重复派发（靠
/// team_task_runs 的 (task_id, role, round) 唯一键 + TaskRegistry 名额）。
pub async fn advance(state: &Arc<AppState>, task_id: &str, trigger: Trigger) -> Result<()>;

pub enum Trigger {
    /// 闸门被应答（继续 / 终止）
    GateAnswered { interaction_id: String, answer: String },
    /// 某个角色 run 走到终态
    RunFinished { role: String, round: i32, outcome: RunOutcome },
    /// 看板 / 飞书 /stop 取消
    Cancelled { operator: String },
}
```

`advance` 内部固定顺序：

1. 读 `team_tasks` 行，用**纯函数** `decide_next(status, trigger, cfg)` 算出下一步
   （`DispatchRole(role, round)` / `OpenGate(boundary)` / `Finish(status)` / `Ignore`）。
   纯函数是为了能单测——状态机分支多，走 DB 测太慢也测不全。
2. `Ignore` 直接返回（重复触发、终态任务被再次推进）。
3. `OpenGate`：创建 `team_gate` 交互单 + emit `AskUser` 让 pusher 出卡片，
   任务状态改 `pending_*_gate`。
4. `DispatchRole`：抢 `TaskRegistry` 名额 → 插 `team_task_runs` 行（唯一键防重）
   → 把该角色的 thread 写进 `sessions.metadata.agent_thread_id`
   → `cli_agent::run_cli_turn` → 起 pusher。
5. `Finish`：写终态 + `finished_at`，改主卡为终态，清 `TaskRegistry` 进度。

每步都往 `team_task_events` 记一行。

**`current_role` 的生命周期**（实现约定，编排器必须遵守）：
开闸门时 `current_role` **保持不变**，只有走终态才清空为 `NULL`。
因为 `decide_next` 在 `pending_*_gate` 分支要靠 `current_role` 算出下一个角色
（`next_role(cfg.roles, current)`）；如果开闸门时清掉它，函数里的兜底
（`unwrap_or("developer")` / `unwrap_or("reviewer")`）会静默派发错角色——
配置被裁剪成非默认顺序时尤其危险。

同理，`dispatch_from_pending_gate` 的两个分支兜底口径要统一：
`pending_review_gate` 找不到下一个角色时返回 `Finish { failed }`，
而 `pending_test_gate` 目前硬编码 `DispatchRole { "tester" }`，
后者可能派发一个不在 `cfg.roles` 里的角色。实践中走不到
（`gate_boundary_for_entering` 只在下一个角色确实是 tester 时才返回 `TestStart`），
但编排器接入时应一并改成 `Finish { failed }`。

**失败回滚**：`DispatchRole` 派发失败时（节点离线、超时）把 run 行标 `failed`、
任务标 `failed`，并释放名额——不做自动重试。自动重试在一个可能已经改了半个仓库的
工作区上重跑，比让人看一眼再决定更危险；看板提供「从当前角色重试」按钮。

### 6.5 run 终态如何回到编排器

现有 `cli_agent` 在 run 结束时会 emit `RunCompleted` / `RunFailed`，
并调 `finalize_open_task_gate` 收尾交互单。团队任务复用同一个钩子：

在 `execute_remote_turn` 的收尾处，若 `sessions.metadata.team_task_id` 非空，
则通知编排器推进（`Trigger::RunFinished`）。

**不能直接 `await advance`**：`advance` → `dispatch_role` → `run_cli_turn`
→ `execute_remote_turn` → `advance` 构成 async 递归，直接 await 会让 future
类型无限大、编译不过。实现改为 **channel + worker**：
`enqueue_run_finished` 只把终态入队，`start_run_finished_worker`（main 里起一次）
在独立协程里消费并调 `advance`。副作用是解耦了时序，run 的终态路径不会被编排阻塞。

入队点只有两处且互斥：`execute_remote_turn` 内部的 `Ok` 路径、
spawn 闭包里的 `Err` 路径。闸门轮通过 `finished_as_gate: true` 显式跳过——
任务此时停在 `pending_confirm` 等用户点按钮，不该由 run 终态推进。
**不要**在 7 处 `finalize_open_task_gate` 旁边各加一次入队，那会让一次 run
推进多次状态机。

已知取舍：channel 是 fire-and-forget，进程在入队后、worker 处理前被杀会丢掉
这次推进，任务停在 `running_*`，靠下次启动的 `fail_stale_team_tasks` 标 `failed`。
不会静默错乱，最坏是一次任务需要重试。

**关键约束（踩过的坑）**：终态判定必须回 `EventBuffer` 补读，
不能只订阅 broadcast——broadcast 订阅者在终态事件发出后才建立就永远收不到。
现有 pusher 已经这么做了，编排器直接复用 run 返回值（`ChatTurnHandle` 的终态）
而不是自己订阅事件流，从结构上避开这个问题。

### 6.6 闸门应答接入

`interaction_flow::answer_and_resume` 在 ③ 原子应答之后、④ 派发之前，
按 `kind` 分三路（现在是两路）：

```rust
match answered_row.kind.as_str() {
    "task_gate" if team_task_enabled => {
        // 新：进编排器，由它派发 developer 角色
        team_task::orchestrator::advance(state, &task_id,
            Trigger::GateAnswered { .. }).await
    }
    "task_gate" => { /* 旧路径：resume_task_gate 直接跑第二轮 */ }
    "team_gate" => {
        team_task::orchestrator::advance(state, &task_id,
            Trigger::GateAnswered { .. }).await
    }
    _ => { /* quant_confirm / ask_user：run_chat_turn 注入 tool_result */ }
}
```

抢名额、claim 卡片、失败回滚这些前置步骤完全不动。

### 6.7 分析轮的改动

`cli_agent::finish_as_task_gate` 除了落交互单，额外创建 `team_tasks` 行
（`status = pending_confirm`，写 `backend` / `exec_client_id` / `agent_kind` / 飞书上下文），
并把 `team_task_id` 写进 `sessions.metadata` 与交互单的 `resume_ref`。
`issue_key` 从用户原文里用 `#([A-Z0-9]{4,12})` 解析，取第一个匹配。

## 7. 飞书卡片

### 7.1 任务主卡（新增）

`team_task::card::build_team_stage_card`，一张卡片贯穿整个流水线原地刷新
（`card_message_id` 存在 `team_tasks`），标题随状态变化：

```
团队任务 · 开发 · 进行中              （running_developer）
团队任务 · 评审 · 进行中              （running_reviewer）
团队任务 · 测试 · 进行中              （running_tester）
团队任务 · 开发完成 · 待进入评审      （pending_review_gate）
团队任务 · 评审 → 开发（打回）        （pending_dev_gate）
团队任务 · 评审通过 · 待进入测试      （pending_test_gate）
团队任务 · 已完成 / 失败 / 已取消     （done / failed / cancelled）
```

header `template`：运行中 `blue`、待闸门 `orange`、`done` `green`、
`failed` `red`、`cancelled` `grey`。

正文分区（顺序固定）：

```
目标        （goal，超 500 字符按 chars() 截断）
基本信息    任务编号 / 状态 / 当前角色 / Issue（可空不渲染）/ 来源 / 后端 / 开发轮次
hr
流转记录    ✅ 开发 第1轮 · 改动 3 个文件
            🔄 评审 第1轮 · 进行中
            ❌ 评审 第1轮 · 打回：漏了错误处理
当前进展    （有 progress 时：进度条 + 最近活动；复用 feishu::card::build_progress_bar）
说明        （终态 reason，可空不渲染）
[在看板查看] http://{dashboard}/#/team/tsk_ms8oi5d9_94ak
```

首次：reply 到 `origin_message_id`（闸门卡）生成主卡并 `set_team_task_card`；
之后 `update_card` 原地刷新。`sync_team_card` 全程 best-effort。
节流：主卡跨角色离散同步，用进程内 `task_id → Instant` 最小 2s 间隔
（`ThrottledCardUpdater` 面向单角色连续 push，形状不适配）。

### 7.2 闸门卡片

`build_task_gate_card` 加两个可选字段：`task_no`、`issue_key`，
基本信息区从「任务编号/状态/当前角色/来源」改成截图的
「Issue / 状态 / 任务编号 / 来源」，并把「当前角色」下移到主卡。
`admin_url` 改为看板深链（见 §8.3）。字段可空，缺失时不渲染该格。

`team_gate` 卡片是同一个构造器换标题与按钮文案：
- review_start：`开发完成 · 是否进入评审`，按钮 继续评审 / 终止
- dev_restart：`评审打回 · 是否重新开发`，按钮 重新开发 / 终止（正文带打回理由）
- test_start：`评审通过 · 是否进入测试`，按钮 继续测试 / 终止

### 7.3 终态卡片

主卡改写为终态（`done` 绿 / `failed` 红 / `cancelled` 灰），
保留完整流转记录，附看板链接。

## 8. team 看板（独立前端）

### 8.1 工程结构

新建 `team/`，与 `admin/` 同栈（Vue 3.5 + Vite 6 + Tailwind 4 + TS），
风格对齐 admin，但**独立部署、独立端口**（开发 18789，与截图一致）：

```
team/
├── package.json          # 同 admin 的依赖集，去掉 xterm / qrcode
├── vite.config.ts        # base: '/', server.port: 18789, /api 代理到 3000
├── index.html
└── src/
    ├── main.ts           # createWebHashHistory（截图是 #team/... 形式）
    ├── App.vue
    ├── composables/api.ts # 复用 admin 的 request 封装与 TOKEN_KEY 约定
    └── views/
        ├── Login.vue      # 与 admin 同一 /api/auth/login，scope=admin
        ├── TaskBoard.vue  # 路由 /            任务泳道（按 status 分列）
        └── TaskDetail.vue # 路由 /team/:taskNo 任务详情
```

**路由用 hash**：截图的 `#team/tsk_xxx` 是 hash 形式。这与 admin 的 history
路由不同——admin 必须用 history 是因为它由 server 的 `ServeDir` 托管，
而看板独立部署（静态托管或本机 dev server），hash 路由不需要服务端 rewrite 配合。
注意这里有个已踩过的坑：飞书卡片深链**如果**指向 admin 必须是 history 形式
（`/admin/interactions/{id}`），写成 hash 会 404；看板是反过来的，
两套规则不要混用。

### 8.2 页面

**TaskBoard**：按 status 分泳道（待确认 / 开发中 / 待放行 / 评审中 / 测试中 /
已完成 / 失败 / 已取消；三个 `pending_*_gate` 合并为「待放行」），
卡片显示 `task_no`、目标首行、Issue、当前角色、已用时长、开发轮次。
轮询 `GET /api/team/tasks`（5s，仅非终态时开启，与 admin Jobs 页同款，不引入 WS）。

**TaskDetail**：
- 顶部：任务编号 / Issue / 状态 / 后端 / 耗时 / 最后修改
- 左侧时间轴：`team_task_events` 逐条（kind 中文）
- 右侧主区：按角色轮次折叠展示 `team_task_runs`（summary、handoff、改动文件数、错误）
- 分析全文（`team_tasks.analysis`，**纯文本 `<pre>`**，不引入 markdown 库）
- 操作区：运行中可取消，失败可从当前角色重试。
  **闸门应答不在看板做**——需要交互单 id 与 `/api/admin/interactions/{id}/answer`，
  属额外链路；待闸门时提示「请在飞书卡片上确认」。

### 8.3 REST 接口

`server/src/team_task/routes.rs`，挂在 `admin_api` 那一组
（同样 `admin_required` + `auth_middleware`，看板登录复用 admin JWT）。
配置接口在 `/api/admin/team-task/*`，看板数据在 `/api/team/*`，前缀刻意不同。

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/team/tasks` | 列表，支持 status / user_id / issue_key 筛选与分页 |
| GET | `/api/team/tasks/{task_no}` | 详情：任务 + runs + events |
| POST | `/api/team/tasks/{task_no}/cancel` | 取消（走 `Trigger::Cancelled`，终态幂等） |
| POST | `/api/team/tasks/{task_no}/retry` | 从当前角色重试（仅 failed 可用，round+1） |
| POST | `/api/team/tasks` | 看板手动建任务（source=dashboard）— **未实现** |

筛选参数沿用 `interactions::parse_filter_param` 的白名单校验，不放行任意值。

**看板 base URL 配置** `dashboard_base_url`（admin「团队任务」页可改，DB 优先），
深链格式只在 `team_task::card::build_dashboard_task_url` 一处定义
（仿照 `interaction_flow::admin_interaction_url`）：
`{dashboard_base_url}/#/team/{task_no}`。未配置时卡片不渲染看板链接行，
而不是拼出一个坏链接。

### 8.4 部署

看板是纯静态产物，两种方式都支持：
- 开发：`cd team && pnpm dev`（18789，`/api` 代理到 3000）
- 生产：`pnpm build` 后由任意静态服务器托管，或加一条
  `.nest_service("/team", ServeDir::new("team/dist"))` 由 server 托管
  （此时 `dashboard_base_url` 填 server 地址，hash 路由照样工作）

## 9. 配置

配置**存在数据库**（`settings` 表单行 JSON，key = `team_task_config`），
在 admin「团队任务」页（`/admin/team-task`）修改，**改完即时生效、无需重启**。

字段：

| 字段 | 说明 |
|------|------|
| `task_gate_enabled` | 两阶段闸门总开关。关闭时代码任务直接执行，不弹分析闸门 |
| `enabled` | 多角色流水线总开关。关闭时「开始修」走原来的单角色 resume |
| `roles` | 参与流水线的角色，按数组顺序流转。可裁剪成 `["developer"]` |
| `gates` | 需要人工确认的边界。默认只有 `dev_start`，其余自动流转 |
| `max_dev_rounds` | 评审打回后最多重新开发几轮（1–10），超出即 `failed` |
| `dashboard_base_url` | 看板地址，用于飞书卡片深链；留空则不渲染看板链接行 |

单个角色 run 的超时沿用 `[server_agent].agent_timeout_secs`，不另设。

**为什么在 DB 而不在 `config.toml`**：这些是运行时策略开关（要不要弹闸门、
要不要走流水线），不是部署基础设施。与 `job_states`（定时任务启停）、
`agent_cli_profiles`（CLI 凭据）同类。放文件里意味着改个开关要登服务器改文件 + 重启。

留在 `config.toml` 的 `[team_task]` 段与 `[server_agent].task_gate_enabled`
**降级为「DB 里还没有配置时的初始默认值」**，只在首次部署时兜底，
这样升级上线时行为不变、不必先去 admin 点一遍。读取路径见
`server/src/team_task/settings.rs` 的 `effective_with_source`：
DB 有值用 DB，DB 无值或读失败则退回 config.toml 并 `warn`（DB 抖一下不该让所有任务失败）。

**为什么每次读 DB 不缓存**：这些开关只在「派发一个任务」「推进一次状态机」时读，
一个任务全程也就几次，不是每 token 都读。直接读换来的是 admin 改完立刻生效、
没有缓存失效 bug、多实例共库天然一致。

**校验在写入时而非启动时**：`settings::validate` 由 admin REST 在保存前调用，
非法配置回 400 并给出中文原因。用户点保存立刻看到问题，而不是重启后服务起不来。
校验项：`enabled` 依赖 `task_gate_enabled`（流水线入口是分析轮）、
角色名合法且不重复（`next_role` 靠位置查找，重复会让流转错乱）、
边界名合法、`max_dev_rounds` 在 1–10（防手滑输入 1000 烧穿 token）。

与 `[server_agent].enabled` 完全解耦（client-only 链路照样能跑流水线）。

## 10. 并发与失败边界

这些是现有链路上已经踩过、必须在编排层同样守住的点：

| 场景 | 处理 |
|------|------|
| 同 session 两个角色同时派发 | `TaskRegistry::try_acquire` 抢名额（先抢名额再改状态，不能反） |
| 编排器重复 advance 同一状态 | `decide_next` 返回 `Ignore` + `(task_id, role, round)` 唯一键双保险 |
| 闸门挂着时用户在话题里派新任务 | 沿用 `supersede_stale_task_gates`：旧闸门作废、卡片改灰；同时把 `team_tasks` 标 `cancelled`（理由「已被新一轮取代」） |
| server 重启，任务停在 `running_*` | 启动时扫 `running_*` 的任务，对应 run 行标 `failed`、任务标 `failed`（仿 scheduler 对遗留 `running` job_run 的收尾）。**不自动续跑**——进程重启后 CLI thread 已不可信 |
| 节点离线 | 派发前 `is_client_online` + `client_reports_backend` 双检（与 `resume_task_gate` 同口径），失败则任务 `failed` 并在卡片说明，不换节点 |
| 角色 run 超时 | 沿用 `agent_timeout_secs`，超时按 run 失败处理 |
| 评审 verdict 解析不出来 | 记 `unknown` → 任务 `failed`，看板重试；不猜、不开闸门（理由见 §6.3） |
| 打回死循环 | `max_dev_rounds` 上限 |
| 多实例共库 | 编排器由 run 终态驱动，run 只在派发它的那个实例上跑，天然不重复；唯一键兜底 |

## 11. 测试

纯函数优先，这类状态机走 DB 测既慢又覆盖不全：

- `decide_next(status, trigger, cfg)`：状态机全分支单测，包括
  打回、上限、闸门开关组合、终态被重复触发。
- `parse_handoff`：正常四段、缺字段、verdict 拼写异常、模型加了额外前言。
- `task_no` 生成：格式与唯一性（同毫秒多次生成不重复）。
- `build_team_stage_card` / 改后的 `build_task_gate_card`：
  沿用 `card.rs` 现有测试风格（断言 callback value 与字段渲染，
  含 `task_no`/`issue_key` 缺失时不渲染该格）。
- 集成层面手工验证清单：
  1. 全自动（`gates = ["dev_start"]`）跑通 开发 → 评审 → 测试 → done
  2. 评审打回一次后重开发通过
  3. 连续打回触顶 `max_dev_rounds` → failed
  4. 每个边界都开闸门时的四次点击
  5. 开发轮中途节点离线
  6. `enabled = false` 时老路径行为不变（回归）

## 12. 实施顺序

按「每步都能独立验证」切分，不做一次性大合并：

1. ✅ **数据层**：三张表 + `hank-db` 的 CRUD（`create_team_task` / `list_team_tasks`
   / `get_team_task_by_no` / `insert_team_run` / `finish_team_run` / `append_team_event`）。
2. ✅ **状态机纯函数**：`team_task/mod.rs` 的 `decide_next` + `parse_handoff` + `task_no`，
   带完整单测。此时不接任何链路。
3. ✅ **角色注册表与 prompt**：`roles.rs`，三个角色的 prompt 与交接段要求。
4. ✅ **编排器**：`orchestrator.rs` 的 `advance`，接 `run_cli_turn` 与 `TaskRegistry`。
5. ✅ **接入分析轮与闸门**：`finish_as_task_gate` 建任务行、`answer_and_resume` 分三路，
   配置开关加上。此时飞书链路已可跑通（卡片先复用现有闸门卡）。
   含 05a 配置入库 + 05b admin 配置页。
6. ✅ **飞书主卡**：`team_task/card.rs` + `origin_message_id` + `sync_team_card`，
   编排器四个状态点刷新。
7. ✅ **REST**：`team_task/routes.rs` 看板列表/详情/取消/重试挂到 `admin_api`。
8. ✅ **看板前端**：`team/` 工程，Login → TaskBoard → TaskDetail，端口 18789。
9. ✅ **文档**：`docs/feishu.md` 增「团队任务流水线」小节，`CLAUDE.md` 补目录与页面表。

1–4 步不改任何现有行为，可以放心先落地。第 5 步是唯一动到现有链路的地方，
且由 `team_task.enabled` 守着，默认关闭。

## 13. 本期不做

- **Issue 巡检入队**（Gitee/GitHub/Jira）：`issue_key` 字段与 `source` 取值已预留，
  接入时新增一个 scheduler job 往 `team_tasks` 插 `pending_confirm` 行即可，
  编排器不用改。
- **角色并行**：三角色严格串行。并行需要处理同一工作区的写冲突，
  与当前「一个 session 一个 run」的模型冲突，不在本期范围。
- **跨节点角色分配**（开发在 A 机、测试在 B 机）：整个流水线固定一个节点。
- **看板实时推送**：先轮询，需要再上 WS。
- **自动重试**：failed 任务由人在看板点重试。
- **看板手动建任务**（`POST /api/team/tasks`）：需要选节点、选 backend、跑分析轮，
  是另一条独立入口，本期未实现。
- **看板内闸门应答**：需要交互单 id 与 admin answer 链路，引导去飞书卡片确认。
- **markdown 渲染**：分析全文在看板为纯文本 `<pre>`，不引入 markdown 库。
