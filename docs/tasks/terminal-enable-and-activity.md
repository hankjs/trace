# 任务：终端会话支持停用/启用 + 记录最后工作时间与最后在线时间

## 背景与目标

admin 的「终端」页（`admin/src/views/Terminals.vue`）左侧展示某个 hank-cli 节点上的终端会话列表，
目前每个会话只能看到前台进程名、cwd、alive 与短 id。缺两件事：

1. **停用/启用**：某个终端暂时不想让它被外部驱动（微信/飞书/admin 都会往里写命令），
   希望能一键停用；停用后仍能查看输出、随时启用恢复。
2. **活跃时间**：看不出一个终端"上次干活是什么时候""什么时候还活着"，
   一排终端摆在那里无法判断哪个是遗留的空壳。

本任务的对象是**终端会话层**（右侧「终端会话」列表里的每一条），不是 client 节点层。
终端会话存活在 hank-cli 进程内存里（`cli/src/terminal.rs` 的 `TermManager.sessions`），
server 无持久化记录，因此 enabled 与两个时间戳同样存在 cli 内存中：
**hank-cli 重启后连会话本身都不存在了，状态一并丢失，这是符合预期的，不要引入 DB 表。**

### 停用的语义（已确认，务必严格按此实现）

停用 = **只屏蔽对外交互，不杀进程、不影响观察**：

- `terminal_write`（写入命令）被拒绝，返回错误；PTY 子进程继续运行，不发信号、不 kill。
- 该会话的终端通知（OSC 9/777/133/BEL）不再上报 server —— 停用的终端不该继续往微信/飞书推响铃。
- `terminal_read`（读输出）、`terminal_list`（列表）、`terminal_close`（关闭）**不受停用影响**，
  停用的会话必须仍出现在 `terminal_list` 里，否则前端无法把它启用回来。
- scrollback 仍然照常累积，「最后工作时间」仍然照常更新。

### 做完之后的可观察效果

- 终端页每条会话右侧多一个开关，点击即停用/启用，列表 5s 轮询后状态保持。
- 停用的会话在列表里显式标灰并标注「已停用」；选中它时输入框与「发送」按钮禁用。
- 每条会话展示「最后工作」与「最后在线」两个相对时间（如 `3分钟前`）。
- 对停用的终端调用写入（admin 发送、微信 `t <前缀> <命令>`）会收到明确的错误文案，而不是静默无效。

## 涉及文件清单

| 文件 | 改什么 |
|------|--------|
| `cli/src/terminal.rs` | `TermSession`/`TermInfo` 加字段；新增 `term_set_enabled`；`term_write` 加停用守卫；reader 线程更新工作时间并在停用时不发通知；`session_info` 刷新在线时间；补单测 |
| `cli/src/worker.rs` | dispatch 新增 `terminal_set_enabled` 分支；补单测 |
| `server/src/admin_terminal.rs` | 新增 `terminal_set_enabled` handler（透明转发到 client） |
| `server/src/main.rs` | 注册新路由 `POST /api/admin/clients/{cid}/terminals/{tid}/enabled` |
| `admin/src/composables/api.ts` | `TermInfo` 补 3 个字段；新增 `terminalSetEnabled` 方法 |
| `admin/src/views/Terminals.vue` | 列表加开关与时间展示；停用时禁用输入区 |

## 实现步骤

### 1. cli/src/terminal.rs — 会话状态与时间戳

- [ ] `TermSession` 新增 3 个字段（都要能被 reader 线程共享，故用 `Arc`）：

```rust
pub struct TermSession {
    // ...existing fields...
    /// 是否启用；停用后拒绝写入、不再上报通知（不杀进程）
    pub enabled: Arc<AtomicBool>,
    /// 最后工作时间：最近一次有 PTY 输出或写入的时刻
    pub last_active_at: Arc<Mutex<DateTime<Utc>>>,
    /// 最后在线时间：最近一次被观测到 alive 的时刻（term_list 时刷新）
    pub last_seen_at: Arc<Mutex<DateTime<Utc>>>,
}
```

需要 `use chrono::{DateTime, Utc};`（chrono 已是 cli 依赖）。三者在 `term_create` 里初始化：
`enabled = Arc::new(AtomicBool::new(true))`，两个时间戳初值都用会话创建时刻（与 `created_at` 同一时刻）。
注意现有 `created_at` 是 `String`（RFC3339），保持原样不要改类型，只是取同一个 `Utc::now()` 来源即可。

- [ ] `TermInfo` 新增对应的 3 个可序列化字段（时间用 RFC3339 字符串，与 `created_at` 风格一致）：

```rust
pub struct TermInfo {
    // ...existing fields...
    pub enabled: bool,
    pub last_active_at: String,
    pub last_seen_at: String,
}
```

