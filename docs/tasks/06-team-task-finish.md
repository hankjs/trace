# 任务 06：团队任务流水线收尾（主卡 + REST + 看板 + 文档）

> `docs/feature/team-task-pipeline.md` 实施顺序的**第 6–9 步，合并成一份**。
> 第 1–5 步（数据层、状态机、角色 prompt、编排器、飞书链路、配置入库、admin 配置页）
> 已完成并提交（`6551d6c`、`fe7c848`）。
>
> **本文档比之前每份都大，请按 A→B→C→D 四个阶段顺序执行，每阶段做完先自测再进下一阶段。**
> 阶段之间有依赖：B 依赖 A 的表字段、C 依赖 B 的接口、D 记录 A–C 的结果。
>
> 动手前必须通读 `docs/feature/team-task-pipeline.md` 全文，特别是
> §6.4（编排器与 `current_role` 生命周期）、§7（飞书卡片）、§8（看板）、§9（配置）。

## 总体背景

流水线现在**能跑但看不见**。已经能做到：飞书派代码任务 → 只读分析 → 闸门卡片 →
点「开始修」→ 开发/评审/测试三角色串行执行 → 三张表留完整记录。

缺的是：

1. **飞书里看不到进度**。`team_tasks.card_message_id` 永远是 NULL —— 第 5 步写的
   pusher 接入有一句「`card_message_id` 为空则跳过 pusher」，而**没有任何代码写这个字段**
   （`set_team_task_card` 至今零调用方）。所以角色 run 跑起来后飞书是静默的。
2. **没有看板**。`list_team_tasks` / `get_team_task_by_no` / `list_team_events`
   三个 DB 方法零调用方，等着 REST 来用。
3. **文档没记**。`docs/feishu.md` 里没有团队任务这一节。

本文档把这三块补齐。

## 阶段 A（原第 6 步）：飞书团队任务主卡

### A0. 先解决「主卡挂在哪条消息下」

飞书的 `reply_card` 需要一个被回复的 `message_id`。而主卡要在**用户点「开始修」之后**
才出现（此时才真正开始干活），那时手上没有原始消息 id ——
`finish_as_task_gate` 建任务行时还没有卡片（卡片是 pusher 收到 `AskUser` 后才发的）。

解法：给 `team_tasks` 加一列 `origin_message_id`，由 pusher 在发闸门卡片时回填，
编排器派发首个角色时回复它、生成主卡。

- [ ] **A0.1** 用**现有的** `Database::ensure_column` helper（`crates/hank-db/src/lib.rs`
  里已有，check-then-alter 幂等）加列，不要手写 `ALTER TABLE`：

```rust
Self::ensure_column(
    &pool,
    "team_tasks",
    "origin_message_id",
    "ALTER TABLE team_tasks ADD COLUMN origin_message_id VARCHAR(256) DEFAULT NULL",
).await?;
```

放在三张 `team_*` 建表语句之后。同时给 `TeamTask` 结构加
`pub origin_message_id: Option<String>`，并把它加进 `TEAM_TASK_COLS`
（**注意**：该常量的列顺序必须与结构体字段顺序一致，加在 `card_message_id` 之后）。

- [ ] **A0.2** 加访问器：

```rust
/// 回填闸门卡片的 message_id，供后续主卡 reply 使用。
pub async fn set_team_task_origin_message(&self, task_id: &str, message_id: &str) -> Result<()>;
```

- [ ] **A0.3** 在 `server/src/feishu/pusher.rs` 的 `AskUser` 分支里，
  成功拿到 `card_mid` 之后（现在那里已经在调 `set_interaction_card`），
  若该交互单的 `resume_ref` 里有 `team_task_id`，则一并调
  `set_team_task_origin_message`。失败只 `warn`，不影响卡片本身。

### A1. 主卡构造

- [ ] **A1.1** 新建 `server/src/team_task/card.rs`。入参用结构体（字段多）：

