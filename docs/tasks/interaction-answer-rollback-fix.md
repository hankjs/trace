# 修正：确认被消耗但任务未派发时，交互单卡死在 answered

## 一、背景

任务 A（`docs/tasks/agent-interactions-table-and-cards.md`）已实现，尚未提交。
编译、fmt、clippy（58 条，低于基线 60）、测试全部通过，时序设计正确。

review 发现一个**后果不可恢复**的缺陷：用户点了确认，交互单被标记为已应答，
但任务从未派发，且**无法重试**。

## 二、缺陷

### 链路

`server/src/feishu/callback.rs` 的按钮回调顺序是：

1. `card_action_claim`（`insert_channel_message` 幂等去重）
2. `answer_interaction` → 状态 `pending` → **`answered`**
3. 改卡片为终态「已选择 确认」
4. `tokio::spawn` → `resume_interaction_on_session` → 跑 `run_chat_turn`

第 4 步的 `resume_interaction_on_session`（`callback.rs:311` 附近）开头是：

```rust
let dispatch_guard = state.tasks.try_acquire(session_id).await;
let Some(dispatch_guard) = dispatch_guard else {
    api.reply_text(message_id, "任务仍在处理上一次确认，请稍候", in_thread).await?;
    return Ok(());          // ← 交互单仍是 answered，没有回滚
};
if state.active_tasks.read().await.contains_key(session_id) {
    dispatch_guard.release().await;
    api.reply_text(message_id, "任务仍在处理上一次确认，请稍候", in_thread).await?;
    return Ok(());          // ← 同上
}
```

而 `answer_interaction` 的 SQL（`crates/hank-db/src/lib.rs`）是：

```sql
WHERE id = ? AND status = 'pending' AND (expires_at IS NULL OR expires_at > NOW())
```

**一旦离开 `pending` 就永远无法再次应答。**

### 用户看到什么

点「确认」→ 卡片变终态「已选择 确认」→ 收到「任务仍在处理上一次确认，请稍候」
→ **然后什么都不会发生**。回测永远不跑。再点也没用（卡片已终态、claim 已占用、
交互单已非 pending）。只能 `/new` 重开话题。

### 为什么兜底机制救不了

`expire_stale_interactions` 只处理：

```sql
WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at <= NOW()
```

卡死的这行是 `answered`，且飞书交互单的 `expires_at` 是 `NULL`（按设计，
5 分钟 TTL 只给微信）。两个条件都不满足，收尾任务永远不会碰它。

### 触发条件

`try_acquire` 拿不到名额，或 `active_tasks` 已含该 session。ask_user 期间 run 处于
暂停，正常情况下名额应为空，所以不是必然触发 —— 但连点、飞书重复投递、
或上一轮 turn 尚未完全收尾时会撞上。**低频，但后果不可恢复**，值得修。

### 附带的轻量问题（一起修）

若 `run_chat_turn` 启动失败（无可用 provider、外部 Agent 报错），交互单同样停在
`answered`。下一条用户消息会被 `resolve_pending_ask_user` 当成对这张陈旧交互单的
应答、包成 `ToolResult` 发给模型（`chat.rs:553` 附近），之后才标 `done` 自愈。
代价是**牺牲一条用户消息**（被误解成确认答复），比上面那个轻，但同样该修。

## 三、涉及文件清单

| 文件 | 改什么 |
|---|---|
| `crates/hank-db/src/lib.rs` | 新增 `revert_interaction_to_pending`；扩展 `expire_stale_interactions` 兜底 `answered` 僵尸 |
| `server/src/feishu/callback.rs` | 派发失败/抢不到名额时回滚交互单与卡片；调整应答与派发的先后 |

**不许碰**：

- `server/src/chat.rs`（第 550 行的 `done` 推进逻辑是对的，自愈路径保留）
- `crates/code-agent/`、`crates/code-tools/`
- `server/src/deployment.rs`、`server/src/feishu/router.rs`、`pusher.rs`、`card.rs`
- `admin/`（是任务 B）、`quant/`、`client/`

**保留工作区原有改动**：任务 A 的改动尚未提交，**不要回退它们**，在其之上继续改。
`docs/tasks/*.md` 均不要删除。

## 四、实现步骤

### 1. 调整回调顺序：先占派发名额，再应答

根因是「状态已改、名额没抢到」。最稳的修法是**把抢名额提到应答之前**，
让两者要么都成功、要么都不发生。

在 `handle_card_action` 里，把派发名额的获取从 `resume_interaction_on_session`
内部提到 `answer_interaction` **之前**：

```rust
// 先抢派发名额：避免"状态已改成 answered 但名额没抢到"导致确认被吞掉且无法重试。
// 名额抢不到说明该会话确实在忙，此时不应答、不改卡片，用户可稍后再点。
let dispatch_guard = if !interaction_id.is_empty() && !session_id.is_empty() {
    match state.tasks.try_acquire(&session_id).await {
        Some(guard) => {
            if state.active_tasks.read().await.contains_key(&session_id) {
                guard.release().await;
                return Ok(json!({
                    "toast": { "type": "warning", "content": "任务正在执行中，请稍候再点" }
                }));
            }
            Some(guard)
        }
        None => {
            return Ok(json!({
                "toast": { "type": "warning", "content": "任务正在执行中，请稍候再点" }
            }));
        }
    }
} else {
    None
};
```

关键点：

