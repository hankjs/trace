# 08 飞书交互单终态卡片同步

## 背景与目标

飞书渠道的确认卡片（`quant_confirm` / `ask_user`）与闸门大卡片（`task_gate` / `team_gate`）
上都带可点按钮。交互单进入终态后，卡片应当立刻变成灰色终态、按钮消失，否则用户看到的是
一张还在邀请点击的卡片，点下去只能得到一句"这个操作已经提交过了"的 toast。

目前只有**一条**路径做了卡片同步：同会话开新一轮时作废旧闸门单
（`db.supersede_pending_task_gates` → `interaction_flow::close_superseded_gate_card`，
调用点 `server/src/cli_agent.rs:2721`）。以下三条路径只改库状态、不动卡片：

1. **admin 手动取消**：`server/src/interactions.rs::cancel_interaction` 调
   `db.cancel_interaction` 后直接返回，卡片仍可点。
2. **启动过期回收**：`server/src/main.rs:205` 调 `db.expire_stale_interactions()`，
   把超时 pending 标 `expired`（微信 5 分钟 TTL），卡片仍可点。
3. **点击时惰性判过期**：`server/src/interaction_flow.rs::toast_for_unanswerable`
   发现已过期时补标 `expired`，只回 toast，卡片仍可点。

另外 `close_superseded_gate_card` 只服务 `task_gate` 一种 kind：它硬编码标题
「新任务 · 已被新一轮取代」，且用 `row.goal` 当问题文案，套到 `quant_confirm` /
`ask_user` 上标题和文案都不对。

**做完之后的可观察效果**：

- 在 admin 交互单页对一张 pending 的飞书交互单点「取消」，飞书群里那张卡片在几秒内
  变成灰色终态卡，正文显示「已取消（管理员）」，按钮消失，再点无按钮可点。
- 微信/飞书交互单因超时被回收后（重启扫表或用户点击时惰性判定），飞书卡片同步变灰，
  正文显示「已超时」。
- 上述改写对 `quant_confirm` / `ask_user` / `task_gate` / `team_gate` 四种 kind 都用
  正确的标题与问题文案，不再套用「已被新一轮取代」。
- 卡片改写失败（飞书接口报错、账号已删、`card_message_id` 为空）只写 warn 日志，
  **不影响**取消/过期本身成功——库状态是权威，卡片是尽力而为的镜像。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `crates/hank-db/src/lib.rs` | `expire_stale_interactions` 返回值改为结构体，带上被标 `expired` 的行的 `(id, card_message_id)`，供上层改写卡片 |
| `server/src/interaction_flow.rs` | 新增通用 `close_interaction_card` 与 `interaction_card_title`；把 `close_superseded_gate_card` 改成它的薄封装；`toast_for_unanswerable` 判过期后补改卡片 |
| `server/src/interactions.rs` | `cancel_interaction` 成功后尽力改写飞书卡片 |
| `server/src/main.rs` | 启动收尾适配新返回值，并对 expired 行改写卡片 |
| `docs/feishu.md` | 第九节「后续（未实现）」删掉已完成的那一条，正文补一句终态同步说明 |

**不许碰**：`server/src/feishu/card.rs`（`build_confirm_done_card` 现有签名与输出保持不变）、
`server/src/feishu/callback.rs`、`server/src/team_task/`、`server/src/cli_agent.rs`
（只保留它对 `close_superseded_gate_card` 的现有调用不变）、`admin/`、`client/`、`team/`、`quant/`。
保留工作区原有改动，不回退与本任务无关的内容。

## 实现步骤

### 1. hank-db：`expire_stale_interactions` 带出 expired 行的卡片 id

`crates/hank-db/src/lib.rs`，当前签名（约 4564 行）：

```rust
pub async fn expire_stale_interactions(&self) -> Result<(u64, u64, u64)>
```

- [ ] 在 `AgentInteraction` 附近（或紧邻该函数上方）新增返回结构体，带中文文档注释：