```rust
/// 团队任务主卡入参。一张卡片贯穿整条流水线原地刷新，
/// 所以它要能表达「任意角色 / 任意阶段」的状态。
pub struct TeamStageCardOptions {
    pub task_no: String,
    pub goal: String,
    pub status: String,
    /// 当前角色 id；终态为 None
    pub current_role: Option<String>,
    pub issue_key: Option<String>,
    pub source_label: String,
    pub backend: String,
    pub dev_rounds: i32,
    /// 已完成 + 进行中的轮次，用于渲染「流转记录」
    pub runs: Vec<TeamStageRun>,
    /// 当前进度（来自 TaskRegistry 快照）；无则不渲染进度区
    pub progress: Option<TeamStageProgress>,
    pub dashboard_url: Option<String>,
    /// 终态说明（失败原因 / 取消理由）
    pub reason: Option<String>,
}

pub struct TeamStageRun {
    pub role_label: String,
    pub round: i32,
    pub status: String,
    pub verdict: Option<String>,
    pub summary: Option<String>,
    pub dirty_files: Option<i32>,
}

pub struct TeamStageProgress {
    pub percent: u32,
    pub detail: String,
    pub activities: Vec<String>,
}
```

- [ ] **A1.2** 构造函数：

```rust
/// 团队任务主卡（schema 2.0，update_multi）。
///
/// 标题随状态变化，对齐设计文档 §7.1：
///   running_developer     → 团队任务 · 开发 · 进行中
///   pending_review_gate   → 团队任务 · 开发完成 · 待进入评审
///   pending_dev_gate      → 团队任务 · 评审 → 开发（打回）
///   done / failed / cancelled → 团队任务 · 已完成 / 失败 / 已取消
pub fn build_team_stage_card(opts: &TeamStageCardOptions) -> Value;
```

正文分区（顺序固定）：

1. **目标** —— `goal`，超 500 字符按 `chars()` 截断
2. **基本信息** —— 任务编号 / 状态 / 当前角色 / Issue / 来源 / 后端。
   `issue_key` 为 `None` 时**不渲染该格**（不要显示空值或 `None`）
3. `hr`
4. **流转记录** —— 每个 run 一行：`✅ 开发 第1轮 · 改动 3 个文件`、
   `🔄 评审 第1轮 · 进行中`、`❌ 评审 第1轮 · 打回：漏了错误处理`。
   `summary` 每行截断到 80 字符
5. **当前进展** —— 有 `progress` 时渲染进度条 + 最近活动。
   **复用 `feishu/card.rs` 里已有的 `build_progress_bar`**（把它改成 `pub(crate)`
   或 `pub`，不要复制一份）
6. **终态说明** —— 有 `reason` 时渲染
7. 看板链接 —— `dashboard_url` 为 `None` 时整行不渲染

header `template` 按状态取色：运行中 `blue`、待闸门 `orange`、
`done` `green`、`failed` `red`、`cancelled` `grey`。

- [ ] **A1.3** 单测（放 `card.rs` 的 `#[cfg(test)] mod tests`，
  风格照 `feishu/card.rs` 现有测试）：
  - 各状态标题与 `template` 正确
  - `issue_key = None` 时正文不含「Issue」字样
  - `dashboard_url = None` 时不含 `http`
  - 流转记录三种状态（完成/进行中/打回）各渲染一行
  - 超长 `goal` 按字符截断且不切坏中文
  - `progress = None` 时不含「当前进展」

### A2. 主卡生命周期

- [ ] **A2.1** 在 `server/src/team_task/` 加一个模块内 helper（放 `card.rs` 或
  单独 `card_sync.rs`，二选一，保持一个文件一个职责）：

```rust
/// 刷新（必要时首次创建）团队任务主卡。
///
/// 首次：reply 到 origin_message_id 生成卡片，把 message_id 存回
/// team_tasks.card_message_id。之后：update_card 原地刷新同一张卡。
///
/// 整个函数是 best-effort：任何一步失败只 warn，绝不向上传播——
/// 卡片是可观测性，不该让它的故障影响任务执行。
pub async fn sync_team_card(state: &Arc<AppState>, task_id: &str);
```

