# 任务：hank-cli 节点（client）支持停用/启用 + 记录最后运行时间与最后在线时间

## 背景与目标

admin 的「终端」页（`admin/src/views/Terminals.vue`）分两层：

- 左上「桌面 CLIENT」= **hank-cli 节点层**（`client_agents` 表里的注册记录）
- 左下「终端会话」= **终端会话层**（活在 hank-cli 进程内存里的 PTY 会话）

**终端会话层的停用/启用与活跃时间已经做完了**（见 `docs/tasks/terminal-enable-and-activity.md`，
已合并为 commit `7b99fbe`）。本任务要给**节点层**补上同样的三件事：

1. **停用/启用**：一台机器上的 hank-cli 长期挂着，但不希望它再被自动选中承接任务
   （微信 / 飞书 / cli_agent 都会通过 `pick_online_client` 之类的函数自动挑节点，
   多台在线时经常挑错机器）。希望能在 admin 页面一键停用某个节点。
2. **最后运行时间**：这个节点上一次真的被派发任务是什么时候。
3. **最后在线时间**：这个节点上一次长轮询（poll）是什么时候。

现状是左侧只有一个绿点表示在线，一排 id 摆在那里分不清哪个是遗留的空壳
（截图里 `2efe9e65`、`cb0af3e9`、`ser972044545164` 都无从判断）。

### 与终端会话层的关键差异：状态必须持久化

终端会话的状态存在 cli 内存里、重启即丢，那是符合预期的。
**节点不一样：`client_agents` 是 MySQL 表，停用一台机器必须跨 server 重启、跨 hank-cli 重启保持。**
所以本任务的 3 个状态都是 `client_agents` 的新列，不要放内存。

特别注意：hank-cli 每次启动都会 `PUT /api/client/registration` 走
`upsert_client_agent` 的 `ON DUPLICATE KEY UPDATE`。
**这条 UPDATE 绝对不能带上 `enabled`**，否则被停用的节点一重启就自己启用回来了。

### 停用的语义（已确认，务必严格按此实现）

停用 = **只屏蔽"自动选路"，不屏蔽"人工显式指定"**：

- 被停用的节点不再出现在自动挑选结果里：`pick_online_client`、
  `pick_online_agent_client` 必须跳过它。这是本任务的核心效果。
- admin 终端代理（`server/src/admin_terminal.rs` 的 `dispatch`）**不受影响**：
  停用的节点仍可在 admin 页查看终端列表、读输出、发命令。
  理由与会话层一致——停用不能让人失去观察和补救能力。
- 已经绑定了 `exec_client_id` 的历史会话（`server/src/remote_tools.rs`）**不受影响**，
  `dispatch_tool_call` 本身不加守卫。
- `client_reports_backend`（按 client_id 校验某节点是否具备某 Agent CLI）**不受影响**，
  它拿到的 client_id 都是上游已经选定或人工指定的，不重复加闸。
- 停用不影响在线判定：节点照常 poll，绿点照常亮，`online` 字段照常为 true。
  「停用」与「离线」是两个独立维度，前端要能同时表达。

### 两个时间的定义

| 字段 | 含义 | 何时刷新 |
|------|------|----------|
| `last_active_at` | 最后运行：最近一次被派发任务 | `dispatch_tool_call`、`start_agent_run` 入队成功时 |
| `last_seen_at` | 最后在线：最近一次被观测到活着 | `poll_requests` 每次进入、`register_client` 注册时 |

两者互不联动：派发成功不代表节点在线（可能正好掉线），poll 也不代表它在干活。

### 做完之后的可观察效果

- 左侧每个 client 右侧多一个开关，点击即停用/启用，5s 轮询后状态保持，server 重启后仍保持。
- 停用的 client 标灰并标注「已停用」，但仍可点选、仍能看它的终端会话与输出。
- 每个 client 下方展示「最后运行」与「最后在线」两个相对时间（如 `3分钟前`）。
- 停用某节点后，微信/飞书发起的新任务不会再落到它头上（多台在线时可实测）。

## 涉及文件清单