```rust
/// 启动收尾扫表的结果。
#[derive(Debug, Default)]
pub struct StaleInteractionSweep {
    /// 被标 expired 的 pending 行：`(交互单 id, card_message_id)`。
    /// 带出卡片 id 是为了让渠道把还亮着按钮的卡片改成终态。
    pub expired: Vec<(String, Option<String>)>,
    /// answered 僵尸退回 pending 的条数
    pub reverted: u64,
    /// executing 僵尸标 failed 的条数
    pub failed: u64,
}
```

- [ ] 把函数签名改为 `-> Result<StaleInteractionSweep>`。保留现有全部文档注释
      （三条僵尸处理规则的说明有价值），在末尾补一句为什么要带出卡片 id。
- [ ] expired 分支改为**先 SELECT 后 UPDATE**，与 `supersede_pending_task_gates`
      同一写法（SELECT 与 UPDATE 之间不加事务：这是启动期尽力而为的清扫，
      与既有 `supersede_pending_task_gates` 口径一致，注释里说明这一点）：

```rust
let expired: Vec<(String, Option<String>)> = db_retry!(sqlx::query_as(
    "SELECT id, card_message_id FROM agent_interactions
         WHERE status = 'pending'
           AND expires_at IS NOT NULL
           AND expires_at <= NOW()"
)
.fetch_all(&self.pool))?;
if !expired.is_empty() {
    db_retry!(sqlx::query(
        "UPDATE agent_interactions
             SET status = 'expired', updated_at = NOW()
             WHERE status = 'pending'
               AND expires_at IS NOT NULL
               AND expires_at <= NOW()"
    )
    .execute(&self.pool))?;
}
```

- [ ] `reverted` / `failed` 两条 UPDATE 保持原样，只是把 `rows_affected()` 装进结构体返回。
- [ ] 若 `StaleInteractionSweep` 定义在 `lib.rs` 内且外部要用，确认它是 `pub`
      （`hank-db` 的 `lib.rs` 里类型默认从 crate 根导出，与 `AgentInteraction` 同处理）。

### 2. interaction_flow：通用终态卡片改写

`server/src/interaction_flow.rs`。当前 `close_superseded_gate_card`（约 1112 行）
只处理 `task_gate`，硬编码标题与「已作废」。

- [ ] 新增纯函数 `interaction_card_title`，把 kind 映射到卡片标题。文案必须与
      现有各处保持一致（`build_task_gate_card` 用「新任务 · 待确认是否开始修」，
      `answer_and_resume` 步骤④ team_gate 用「团队任务闸门」，
      `confirm_card_from_interaction` 用「高成本操作确认」/「需要你的输入」）：

```rust
/// 交互单 kind → 卡片标题。终态改写与可点卡片必须用同一套标题，
/// 否则同一张卡片在不同阶段标题会跳变。
pub(crate) fn interaction_card_title(kind: &str) -> &'static str {
    match kind {
        "task_gate" => "新任务 · 待确认是否开始修",
        "team_gate" => "团队任务闸门",
        "quant_confirm" => "高成本操作确认",
        "ask_user" => "需要你的输入",
        _ => "待确认",
    }
}
```

- [ ] 新增纯函数从交互单里取"问题文案"，闸门类用 `goal`，其余用
      `resume_ref.question`，都为空时回落 `title`：

```rust
/// 终态卡片正文用的问题文案。闸门类的语义主体是 goal；
/// quant_confirm / ask_user 的原问句在 resume_ref.question。
pub(crate) fn interaction_card_question(row: &AgentInteraction) -> String {
    if row.kind == "task_gate" || row.kind == "team_gate" {
        return row
            .goal
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| row.title.clone());
    }
    row.resume_ref
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| v["question"].as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| row.title.clone())
}
```

- [ ] 新增通用改写函数，替代 `close_superseded_gate_card` 的实现主体：

```rust
/// 把某张仍可点的飞书卡片改成灰色终态，让按钮不再邀请点击。
///
/// `choice_label` 是终态文案（如「已取消」「已超时」「已作废」），
/// `operator_label` 是执行者展示名（如「管理员」「系统」）。
///
/// 尽力而为：非飞书渠道、账号已删、卡片 id 为空都直接返回 Ok。
/// 库状态才是权威，卡片只是镜像；改卡失败不能让取消/过期本身失败。
pub(crate) async fn close_interaction_card(
    state: &Arc<AppState>,
    interaction_id: &str,
    card_message_id: Option<&str>,
    choice_label: &str,
    operator_label: &str,
) -> Result<()>
```