内部顺序：
1. 读 `team_tasks` 行；读不到直接返回
2. `account_id` / `chat_id` 缺失或账号已停用 → `warn` 返回
   （对齐第 5 步 pusher 接入的降级口径）
3. 组装 `TeamStageCardOptions`：`runs` 来自 `list_team_runs`，
   `progress` 来自 `state.tasks.progress(&session_id)`，
   `dashboard_url` 用 `settings::effective(state).await.dashboard_base_url`
   拼 `{base}/#team/{task_no}`（**格式只在这一个地方定义**，
   仿 `interaction_flow::admin_interaction_url` 的做法）
4. `card_message_id` 为空 → 需要 `origin_message_id`，`reply_card` 后
   `set_team_task_card`；两者都空则 `warn` 返回
5. 否则 `update_card`

- [ ] **A2.2** 在编排器的四个状态变更点调 `sync_team_card`：
  `dispatch_role` 成功后、`open_gate` 之后、`finish_task` 之后、
  以及 `finalize_run` 之后（让流转记录及时出现）。
  **调用点一律 `sync_team_card(...).await;` 不接收返回值**——它没有返回值。

- [ ] **A2.3** 主卡刷新要节流。角色 run 期间 pusher 会高频更新进度，
  若每次都刷主卡会撞飞书频控。**复用 `feishu/card.rs` 里已有的 `CardUpdater`**
  （2s 合并推送）而不是新写一套；若 `CardUpdater` 的形状不适配跨角色场景，
  则在 `sync_team_card` 里加一个最小间隔判断（进程内 `HashMap<task_id, Instant>`
  放 `AppState` 或模块内 `OnceLock`），并注释说明为何不用 `CardUpdater`。

- [ ] **A2.4** 修掉第 5 步遗留：`orchestrator.rs` 里 pusher 接入那段
  「`card_message_id` 为空则跳过」现在应该能拿到值了。确认逻辑顺序是
  **先 `sync_team_card` 建主卡、再起 pusher**，否则首个角色的进度仍然丢。

## 阶段 A 自测

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test --workspace
```

- clippy 基线 **46** 个 warning，无新增
- `team_task` 测试数 ≥ 73 + A1.3 新增，全绿
- `cargo test --workspace` 全绿

---

## 阶段 B（原第 7 步）：看板 REST

在**已有的** `server/src/team_task/routes.rs`（05a 建的，现有 `get_config`
/ `update_config`）里追加。**不要新建文件。**

### B1. 接口清单

全部挂在 `main.rs` 的 `admin_api` 组（沿用 `admin_required` + `auth_middleware`，
看板登录复用 admin JWT）：

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/team/tasks` | 列表，支持 `status` / `user_id` / `issue_key` 筛选 + 分页 |
| GET | `/api/team/tasks/{task_no}` | 详情：任务 + runs + events |
| POST | `/api/team/tasks/{task_no}/cancel` | 取消 |
| POST | `/api/team/tasks/{task_no}/retry` | 从当前角色重试（仅 `failed` 可用） |

> 设计文档 §8.3 还列了 `POST /api/team/tasks`（看板手动建任务）。
> **本任务不实现它**：手动建任务需要选节点、选 backend、跑分析轮，
> 是另一条独立入口，塞进来会让本阶段过大。在文档里标注为未实现。

- [ ] **B1.1 列表**。直接用 `db.list_team_tasks(status, user_id, issue_key, page, per_page)`
  （第 1 步已实现）。筛选参数**必须过白名单**，沿用
  `interactions::parse_filter_param`（已 `pub`）：`status` 的合法值是
  `team_task` 的 9 个状态常量。非法值回 400，不放行任意字符串。
  `per_page` 上限 100，默认 20。响应形状复用 `admin::PaginatedResponse`。