| 文件 | 改什么 |
|------|--------|
| `crates/hank-db/src/lib.rs` | `client_agents` 建表语句加 3 列 + `ensure_column` 迁移；`ClientAgent` 结构体加 3 字段；4 处 SELECT 补列；`upsert_client_agent` 刷新 `last_seen_at` 但不动 `enabled`；新增 `set_client_agent_enabled` / `touch_client_agent_active` / `touch_client_agent_seen` |
| `server/src/remote_exec.rs` | `dispatch_tool_call`/`start_agent_run` 刷新 last_active_at；`poll_requests`/`register_client` 刷新 last_seen_at；`pick_online_client`/`pick_online_agent_client` 过滤 `enabled`；`list_online` 输出新字段；补单测 |
| `server/src/admin_terminal.rs` | `list_clients` 输出新字段；新增 `set_client_enabled` handler |
| `server/src/main.rs` | 注册新路由 `POST /api/admin/clients/{cid}/enabled` |
| `admin/src/composables/api.ts` | `ClientAgentInfo` 补 3 个字段；新增 `clientSetEnabled` 方法 |
| `admin/src/views/Terminals.vue` | client 列表加开关与两个时间展示 |

## 实现步骤

### 1. crates/hank-db/src/lib.rs — schema 与 CRUD

- [ ] `client_agents` 建表语句（约 1161 行）在 `accept_remote` 之后加 3 列：

```sql
enabled BOOLEAN NOT NULL DEFAULT TRUE,
last_active_at DATETIME DEFAULT NULL,
last_seen_at DATETIME DEFAULT NULL,
```

  两个时间列可空：DEFAULT NULL 表示"从未发生过"，前端展示 `—`，
  不要用 `NOW()` 兜底，否则一台从没干过活的节点会显示成刚刚运行过。

- [ ] 在 "Migrations for existing databases" 段落（约 1194 行起）追加 3 条 `ensure_column`
      给旧库补列，沿用 `sessions.exec_client_id` 那处的写法：

```rust
Self::ensure_column(
    &pool,
    "client_agents",
    "enabled",
    "ALTER TABLE client_agents ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT TRUE AFTER accept_remote",
)
.await?;
```

  另两列同理（`last_active_at`、`last_seen_at`，都是 `DATETIME DEFAULT NULL`）。
  用 `ensure_column` 而不是裸 `let _ = ALTER`，与 `exec_client_id` 保持一致。

- [ ] `ClientAgent` 结构体（约 507 行）加 3 个字段，顺序与 SELECT 列表一致：

```rust
pub enabled: bool,
pub last_active_at: Option<DateTime<Utc>>,
pub last_seen_at: Option<DateTime<Utc>>,
```

- [ ] **4 处 SELECT 都要补列**（`sqlx::query_as` 靠列顺序映射，漏一处就是运行时错误）：
      `get_client_agent`、`list_client_agents`、`list_all_client_agents`、`get_client_agent_by_id`。
      统一改成：

```sql
SELECT id, user_id, hostname, work_dir, accept_remote, enabled, last_active_at, last_seen_at, created_at, updated_at FROM client_agents ...
```

- [ ] `upsert_client_agent`：INSERT 的列与 UPDATE 子句都要**顺带把 `last_seen_at` 刷成 NOW()**
      （注册即一次观测），**但 UPDATE 子句里绝对不要出现 `enabled`**：

```rust
"INSERT INTO client_agents (id, user_id, hostname, work_dir, accept_remote, last_seen_at) VALUES (?, ?, ?, ?, ?, NOW())
 ON DUPLICATE KEY UPDATE user_id = VALUES(user_id), hostname = VALUES(hostname), work_dir = VALUES(work_dir), accept_remote = VALUES(accept_remote), last_seen_at = NOW(), updated_at = NOW()"
```

  新插入的行 `enabled` 走列默认值 TRUE；已存在的行保持人工设置的值不被覆盖。

- [ ] 新增 3 个方法，放在 `get_client_agent_by_id` 之后，都用 `db_retry!` 包裹：