实现要点：

1. `state.db.get_interaction(interaction_id)`，取不到直接 `Ok(())`。
2. `row.channel != "feishu"` 直接 `Ok(())`（微信没有卡片）。
3. 卡片 id 取 `card_message_id` 参数，为空回落 `row.card_message_id`；仍为空返回 `Ok(())`。
4. `row.account_id` 为空返回 `Ok(())`；`state.db.get_feishu_account` 取不到返回 `Ok(())`
   （账号被删是正常终局，不是错误）。
5. `FeishuApi::new_archived(&account, state.db.clone())`，用
   `build_confirm_done_card(interaction_card_title(&row.kind), &question, choice_label, operator_label, Some(interaction_id))`
   调 `api.update_card(card_mid, &card)`。

- [ ] 把 `close_superseded_gate_card` 改成薄封装，保持现有签名与调用点
      （`cli_agent.rs:2721`）不变、行为不变（原标题是「新任务 · 已被新一轮取代」，
      终态文案「已作废」、执行者「系统」）。因标题现在统一走
      `interaction_card_title`，闸门标题会变成「新任务 · 待确认是否开始修」而正文写
      「已选择：已作废（系统）」——这是可接受的统一化；把这层取舍写进函数注释：

```rust
/// 闸门单被同会话新一轮取代时的卡片改写。
///
/// 标题统一走 interaction_card_title，取代语义体现在正文的「已作废」上，
/// 避免同一张卡片在不同阶段标题跳变。
pub(crate) async fn close_superseded_gate_card(
    state: &Arc<AppState>,
    interaction_id: &str,
    card_message_id: &str,
) -> Result<()> {
    close_interaction_card(state, interaction_id, Some(card_message_id), "已作废", "系统").await
}
```

- [ ] `toast_for_unanswerable` 里补改卡片：该函数在发现 `expires_at` 已过时会补标
      `expired`（约 857 行）。在那次 `update_interaction_status` 成功之后，追加一次
      尽力而为的卡片改写（用 `row.card_message_id`，`choice_label = "已超时"`，
      `operator_label = "系统"`），失败只 `tracing::warn!`。注意该函数返回 `String`
      且无 `api` 参数，直接调 `close_interaction_card` 即可（它内部自己解析账号）。

### 3. admin 手动取消同步卡片

`server/src/interactions.rs::cancel_interaction`（约 154 行）。当前
`db.cancel_interaction` 返回 `Ok(true)` 后直接重查返回。

- [ ] 在 `Ok(true)` 分支里、重查之前，插入尽力而为的卡片改写：

```rust
Ok(true) => {
    // 尽力而为：卡片改灰失败不影响取消结果（库状态是权威）。
    if let Err(e) = interaction_flow::close_interaction_card(
        &state, &id, None, "已取消", "管理员",
    )
    .await
    {
        tracing::warn!(interaction_id = %id, "取消后改写飞书卡片失败: {e:#}");
    }
    match state.db.get_interaction(&id).await { /* 原逻辑不变 */ }
}
```

- [ ] `interactions.rs` 已 `use crate::interaction_flow;`，无需新增 import；
      如缺 `tracing` 引用按文件既有风格补。

### 4. main.rs 适配新返回值并改写过期卡片

`server/src/main.rs:205` 起的启动收尾块。

- [ ] `match state.db.expire_stale_interactions().await` 的 `Ok` 分支改为接结构体：
      三条 `tracing::info!` 的判断分别用 `sweep.expired.len()`、`sweep.reverted`、
      `sweep.failed`，日志文案保持原样。
- [ ] 在 expired 日志之后，遍历 `sweep.expired` 逐条改写卡片：