- [ ] **B1.2 详情**。`get_team_task_by_no` → `list_team_runs` + `list_team_events`，
  一次返回三段：

```json
{ "task": {...}, "runs": [...], "events": [...] }
```

查不到回 404。

- [ ] **B1.3 取消**。走编排器，**不要**直接改库状态：

```rust
orchestrator::advance(&state, &task.id,
    Trigger::Cancelled { operator: claims.username.clone() }).await
```

这样卡片刷新、事件记录、进度清理都跟着走。终态任务再取消会被
`decide_next` 判 `Ignore`（幂等），返回 200 + 当前状态即可，不必报错。

- [ ] **B1.4 重试**。**只允许 `status == "failed"`**，其他状态回 400。

语义：把任务从「当前角色的下一轮」重新派发。实现：
1. 读 `latest_team_run` 拿到失败时的 `role` 与 `round`
2. 该角色的新轮次 = `round + 1`
3. 任务状态改回该角色的 `running_status`、`current_role` 设回该角色
4. 调 `orchestrator::dispatch_role`（需要把它从私有改成
   `pub(crate)`，或在 orchestrator 里加一个
   `pub async fn retry_from_current_role(state, task_id) -> Result<()>` 包一层
   —— **优先后者**，避免把编排细节暴露给 routes）

> 为什么用 `round + 1` 而不是复用原 round：`team_task_runs` 的
> `(task_id, role, round)` 唯一键会拒绝重复插入。递增轮次同时保留了
> 失败那轮的记录，看板上能看到「第 1 轮失败、第 2 轮重试」。

- [ ] **B1.5** 重试要防并发：调 `dispatch_role` 前它内部已经抢 `TaskRegistry` 名额，
  拿不到会返回「已有在途派发」并跳过。routes 层把这种情况转成 409 或
  200 + 提示文案，**不要**当成成功。

### B2. 路由注册

- [ ] **B2.1** 在 `server/src/main.rs` 的 `admin_api` 里挂四条，
  紧邻 05a 加的那两条 `/api/admin/team-task/config`。

> 注意路径前缀不一致是刻意的：配置接口在 `/api/admin/team-task/*`
> （admin 页面用），看板数据接口在 `/api/team/*`（看板前端用）。
> 两者都在 `admin_api` 组内、都要 admin JWT。若嫌不一致，
> **保持现状不要改** —— 05a 的配置接口已验收，改路径会破坏 admin 页面。

### B3. 单测

- [ ] **B3.1** 能提纯的部分做纯函数测（沿用第 4 步定下的策略，
  项目无 DB mock 基建）：
  - `status` 白名单校验：合法值通过、非法值拒绝、空值表示不限
  - `per_page` 归一：0 → 默认、超 100 → 截到 100
  - 重试前置条件判定：`failed` 允许、其他 8 个状态拒绝
    （抽成 `fn can_retry(status: &str) -> bool` 并单测）

## 阶段 B 自测

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server
```

手工验证（本机起 server + admin JWT，不需要飞书）：
- [ ] `GET /api/team/tasks` → 200，返回分页结构
- [ ] `GET /api/team/tasks?status=bogus` → **400**
- [ ] `GET /api/team/tasks/tsk_notexist` → **404**
- [ ] 对一个 `done` 任务 `POST .../cancel` → 200 且状态不变（幂等）
- [ ] 对一个非 `failed` 任务 `POST .../retry` → **400**

---

## 阶段 C（原第 8 步）：team 看板前端

新建 `team/` 工程。与 `admin/` 同栈（Vue 3.5 + Vite 6 + Tailwind 4 + TS），
**独立部署、独立端口 18789**。

### C1. 工程骨架

- [ ] **C1.1** 目录结构：

```
team/
├── package.json          # 照 admin/package.json 删掉 xterm / qrcode / highlight.js
├── vite.config.ts        # base: '/', server.port: 18789, /api 代理
├── tsconfig.json         # 照抄 admin/tsconfig.json
├── index.html
└── src/
    ├── main.ts           # createWebHashHistory
    ├── App.vue
    ├── style.css         # 照 admin 的 Tailwind 入口
    ├── composables/api.ts
    └── views/
        ├── Login.vue
        ├── TaskBoard.vue   # 路由 /
        └── TaskDetail.vue  # 路由 /team/:taskNo