```rust
/// 停用/启用节点；停用后不再被 pick_online_* 自动选中
pub async fn set_client_agent_enabled(&self, client_id: &str, enabled: bool) -> Result<()>

/// 刷新最后运行时间（被派发任务时调用）
pub async fn touch_client_agent_active(&self, client_id: &str) -> Result<()>

/// 刷新最后在线时间（poll / 注册时调用）
pub async fn touch_client_agent_seen(&self, client_id: &str) -> Result<()>
```

  三者都是单条 UPDATE（`SET enabled = ?` / `SET last_active_at = NOW()` / `SET last_seen_at = NOW()`），
  `WHERE id = ?`。`set_client_agent_enabled` 顺带 `updated_at = NOW()`；
  两个 touch **不要**改 `updated_at`（`updated_at` 语义是注册信息变更，别被高频心跳污染）。

### 2. server/src/remote_exec.rs — 时间刷新与自动选路闸门

- [ ] `pick_online_client`（约 231 行）：过滤条件从 `.filter(|c| c.accept_remote)`
      改为 `.filter(|c| c.accept_remote && c.enabled)`。

- [ ] `pick_online_agent_client`（约 249 行）：同样在
      `.filter(|client| client.accept_remote && client.work_dir.is_some())`
      里加上 `&& client.enabled`。

- [ ] `dispatch_tool_call`（约 110 行）：在 pending 入队之后（释放 `client_hubs` 写锁之后、
      等待结果之前）刷新最后运行时间。**注意不要在持有 `state.client_hubs.write()` 的
      作用域里 await 数据库**，会拖长锁持有时间：

```rust
    // 入队即视为一次派发；DB 失败不影响本次调用
    let _ = state.db.touch_client_agent_active(client_id).await;

    let result = tokio::time::timeout(timeout, rx).await;
```

  `let _ =` 忽略错误是有意的：时间戳只是观测数据，不该让一次 DB 抖动打断真实任务。

- [ ] `start_agent_run`（约 154 行）：同样在入队成功后 `touch_client_agent_active`。
      如果它内部就是复用 `dispatch_tool_call`，则不要重复刷新——先读代码确认，
      只在真正独立入队的路径上加。

- [ ] `poll_requests`（约 403 行）：**在进入 loop 之前**刷新一次 `last_seen_at`
      （放在 `client_id.trim().is_empty()` 校验之后）。
      不要放在 loop 内部——loop 每次被 notify 唤醒都会转一圈，会造成无意义的 DB 写。
      25s 一次的粒度对"最后在线"足够。

```rust
    // 长轮询进入即视为一次在线观测；25s 粒度足够，不必在 loop 内重复写
    let _ = state.db.touch_client_agent_seen(&client_id).await;
```

- [ ] `register_client`（约 300 行）：`upsert_client_agent` 已在 SQL 里刷了 `last_seen_at`，
      **这里不要再额外调 touch**，避免一次注册两次写库。

- [ ] `list_online`（约 508 行）的 json 里补上 3 个字段，与 `admin_terminal::list_clients` 保持一致：

```rust
"enabled": c.enabled,
"last_active_at": c.last_active_at,
"last_seen_at": c.last_seen_at,
```

- [ ] 单测（追加到文件末尾的 `mod tests`，只测纯函数，不碰 DB）：
      现有测试里的 `ClientAgent` 字面量若因新增字段编译失败，补齐字段即可。
      新增 2 条：
  - `disabled_client_is_skipped_by_pick`：构造 `enabled = false` 的 ClientAgent，
    断言过滤谓词 `c.accept_remote && c.enabled` 为 false。
    若现有代码里过滤谓词是内联闭包不便单测，就抽一个私有函数
    `fn is_dispatchable(c: &ClientAgent) -> bool { c.accept_remote && c.enabled }`
    供两个 pick 函数与测试共用。
  - `enabled_client_passes_pick`：`accept_remote = true, enabled = true` 时为 true；
    并断言 `accept_remote = false, enabled = true` 仍为 false（两个条件是且关系）。

### 3. server/src/admin_terminal.rs — 节点开关 API

- [ ] `list_clients`（19 行）的 json 里补 3 个字段：

```rust
"enabled": a.enabled,
"last_active_at": a.last_active_at,
"last_seen_at": a.last_seen_at,
```

- [ ] 新增 handler，放在 `list_clients` 之后（注意它操作 DB 而非转发 client，
      所以**不要**走本文件的 `dispatch`）：