- 这个 early return **必须在 `card_action_claim`（`insert_channel_message`）之前**，
  否则 claim 已占用，用户稍后重点会被当成重复投递而拒绝。请把顺序调整为：
  **抢名额 → claim → 应答 → 改卡片 → 派发**。
- 返回 toast 而不是发文本消息 —— 卡片还没变终态，用户可以直接再点，
  toast 是恰当的即时反馈。
- `dispatch_guard` 要一路传进 `resume_interaction_on_session`，由它在
  `run_chat_turn` 返回后 `release()`（与 `router.rs` 里 `dispatch_task` 的
  既有做法一致：拿到 handle 后就还名额）。函数内部原有的两段 try_acquire /
  active_tasks 检查随之删除。

### 2. 派发失败时回滚交互单与卡片

即使名额抢到了，`run_chat_turn` 仍可能失败。此时必须把交互单退回 `pending`，
并把卡片改回可点状态，否则用户的确认被静默吞掉。

`hank-db` 新增：

```rust
/// 派发失败时把交互单退回 pending，让用户可以重新点确认。
/// 只回滚 answered——已 done/expired/cancelled 的不动，避免覆盖终态。
pub async fn revert_interaction_to_pending(&self, id: &str) -> Result<bool> {
    let result = db_retry!(sqlx::query(
        "UPDATE agent_interactions
             SET status = 'pending', answer = NULL, answered_by = NULL,
                 answered_at = NULL, updated_at = NOW()
           WHERE id = ? AND status = 'answered'"
    )
    .bind(id)
    .execute(&self.pool))?;
    Ok(result.rows_affected() == 1)
}
```

`resume_interaction_on_session` 里 `run_chat_turn` 返回 `Err` 时：

1. `revert_interaction_to_pending(&interaction_id)`
2. 用 `build_confirm_card` 重新渲染**可点**的确认卡片，`update_card` 回原 card
   （`card_message_id` 在交互单上，直接取）。这一步失败只记 warn，不要因为
   改卡片失败而丢掉回滚。
3. 给用户一条可读回复：「派发失败，已恢复待确认，可重新点击按钮。原因：…」

`resume_interaction_on_session` 需要 `interaction_id` 入参才能回滚 —— 目前签名没有，
补上。该函数已有 `#[allow(clippy::too_many_arguments)]`，再加一个参数不会新增告警；
但如果参数已超过 10 个，**改成传一个入参 struct**，不要继续堆。

注意 `_account` 那个下划线前缀的未使用参数：如果重构后仍不需要，直接删掉它，
不要留着占位。

### 3. 兜底：收尾 answered 僵尸

即使有上面两层，进程被 kill 仍可能留下 `answered` 僵尸。扩展启动收尾：

```rust
/// 进程重启收尾：
/// 1. 已过期的 pending → expired
/// 2. answered 僵尸 → pending（应答已记录但派发未完成，退回让用户重点）
///
/// 为什么 answered 要退回而不是标失败：用户的确认意图是真实的，
/// 丢掉它等于让用户白点一次且无从得知。退回 pending 可重试。
pub async fn expire_stale_interactions(&self) -> Result<(u64, u64)> {
    // 返回 (expired 条数, reverted 条数)
}
```

`answered` 僵尸的判定要**避免误伤正在派发中的交互单**：加时间窗，
只回滚 `answered_at < NOW() - INTERVAL 5 MINUTE` 的行。进程重启时不存在
「正在派发」的行，但手动调用或将来加定时清理时这个窗很重要。

调用点在 `main.rs` 启动序列（任务 A 已加），更新它以消费新的返回值并分别记日志。

### 4. 单元测试

DB 层无集成测试基建，**不要**新建测试容器。可测的是纯逻辑：

- 若第 1 步把「名额检查」抽成了独立纯函数，为它补测试。
- 若没有可抽的纯函数，**不强求新增测试** —— 本次修改主要是顺序与 SQL，
  靠编译期与人工验收保证。不要为了凑测试数写无意义的断言。

既有测试一条都不改、不删。

## 五、验收标准

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test -p hank-server
cargo test -p code-tools
cargo test -p code-agent
```

期望结果：

- 全部通过。
- clippy 警告数**不得超过 58**（任务 A 之后的当前值；基线 60，A 已降到 58）。
  不要引入新的 `too_many_arguments`。
- 既有测试全部保持通过。
- `grep -n "return Ok(())" server/src/feishu/callback.rs` 的每个 early return
  路径，都不应留下「交互单已 answered 但未派发」的状态。

**人工验收**（我来跑）：

1. 飞书触发高成本操作 → 点「确认」→ 回测正常执行（主路径不回归）。
2. 制造派发失败（临时停掉 provider）→ 点「确认」→ 卡片回到可点状态，
   收到「已恢复待确认」提示 → 恢复 provider 后再点 → 正常执行。
3. `SELECT id, status, answer FROM agent_interactions` 观察状态流转，
   不应出现停在 `answered` 且无后续的行。

## 六、约定

- 遵循 `CLAUDE.md`：中文注释、中文 commit message、`anyhow` 错误处理。
- 注释写**为什么**。回滚逻辑处必须写清「为什么退回 pending 而不是标失败」。
- commit message 建议：`fix(feishu): 派发失败时回滚交互单，确认不再被静默吞掉`
- 不新增依赖，不改 `Cargo.toml` / `config.toml`。
- 本次修改在任务 A 的未提交改动之上进行；A 与本次修正可以合成一个 commit 提交。