```

- [ ] **C1.2** `vite.config.ts`：

```ts
const apiTarget = process.env.HANK_API ?? 'http://127.0.0.1:3000'
export default defineConfig({
  base: '/',
  plugins: [vue(), tailwindcss()],
  server: { port: 18789, proxy: { '/api': apiTarget } },
})
```

`base: '/'` 而非 `/admin/`：看板独立部署在自己的根路径下。

- [ ] **C1.3 路由用 hash**（`createWebHashHistory`）。
  截图里的深链是 `http://127.0.0.1:18789/#team/tsk_xxx`，A2.1 拼的也是这个格式。

> ⚠️ 这与 admin 相反，别混：admin 由 server 的 `ServeDir` 托管、必须用
> history 路由（写成 hash 会 404，见 git 历史 `416f87e`）；看板独立部署，
> hash 路由不需要服务端 rewrite 配合。

- [ ] **C1.4** `composables/api.ts`：照抄 admin 的 `request` 封装
  （含 401 跳登录），但 **`TOKEN_KEY` 换成 `hank_team_token`**，
  跳转目标换成 `#/login`。不要复用 admin 的 key —— 两个前端共用
  localStorage key 会互相踢登录态。

### C2. 页面

- [ ] **C2.1 Login.vue**：调 `/api/auth/login`，`scope: 'admin'`
  （看板数据接口在 `admin_api` 组内，要 admin JWT）。照 `admin/src/views/Login.vue`。

- [ ] **C2.2 TaskBoard.vue**：按 status 分泳道。
  - 泳道：待确认 / 开发中 / 待放行 / 评审中 / 测试中 / 已完成 / 失败 / 已取消
    （把 `pending_review_gate` / `pending_dev_gate` / `pending_test_gate`
    合并成一个「待放行」泳道，否则列太多）
  - 卡片显示：`task_no`、目标首行、`issue_key`、当前角色、
    已用时长（`created_at` 到 `finished_at` 或现在）、开发轮次
  - 点卡片进详情
  - **轮询 5s**（`setInterval`），仅在有非终态任务时开启，
    离开页面 `onBeforeUnmount` 清掉。照 `admin/src/views/Jobs.vue` 的
    `syncPolling` 写法

- [ ] **C2.3 TaskDetail.vue**：
  - 顶部：任务编号 / Issue / 状态 / 后端 / 耗时 / 最后修改
  - 左侧时间轴：`events` 逐条（kind 翻译成中文）
  - 右侧主区：按 `runs` 折叠展示每个角色轮次
    （summary、handoff、改动文件数、verdict、错误）
  - 分析全文（`task.analysis`）：**纯文本 `<pre>` 展示即可，不要引入
    markdown 渲染库**（`package.json` 不许加依赖）。设计文档 §13 里
    admin 那边也是同样的取舍
  - 操作区：
    - 运行中 → 「取消」按钮，调 `POST .../cancel`
    - `failed` → 「从当前角色重试」按钮，调 `POST .../retry`
    - 待闸门 → 提示「请在飞书卡片上确认」并给出说明。
      **不要**在看板里做闸门应答 —— 那需要交互单 id 与
      `/api/admin/interactions/{id}/answer`，属额外链路，本阶段不做
  - 操作后重新拉详情（不做乐观更新）

### C3. 不做的事

- [ ] 不要 WebSocket / SSE，先轮询（设计文档 §13 明确推迟）
- [ ] 不要看板手动建任务（阶段 B 已说明不实现）
- [ ] 不要在 `package.json` 加任何依赖（markdown 渲染、图表、拖拽都不要）
- [ ] 不要改 `admin/`、`server/`、`crates/`