```rust
for (interaction_id, card_message_id) in &sweep.expired {
    if card_message_id.as_deref().is_none_or(str::is_empty) {
        continue;
    }
    if let Err(e) = interaction_flow::close_interaction_card(
        &state,
        interaction_id,
        card_message_id.as_deref(),
        "已超时",
        "系统",
    )
    .await
    {
        tracing::warn!(interaction_id = %interaction_id, "过期回收改写飞书卡片失败: {e:#}");
    }
}
```

注意：这段在 `state` 构造之后、`feishu::monitor::start_monitors` 之前，
此时飞书长连接还没起，但 `close_interaction_card` 走的是 REST（自己取 tenant token），
与长连接无关，可以直接调。若 `is_none_or` 在当前 Rust 版本不可用，
改写成 `.map_or(true, str::is_empty)` 等价形式。

- [ ] 若 `main.rs` 尚未导入 `interaction_flow` 的这个符号，按文件既有风格用全路径
      `crate::interaction_flow::close_interaction_card(...)` 或补 `use`。

### 5. 单元测试

- [ ] 在 `server/src/interaction_flow.rs` 的 `mod tests` 里补两个纯函数测试
      （现有 test mod 已 `use super::{build_admin_interaction_url, card_action_claim_id};`，
      按需扩展 import）：

```rust
#[test]
fn card_title_matches_live_card_titles() {
    // 终态卡与可点卡必须同标题，否则同一张卡片标题会跳变。
    assert_eq!(interaction_card_title("task_gate"), "新任务 · 待确认是否开始修");
    assert_eq!(interaction_card_title("team_gate"), "团队任务闸门");
    assert_eq!(interaction_card_title("quant_confirm"), "高成本操作确认");
    assert_eq!(interaction_card_title("ask_user"), "需要你的输入");
    assert_eq!(interaction_card_title("unknown_kind"), "待确认");
}
```

`interaction_card_question` 的测试需要构造 `AgentInteraction`。若字段过多导致构造
成本高，可把取文案逻辑拆成接 `(kind, goal, resume_ref, title)` 四个参数的纯私有函数
再由 `interaction_card_question` 转调，对纯函数写测试，覆盖三种情况：
闸门类取 `goal`、非闸门取 `resume_ref.question`、两者都空回落 `title`。

### 6. 文档

- [ ] `docs/feishu.md` 第九节「后续（未实现）」里删掉这一条：

```
- 取消交互单时同步把飞书卡片改成终态（`task_gate` 被新一轮取代时已会改灰；
  admin 手动取消、过期回收等路径的卡片仍可点，点了会被拒但不会误执行）
```

- [ ] 在「确认闸门 / 交互单落表」那条要点末尾补一句：交互单进入终态（取代 / 取消 /
      过期）时会同步把飞书卡片改成灰色终态，改卡失败只记日志、不影响库状态。

## 明确边界

- 不改 `build_confirm_done_card` 的签名与输出结构；只复用它。
- 不给 `executing` 僵尸（`sweep.failed` 那批）改卡片：那批的确认卡在
  `answer_and_resume` 步骤④ 已经改成终态了，它们的 `card_message_id` 指向的是已终态的卡。
- 不改微信渠道任何代码；微信没有卡片，靠 `channel != "feishu"` 早返回。
- 不引入新依赖，不改 `Cargo.toml`。
- 不动 admin / client / team / quant 前端（`analysis` 渲染 markdown 是另一个任务）。
- 保留工作区原有改动，不回退与本任务无关的内容。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo test -p hank-db
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

期望：全部通过，无新增 warning。特别确认：

- `expire_stale_interactions` 的所有调用点都已适配新返回类型（用
  `rg 'expire_stale_interactions' -n` 确认只有 `main.rs` 一处调用）。
- `close_superseded_gate_card` 调用点（`cli_agent.rs:2721`）签名未变、编译通过。
- 新增测试 `card_title_matches_live_card_titles` 通过。

## 约定

遵循 `CLAUDE.md`：中文注释、中文 commit message；后端错误处理沿用 `anyhow`；
日志用 `tracing`，字段风格与文件内既有调用一致。注释写"为什么"而非"是什么"
（本仓库既有注释密度较高，保持同等水平）。commit message 建议：

```
fix(feishu): 交互单终态时同步改写卡片
```