```rust
#[derive(Deserialize)]
pub struct ClientEnabledBody {
    enabled: bool,
}

/// POST /api/admin/clients/{cid}/enabled — 停用/启用 hank-cli 节点。
/// 停用只影响自动选路（pick_online_*），admin 侧终端代理仍可用。
pub async fn set_client_enabled(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
    Json(body): Json<ClientEnabledBody>,
) -> impl IntoResponse {
    // 先确认存在，避免对不存在的 id 静默返回成功
    match state.db.get_client_agent_by_id(&cid).await {
        Ok(Some(_)) => {}
        Ok(None) => return R::not_found("client not found"),
        Err(e) => return R::internal_error(e),
    }
    match state.db.set_client_agent_enabled(&cid, body.enabled).await {
        Ok(()) => R::ok(serde_json::json!({ "id": cid, "enabled": body.enabled })),
        Err(e) => R::internal_error(e),
    }
}
```

### 4. server/src/main.rs — 路由

- [ ] 在 "Admin terminal proxy" 段落里，`/api/admin/clients` 那条之后插入
      （必须排在 `/api/admin/clients/{cid}/terminals` 之前后都无妨，axum 0.8 路径不冲突）：

```rust
.route(
    "/api/admin/clients/{cid}/enabled",
    post(admin_terminal::set_client_enabled),
)
```

### 5. admin/src/composables/api.ts — 类型与方法

- [ ] `ClientAgentInfo`（约 659 行）补 3 个字段：

```ts
enabled: boolean
last_active_at: string | null
last_seen_at: string | null
```

  两个时间用 `string | null`（后端可空），与 `hostname: string | null` 的写法一致。

- [ ] 在 `listClients` 之后新增方法：

```ts
clientSetEnabled(clientId: string, enabled: boolean) {
  return request<{ id: string; enabled: boolean }>(
    `/api/admin/clients/${clientId}/enabled`,
    { method: 'POST', body: JSON.stringify({ enabled }) }
  )
},
```

### 6. admin/src/views/Terminals.vue — 列表展示与开关

- [ ] 新增 `toggleClientEnabled`，紧挨现有的 `toggleEnabled`（终端会话用）放置，
      注释区分两者，避免以后混淆：

```ts
/** 停用/启用整个 hank-cli 节点；停用后新任务不再自动落到它头上 */
async function toggleClientEnabled(c: ClientAgentInfo) {
  try {
    await api.clientSetEnabled(c.id, !c.enabled)
    await loadClients()
  } catch (e: any) {
    error.value = e.message
  }
}
```

- [ ] client 列表项（模板约 218–230 行）改造。当前是单行 flex，需要改成两行结构
      （第一行：在线点 + 主机名 + 已停用标签 + 开关；第二行：两个时间），
      整体照抄下方终端会话项的排版手法，保持视觉一致：

```vue
<div
  v-for="c in clients"
  :key="c.id"
  class="px-2 py-1.5 rounded-md cursor-pointer text-[13px] transition-colors"
  :class="[
    c.id === selectedClientId ? 'bg-surface-raised text-text-primary' : 'text-text-secondary hover:bg-surface-raised/50',
    c.enabled === false ? 'opacity-60' : '',
  ]"
  @click="selectedClientId = c.id"
>
  <div class="flex items-center gap-2">
    <span
      class="w-1.5 h-1.5 rounded-full shrink-0"
      :class="c.online ? 'bg-green-400' : 'bg-text-tertiary'"
    ></span>
    <span class="truncate">{{ c.hostname || shortId(c.id) }}</span>
    <span
      v-if="c.enabled === false"
      class="text-[10px] text-text-tertiary shrink-0 px-1 rounded bg-surface-raised"
    >已停用</span>
    <button
      class="flex items-center shrink-0 ml-auto"
      title="停用/启用节点"
      @click.stop="toggleClientEnabled(c)"
    >
      <!-- 开关样式与终端会话项、FeishuBot/Jobs 页保持一致 -->
      <span
        class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors"
        :class="c.enabled !== false ? 'bg-green-500' : 'bg-border-subtle'"
      >
        <span
          class="inline-block h-3 w-3 rounded-full bg-white transition-transform"
          :class="c.enabled !== false ? 'translate-x-3.5' : 'translate-x-0.5'"
        ></span>
      </span>
    </button>
  </div>
  <div class="text-[11px] text-text-tertiary truncate pl-3.5">
    最后运行 {{ relTime(c.last_active_at) }} · 最后在线 {{ relTime(c.last_seen_at) }}
  </div>
</div>
```

  注意 `@click.stop` 必不可少，否则点开关会连带切换选中的 client。
  `relTime` 已存在于本文件（28 行）且已处理 null，直接复用，不要新写一个。