## 阶段 C 自测

```bash
cd team && pnpm install && pnpm build
```

- `pnpm build` 成功，`vue-tsc` 零错误
- `pnpm dev` 起在 18789，`/api` 能代理到 3000

手工验证：
- [ ] 登录后看到泳道，已有任务出现在对应列
- [ ] 点卡片进详情，能看到 runs 与 events
- [ ] 飞书卡片上的看板链接点开能直达对应任务详情
      （验证 A2.1 拼的 URL 与 C1.3 的 hash 路由对得上）
- [ ] 全终态时轮询停止（开浏览器 Network 面板确认没有持续请求）

---

## 阶段 D（原第 9 步）：文档

- [ ] **D1. `docs/feishu.md` 新增「团队任务流水线」小节**，放在
  「六、排障」之前（与现有章节序号衔接，若有编号则顺延）。内容：
  - 一句话说清是什么：代码任务从单角色两阶段扩展成开发→评审→测试串行流水线
  - 开关在哪：admin「团队任务」页，**改完即时生效无需重启**；
    依赖两阶段闸门（流水线入口是分析轮）
  - 流转图（照 `docs/feature/team-task-pipeline.md` §4 的状态机简化版）
  - 每个角色干什么、独占 CLI thread、角色间用产物交接不共享上下文
  - 看板地址与深链格式
  - **排障条目**（追加到现有「六、排障」列表）：
    - 主卡不出现 → 检查任务是否有 `origin_message_id`
      （闸门卡片没发成功时为空）
    - 评审 verdict 是 unknown、任务莫名 failed → 模型没按格式输出交接段，
      看 `team_task_runs.summary` 原文
    - 任务卡在 `running_*` → 可能是 run 终态回调丢了
      （channel fire-and-forget，进程被杀会丢），重启后会被
      `fail_stale_team_tasks` 标 failed，在看板点重试
    - 打回反复到上限 → `max_dev_rounds` 触顶是有意行为，需人工接手

- [ ] **D2. `CLAUDE.md`**：
  - 目录结构里 `server/src/team_task/` 那行已有（本轮前面已补），
    确认描述准确
  - 加 `team/` 到目录结构（与 `admin/`、`cli/`、`quant/` 并列），
    注明「A股看板同款独立前端，端口 18789」
  - 「常用命令」加 team 看板的 `pnpm dev` / `pnpm build`
  - Admin 页面表里 `/team-task` 那行已有，确认准确

- [ ] **D3. `docs/feature/team-task-pipeline.md` 同步实际实现**。
  这份是长期设计文档，必须与代码一致。要改的地方：
  - §5 数据库设计：`team_tasks` 补 `origin_message_id` 列
  - §7.1 主卡：把标题与分区改成 A1.2 的实际形状
  - §8.3 REST：标注 `POST /api/team/tasks`（手动建任务）**未实现**
  - §8.2 看板：标注闸门应答不在看板做（引导去飞书）
  - §12 实施顺序：全部标记完成
  - §13 本期不做：补上「看板手动建任务」「看板内闸门应答」
    「markdown 渲染（分析全文为纯文本）」

- [ ] **D4.** 本文档（`docs/tasks/06-team-task-finish.md`）**执行完后不要删**，
  留给我 review。review 通过后由我决定是否清理。

---

## 全局边界（四个阶段共同遵守）

**不许碰**：
- `crates/` 下除 `hank-db`（仅 A0 加一列 + 一个访问器）外的任何 crate
- `client/`、`quant/`、`cli/`
- `admin/`（05b 已验收；阶段 C 是新的 `team/` 工程，不改 admin）
- `server/src/feishu/` 下除 `pusher.rs`（仅 A0.3 回填一处）与
  `card.rs`（仅把 `build_progress_bar` 改成可见）外的任何文件
