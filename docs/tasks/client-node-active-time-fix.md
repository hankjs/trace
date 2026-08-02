# 任务：修正「最后运行时间」被 admin 轮询污染

## 背景与目标

前一个任务（`docs/tasks/client-node-enable-and-activity.md`，已实现，尚未提交）给
`client_agents` 加了 `last_active_at`（最后运行时间），刷新点放在
`server/src/remote_exec.rs` 的 `dispatch_tool_call` 里——**每次派发任何工具都刷新**。

这是个错误。`server/src/admin_terminal.rs` 的 `dispatch` 也走 `dispatch_tool_call`，
而 admin 终端页（`admin/src/views/Terminals.vue`）在自动刷新开启时会：

- 每 5s 调一次 `terminal_list`（`loadTerminals`）
- 每 3s 调一次 `terminal_read`（`loadOutput`）

结果：**只要终端页开着，被选中节点的「最后运行」永远显示「刚刚」**，
而这个字段恰好就展示在同一个页面上——纯观察行为把它要观察的指标刷成了常量，字段失去意义。

### 目标

「最后运行」只反映**真实派发**，不被纯观察类轮询污染。
做完之后：打开终端页只看不发命令，「最后运行」应保持它原本的时间并随时间推移变成
`3分钟前`、`1小时前`；一旦通过 admin 发送命令、或微信/飞书起任务，才跳回「刚刚」。

「最后在线」不受本次修正影响（它由 `poll_requests` 刷新，本来就该随心跳更新）。

## 涉及文件清单

| 文件 | 改什么 |
|------|--------|
| `server/src/remote_exec.rs` | 新增观察类工具白名单与判定函数；`dispatch_tool_call` 按工具名决定是否刷新 `last_active_at`；补单测 |

只改这一个文件。DB 层、admin 侧、前端都不动。

## 实现步骤

### server/src/remote_exec.rs

- [ ] 在文件顶部的常量区（`NETWORK_MARGIN` 附近）新增：

```rust
/// 纯观察类工具：admin 终端页每 3~5s 轮询一次，若计入「最后运行」
/// 会把该字段永久钉在「刚刚」，使它失去意义。
const OBSERVE_ONLY_TOOLS: [&str; 2] = ["terminal_list", "terminal_read"];
```

- [ ] 紧邻常量新增判定函数（独立函数便于单测）：

```rust
/// 该工具调用是否算作一次「真实派发」（用于刷新 last_active_at）
fn counts_as_dispatch(tool: &str) -> bool {
    !OBSERVE_ONLY_TOOLS.contains(&tool)
}
```

- [ ] `dispatch_tool_call` 里现有的这段（约 133 行）：

```rust
    // 入队即视为一次派发；DB 失败不影响本次调用
    let _ = state.db.touch_client_agent_active(client_id).await;
```

  改为按工具名判定：

```rust
    // 入队即视为一次派发；DB 失败不影响本次调用。
    // 观察类工具（admin 页高频轮询）不计入，否则「最后运行」永远是「刚刚」。
    if counts_as_dispatch(tool) {
        let _ = state.db.touch_client_agent_active(client_id).await;
    }
```

  注意 `tool` 参数在此处已被 move 进 `ToolCallRequest`（`tool: tool.to_string()`），
  但那是在前面的作用域里对 `&str` 做了 `to_string()`，`tool: &str` 本身仍可用，
  编译若报错请就地确认，不要为此改函数签名。

- [ ] `start_agent_run` 里的 `touch_client_agent_active` **保持不变**：
      它是真实的长时任务下发，必须计入。

- [ ] 单测（追加到 `mod tests`，紧挨上一个任务加的
      `disabled_client_is_skipped_by_pick` / `enabled_client_passes_pick`）：

```rust
#[test]
fn observation_tools_do_not_count_as_dispatch() {
    // admin 终端页 3~5s 一次的轮询不应刷新「最后运行」
    assert!(!counts_as_dispatch("terminal_list"));
    assert!(!counts_as_dispatch("terminal_read"));
}

#[test]
fn real_tool_calls_count_as_dispatch() {
    assert!(counts_as_dispatch("terminal_write"));
    assert!(counts_as_dispatch("terminal_create"));
    assert!(counts_as_dispatch("shell"));
}
```

## 明确边界

- 只改 `server/src/remote_exec.rs`。不要动 `admin_terminal.rs`
  （不要在那里绕过 `dispatch_tool_call`，闸门放在一处更不容易漏）。
- 不要动 `crates/hank-db/src/lib.rs`、`admin/` 下任何文件。
- 不要把 `terminal_set_enabled`、`terminal_write`、`terminal_create`、`terminal_close`
  加进白名单：它们都是人工显式动作，频率低，计入「最后运行」是合理的。
- 不要改 `poll_requests` 里 `touch_client_agent_seen` 的逻辑。
- 工作区里有与本任务无关的既有改动（`git status` 可见），
  **禁止 `git checkout` / `git stash` / `git restore` 等任何回退操作，
  也不要提交、不要建分支。**

## 验收标准

```bash
# 注意：workspace 里 server 的包名是 hank-server，不是 server
cargo build -p hank-server
cargo test -p hank-server
cargo fmt --check
```

期望结果：

- 全部命令成功；新增 2 条测试通过，既有 107 条测试不回归。
- `cargo fmt --check` 无输出（项目已做过全量 fmt 统一）。
- `cargo build` 不产生新 warning（`OBSERVE_ONLY_TOOLS` 与 `counts_as_dispatch` 均有调用点）。
- 若有条件手动验证：打开 admin 终端页选中一个节点，只看不发命令，
  等 2 分钟后「最后运行」应显示为 `2分钟前` 而非 `刚刚`；
  发一条命令后应立刻变回 `刚刚`。

## 约定

- 遵循 `CLAUDE.md`：中文注释、中文 commit message。
- 注释只写"为什么"，不要把代码翻译一遍。
- commit message 建议（与上一个任务合并提交也可，由人工决定）：
  `fix(client): 「最后运行」不再被 admin 终端轮询刷新`
  （你不要执行 git commit）。