- [ ] `session_info(s: &TermSession) -> TermInfo`：填充新字段；**并在 alive 时把 `last_seen_at` 刷成当前时刻**
      （`term_list` 每次被调用即视为一次观测；子进程已退出时不刷新，时间冻结在死亡前最后一次观测）。
      注意 `session_info` 拿的是 `&TermSession`，靠 `Mutex`/`AtomicBool` 的内部可变性写入，不要改成 `&mut`。

- [ ] `term_create` 内的 reader 线程：每次成功读到 `n > 0` 字节时，
      除现有的 `append_scrollback` 之外，把 `last_active_at` 更新为当前时刻；
      随后的通知派发（`scanner.feed(...)` 产出的事件）**仅在 `enabled` 为 true 时** `notify_tx.send(...)`。
      停用时仍要照常 `scanner.feed`（保持状态机连续，避免启用后错位），只是不发送。
      为此需要把 `enabled` 与 `last_active_at` 的 `Arc` clone 进线程闭包。

- [ ] `term_write`：在取到 session 之后、写入之前加守卫：

```rust
if !session.enabled.load(Ordering::SeqCst) {
    return Err("terminal disabled".into());
}
```

  写入成功后把 `last_active_at` 更新为当前时刻。

- [ ] 新增方法（找不到会话时报错文案与既有 `term_write`/`term_read` 保持一致，用 `"terminal not found"`）：

```rust
/// 停用/启用会话；返回更新后的会话信息
pub fn term_set_enabled(&self, id: &str, enabled: bool) -> Result<TermInfo, String>
```

- [ ] `term_close` 不加守卫（停用的终端仍可关闭）。

- [ ] 补单测（在现有 `mod tests` 内，沿用 `test_manager` / `notify_tx` / `wait_output` 辅助函数）：
  - `set_enabled_blocks_write_but_keeps_read`：创建 → 写入并等到输出 → 停用 →
    `term_write` 返回 `Err`；`term_read` 仍返回之前的输出；`term_list` 里仍能找到该会话且 `enabled == false`
    → 启用 → 写入恢复成功。
  - `last_active_at_advances_on_write`：记下创建后的 `last_active_at`，写一条 `echo` 并等到输出，
    再取 `term_list`，断言 `last_active_at` 变新（解析成 `DateTime<Utc>` 比较，别比字符串字典序）。
  - `term_set_enabled_unknown_id_errors`：不存在的 id 返回 `Err`。
  - 测试结束记得 `term_close` 收尾，与既有测试风格一致。

### 2. cli/src/worker.rs — 协议分支

- [ ] 在 `execute_tool` 的 `match req.tool.as_str()` 中，紧跟 `terminal_close` 之后加：

```rust
"terminal_set_enabled" => {
    let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = input.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    match term.term_set_enabled(id, enabled) {
        Ok(info) => ToolOutput::ok(serde_json::to_string(&info).unwrap_or_default()),
        Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
    }
}
```

  与相邻分支的错误前缀 `Remote exec error:` 保持一致。

- [ ] 在 `mod tests` 里补一条 dispatch 层单测：`terminal_set_enabled` 返回的 JSON 中 `enabled == false`，
      随后对同一 id 的 `terminal_write` 走到 `is_error == true`。

### 3. server/src/admin_terminal.rs — 转发端点

- [ ] 新增请求体与 handler，风格照 `terminal_input` 抄（复用同一个 `dispatch` 辅助函数）：

```rust
#[derive(Deserialize)]
pub struct EnabledBody {
    enabled: bool,
}

/// POST /api/admin/clients/{cid}/terminals/{tid}/enabled — 停用/启用终端会话
pub async fn terminal_set_enabled(
    State(state): State<Arc<AppState>>,
    Path((cid, tid)): Path<(String, String)>,
    Json(body): Json<EnabledBody>,
) -> impl IntoResponse
```

  成功时把 client 返回的 `TermInfo` JSON 解析后原样返回（解析失败则回 `{"enabled": <bool>}` 兜底），
  失败时返回 `dispatch` 给出的错误响应。给 handler 写一行中文文档注释。

### 4. server/src/main.rs — 路由注册

- [ ] 在 `// Admin terminal proxy` 区块内、`.../input` 那条之后追加：

```rust
.route(
    "/api/admin/clients/{cid}/terminals/{tid}/enabled",
    post(admin_terminal::terminal_set_enabled),
)
```

  该区块已在 `admin_required` + `auth_middleware` 之下，无需额外中间件。

### 5. admin/src/composables/api.ts

- [ ] `TermInfo` 接口补字段：

```ts
export interface TermInfo {
  // ...existing...
  enabled: boolean
  last_active_at: string
  last_seen_at: string
}
```

- [ ] 在 `terminalInput` 附近新增（保持既有 `request<...>` 写法）：