- `config.toml`、`Cargo.toml`、`admin/package.json`
- `server/src/team_task/settings.rs` 与 `routes.rs` 的 `get_config`
  / `update_config`（05a 已验收）
- `server/src/interaction_flow.rs`、`server/src/interactions.rs`

**不许做**：
- 不要改 `decide_next` / `parse_handoff` / `ROLE_DEFS` / 状态常量
- 不要改任何既有单测的**断言**
- 不要新建 settings 表或第二套配置读取路径
- 不要给编排器引入 DB mock
- 不要在 `team/package.json` 加依赖
- 不要把配置读取从 `settings::effective` 改回 `state.config`

**保留**：第 1–5 步（含 05a/05b）的全部成果。
除本任务涉及的文件外不要 `git checkout` 或回退任何内容。

## 最终验收

```bash
# 后端
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test --workspace

# 两个前端
cd admin && pnpm build
cd ../team && pnpm install && pnpm build
```

期望结果：
- 编译成功。`server/src/deployment.rs` 那 5 个既有 `never used` warning 属正常
- clippy 基线 **46** 个 warning，无新增
- `cargo test -p hank-server team_task` ≥ 73 + 本任务新增，全绿
- `cargo test --workspace` 全绿，测试总数只增不减
- `admin` 与 `team` 两个 `pnpm build` 都零 TS 错误
- `git status` 改动范围符合上面「不许碰」清单

### 端到端手工验证（需要真实飞书 + 在线 hank-cli 节点）

前置：admin「团队任务」页开启两个开关，`gates` 只留 `dev_start`。

- [ ] 飞书新话题派一个小改动任务
- [ ] 收到闸门卡片（分析四段齐全）
- [ ] 点「开始修」→ **主卡出现**，标题「团队任务 · 开发 · 进行中」
- [ ] 开发跑完 → 主卡原地刷新成评审阶段，流转记录出现「✅ 开发 第1轮」
- [ ] 评审、测试依次跑完 → 主卡变绿「已完成」，三条流转记录齐全
- [ ] 主卡上的看板链接点开 → 直达该任务详情页
- [ ] 看板详情页的 runs / events 与飞书主卡一致
- [ ] 三张表数据完整：`team_tasks.status = done`、
      `team_task_runs` 三行 verdict 正确、`team_task_events` 有完整流转

补充场景（能测就测）：
- [ ] 评审打回 → 主卡显示「评审 → 开发（打回）」，开发第 2 轮
- [ ] 开发中途关掉 hank-cli → 主卡变红失败，看板可点重试
- [ ] 看板点重试 → 新一轮跑起来，失败那轮记录仍在

## 约定

遵循 `CLAUDE.md`：

- **中文注释 + 中文界面文案 + 中文 commit message**
- 以下六处必须注释写清「为什么」：
  1. `origin_message_id` 的存在意义 —— 主卡要 reply 一条已有消息，
     而建任务行时还没有卡片
  2. `sync_team_card` 全程 best-effort —— 卡片是可观测性，
     不该让它的故障影响任务执行
  3. 看板深链格式只在一处定义 —— 仿 `admin_interaction_url`
  4. 看板用 hash 路由而 admin 用 history —— 托管方式不同，
     admin 写成 hash 会 404
  5. 看板 `TOKEN_KEY` 与 admin 不同 —— 共用会互相踢登录态
  6. 重试用 `round + 1` —— 唯一键拒绝重复，且保留失败轮次记录
- 建议分四次提交（每阶段一次），commit message 形如：
  - `feat(team-task): 飞书团队任务主卡，跨角色原地刷新`
  - `feat(team-task): 看板 REST，列表/详情/取消/重试`
  - `feat(team): team 看板前端，泳道与任务详情`
  - `docs(team-task): 补飞书指南与设计文档，同步实际实现`
- 前端用 `<script setup lang="ts">` + Composition API + Tailwind utility class
- API 调用集中在 `composables/api.ts`，页面不直接 `fetch`