- [ ] 侧栏容器宽度是 `grid-cols-[220px_1fr]`（213 行）。加了开关和时间行后 220px 偏窄，
      改为 `grid-cols-[260px_1fr]`。这是本任务允许的唯一布局调整，其余不要动。

## 明确边界

- **不要碰**微信 / 飞书 / scheduler / cli_agent 任何业务逻辑：
  `server/src/weixin/`、`server/src/feishu/`、`server/src/scheduler/`、`server/src/cli_agent.rs`
  全都不改。它们通过 `pick_online_client` / `pick_online_agent_client` 自动获得停用语义。
- **不要碰** `cli/`：hank-cli 侧不需要任何改动（节点不需要知道自己被停用了，
  它照常 poll，只是 server 不再给它派新活）。
- **不要碰** `server/src/remote_tools.rs`、`server/src/snap_tools.rs`：
  它们拿到的 client_id 是上游已选定的，按语义不加闸门。
- **不要给 `dispatch_tool_call` 加 enabled 守卫**——那会连带掐死 admin 终端代理，
  违反"停用不影响观察"的语义。闸门只加在两个 `pick_online_*` 上。
- 不要动 `is_client_online` 的语义，不要把 enabled 混进在线判定。
- 不要新增第三方依赖。
- 工作区里有与本任务无关的既有改动（`git status` 可见），
  **只增量修改本文档列出的文件，禁止 `git checkout` / `git stash` / `git restore`
  等任何回退操作，也不要提交、不要建分支。**
- 不要新建文档文件；不要写 README 或额外说明。

## 验收标准

```bash
# 1. server + 各 crate（含 hank-db）
cargo build -p server
cargo test -p server
cargo build -p hank-db

# 2. admin 前端（含 vue-tsc 类型检查）
cd admin && pnpm build
```

期望结果：

- 全部命令成功；`cargo test -p server` 中新增的 2 条 pick 相关测试通过，既有测试不回归。
- `cargo build -p server` 无新增 warning（新 handler 已被路由引用，新 DB 方法均有调用点）。
- `pnpm build` 的 `vue-tsc` 阶段无类型错误（`ClientAgentInfo` 新字段与模板用法一致）。
- **重点自查**：4 处 `SELECT ... FROM client_agents` 的列顺序与 `ClientAgent`
  字段顺序严格一致（sqlx 按位置映射，顺序错了编译能过、运行时报错或错位）。
- **重点自查**：`upsert_client_agent` 的 `ON DUPLICATE KEY UPDATE` 子句里没有 `enabled`。
- 若有条件手动验证：启动 server + hank-cli，打开 admin 终端页，
  停用某节点后它仍可查看终端输出；重启 server 后停用状态仍在；
  两个时间随 poll 与任务派发更新。

## 约定

- 遵循 `CLAUDE.md`：**中文注释、中文 commit message**；
  server 错误处理沿用 `response::{self as R}` 辅助，DB 层沿用 `db_retry!` + `anyhow::Result`。
- 前端 `<script setup lang="ts">` + Composition API，样式一律 Tailwind utility class，
  颜色只用项目既有语义变量（`text-text-primary` / `text-text-secondary` /
  `text-text-tertiary` / `bg-surface-raised` / `border-border-subtle` 等），不要写死 hex。
- 注释只写"为什么"，不要把代码翻译一遍；新增注释密度与周边保持一致。
- Rust 代码提交前按 `cargo fmt` 格式化（项目刚做过全量 fmt 统一，不要引入格式差异）。
- commit message 建议：`feat(client): hank-cli 节点支持停用/启用与活跃时间`
  （是否提交由人工决定，你不要执行 git commit）。