```ts
terminalSetEnabled(clientId: string, termId: string, enabled: boolean) {
  return request<TermInfo>(
    `/api/admin/clients/${clientId}/terminals/${termId}/enabled`,
    { method: 'POST', body: JSON.stringify({ enabled }) }
  )
},
```

### 6. admin/src/views/Terminals.vue

- [ ] 新增 `toggleEnabled(t: TermInfo)`：调 `api.terminalSetEnabled(selectedClientId, t.id, !t.enabled)`，
      成功后 `await loadTerminals()` 刷新，失败把消息写进现有的 `error`。
      点击开关时要 `@click.stop`，避免冒泡触发外层的选中终端逻辑。

- [ ] 新增相对时间格式化函数（本文件内即可，不用建公共工具）：

```ts
/** 相对时间：刚刚 / N分钟前 / N小时前 / N天前；无值或非法返回 '—' */
function relTime(value: string | null | undefined): string
```

- [ ] 终端列表项模板改造（沿用现有 Tailwind 变量与 `text-[11px]/text-[12px]` 字号档位，不要引入新色板）：
  - 首行末尾（短 id 之前或之后）放一个小开关，样式抄 `admin/src/views/Jobs.vue` 里 `toggleEnabled` 那颗：
    `h-4 w-7` 轨道 + `h-3 w-3` 圆点，开启 `bg-green-500`、关闭 `bg-border-subtle`。
  - 停用的会话整项降透明（如 `opacity-60`），并在前台进程名后面加一个 `已停用` 的小标签。
  - cwd 那行下面再加一行时间：`最后工作 {{ relTime(t.last_active_at) }} · 最后在线 {{ relTime(t.last_seen_at) }}`。
  - 现有的 alive 圆点、`shortId`、`homeCwd` 保持不动。

- [ ] 右侧输入区：当选中的会话 `enabled === false` 时，禁用输入框与「发送」按钮
      （`:disabled` + `disabled:opacity-50 disabled:cursor-not-allowed`），并在上方给一行提示
      「该终端已停用，启用后可发送命令」。「刷新」按钮保持可用（读取不受停用影响）。
      建议加一个 `const selectedTerm = computed(() => terminals.value.find(t => t.id === selectedTermId.value))` 供模板使用。

## 明确边界

- **不许碰**：
  - `crates/hank-db/`（本任务不加表、不加列、不做迁移）
  - `server/src/weixin/`、`server/src/feishu/`、`server/src/snap_tools.rs`、`server/src/remote_tools.rs`、`server/src/cli_agent.rs`
    —— 它们调 `terminal_write` 时会自然收到停用错误，这就是预期行为，不要为它们加特判
  - `server/src/remote_exec.rs`（client 节点层的在线判定与派发闸门本任务不动）
  - `client/`（Tauri 前端）与 `quant/`
- `server/src/main.rs` 只允许加上面那一条路由，不要顺手动其他路由或中间件。
- 工作区当前有大量与本任务无关的未提交改动（`git status` 可见几十个文件已修改）。
  **只增量修改本文档列出的文件，禁止 `git checkout` / `git stash` / `git restore` 等任何回退操作，
  也不要提交、不要建分支。**
- 不要新增第三方依赖（cli 与 admin 的依赖清单都不动）。
- 不要新建文档文件；本任务不需要写 README 或额外说明。

## 验收标准

```bash
# 1. cli（独立 Cargo 项目，不在 workspace 内）
cd cli && cargo build && cargo test

# 2. server + 各 crate
cargo build -p server

# 3. admin 前端（含 vue-tsc 类型检查）
cd admin && pnpm build
```

期望结果：

- 三条命令全部成功；`cd cli && cargo test` 中新增的 4 条终端相关测试通过，既有测试不回归。
- `cargo build -p server` 无 warning 级别的新增未使用项（新 handler 已被路由引用）。
- `pnpm build` 的 `vue-tsc` 阶段无类型错误（`TermInfo` 新字段与模板用法一致）。
- 若有条件手动验证：启动 server + hank-cli，打开 admin 终端页，
  停用某个终端后发送命令应被拒绝且有错误提示，启用后恢复；两个时间随活动更新。

## 约定

- 遵循 `CLAUDE.md`：**中文注释、中文 commit message**；Rust 错误处理沿用各文件既有风格
  （cli 用 `Result<_, String>`，server 用 `response::{self as R}` 的响应辅助）。
- 前端 `<script setup lang="ts">` + Composition API，样式一律 Tailwind utility class，
  颜色只用项目既有语义变量（`text-text-primary` / `text-text-secondary` / `text-text-tertiary` /
  `bg-surface-raised` / `border-border-subtle` 等），不要写死 hex。
- 注释只写"为什么"，不要把代码翻译一遍；新增注释密度与周边保持一致。
- commit message 建议：`feat(terminal): 终端会话支持停用/启用与活跃时间`（是否提交由人工决定，你不要执行 git commit）。
