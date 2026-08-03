# 09A-fix 应答路径的终态卡收敛

> 09A 的补充修正，**在 09B 之前执行**。
> 09A 本身已验收通过（卡片标题 + 终态卡按钮 + `feishu_card_actions`），本文档不动那些成果。
> 执行顺序：09A（已完成）→ **09A-fix（本文档）** → 09B → 09C

## 背景与目标

`agent_interactions` 的终态卡片改写目前有**四条路径**，其中三条（取消 / 过期 / 被新一轮
取代）已经统一走 `close_interaction_card`（`server/src/interaction_flow.rs:1189`），
标题走 `interaction_card_title`。

但**第四条——应答路径**（`answer_and_resume` 的步骤④，`server/src/interaction_flow.rs:176`）
仍是独立实现，带来两个缺陷：

### 缺陷 1：admin 手动应答不改卡片

步骤④ 整块被 `if let Some(ref ctx) = channel_ctx` 包着：

```rust
// server/src/interaction_flow.rs:176
// ④ 改终态卡（仅飞书卡片路径）
if let Some(ref ctx) = channel_ctx {
    if let Some(card_mid) = &ctx.card_message_id {
```

而 admin 手动应答传的是 `None`（`server/src/interactions.rs:143`
`answer_and_resume(&state, &id, answer, &claims.sub, None)`）。

**后果**：管理员在 admin「交互单」页替用户点了「确认」，交互单已经
`answered → executing → done`，但飞书群里那张卡片按钮**依然亮着**。用户再点会得到
「这个操作已经提交过了」——不会误执行，但卡片在骗人。这与已经修好的「取消」路径
是同一类缺陷（取消已经会改卡，应答却不会）。

### 缺陷 2：步骤④ 的标题与 helper 并存两套约定

步骤④ 自己硬编码了一套标题（`server/src/interaction_flow.rs:198`）：

```rust
let title = if answered_row.kind == "task_gate" {
    "新任务 · 待确认是否开始修"
} else if answered_row.kind == "team_gate" {
    "团队任务闸门"
} else {
    "待确认"          // ← quant_confirm / ask_user 都落到这里
};
```

而 `interaction_card_title`（`interaction_flow.rs:1141`）对这两种 kind 返回的是
「高成本操作确认」/「需要你的输入」——那是照可点卡片
`confirm_card_from_interaction` 的文案对齐的。

**后果**：同一张 quant 确认卡，点按钮应答后标题变「待确认」，被 admin 取消则变
「高成本操作确认」。同一张卡片在不同终态下标题跳变。

问题文案的提取逻辑也在步骤④ 里重复了一遍（多一个 `ctx.question_fallback` 兜底），
与 `interaction_card_question`（`interaction_flow.rs:1153`）是两份实现。

**做完之后的可观察效果**：

- 在 admin 交互单页对一张 pending 的飞书交互单点「确认」，飞书群里那张卡片在几秒内
  变灰、按钮消失，正文显示「已选择：确认（管理员）」。
- 飞书按钮点击应答后，卡片正文显示「已选择：确认（你）」——操作者展示名按路径区分。
- quant 确认卡应答后的标题是「高成本操作确认」，与被取消时一致，不再是「待确认」。
- 应答 / 取消 / 过期 / 取代四条路径共用同一段改卡实现。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `server/src/interaction_flow.rs` | 新增 `patch_card_to_done` 作为四条路径的唯一改卡实现；步骤④ 改为调它；`close_interaction_card` 改为转调它；补一个标题一致性测试 |

**只改这一个文件。** 不许碰：`server/src/feishu/`（09A 的成果，已验收）、
`server/src/interactions.rs`（调用方签名不变，无需改动）、
`crates/`（09B / 09C 的范围）、`admin/`、`client/`、`team/`、`quant/`、
`server/src/weixin/`。

保留工作区原有改动，不回退与本任务无关的内容。

## 实现步骤

### 1. 新增 `patch_card_to_done` 作为唯一实现

在 `close_interaction_card`（约 1189 行）之前或之后新增。它要同时服务两种调用场景：
飞书回调（已有 `FeishuApi` 和 `card_message_id`）与 admin / 系统路径（都没有，需按
交互单的 `account_id` 自行解析账号）。

```rust
/// 终态卡片改写的唯一实现：应答、取消、过期、取代四条路径共用。
///
/// 为什么合成一处：这四条路径都要「查账号 → 拼终态卡 → update_card」，
/// 各写一遍会让标题与问题文案漂移——步骤④ 曾自带一套硬编码标题，
/// 导致同一张 quant 确认卡应答后叫「待确认」、被取消后叫「高成本操作确认」。
///
/// `api` 为 None 时（admin 手动应答、取消、过期回收）按交互单的 `account_id`
/// 自行解析飞书账号；飞书按钮回调直接复用回调那侧已建好的 api，不重复建客户端。
/// `question_fallback` 只有卡片回调 payload 带，其余路径传 None。
///
/// 尽力而为：非飞书渠道、账号已删、卡片 id 为空都直接返回 Ok。
/// 库状态才是权威，卡片只是镜像；改卡失败不能让应答/取消/过期本身失败。
#[allow(clippy::too_many_arguments)]
async fn patch_card_to_done(
    state: &Arc<AppState>,
    row: &AgentInteraction,
    api: Option<&FeishuApi>,
    card_message_id: Option<&str>,
    question_fallback: Option<&str>,
    choice_label: &str,
    operator_label: &str,
) -> Result<()> {
    if row.channel != "feishu" {
        return Ok(());
    }
    let card_mid = card_message_id
        .filter(|s| !s.is_empty())
        .or(row.card_message_id.as_deref().filter(|s| !s.is_empty()));
    let Some(card_mid) = card_mid else {
        return Ok(());
    };

    // 已有 api 直接用，避免飞书回调路径重复建客户端。
    let owned_api = match api {
        Some(_) => None,
        None => {
            let Some(account_id) = row.account_id.as_deref().filter(|s| !s.is_empty()) else {
                return Ok(());
            };
            let Some(account) = state.db.get_feishu_account(account_id).await? else {
                // 账号被删是正常终局，不是错误。
                return Ok(());
            };
            Some(FeishuApi::new_archived(&account, state.db.clone()))
        }
    };
    let api = api.or(owned_api.as_ref()).expect("api 必有其一");

    let mut question = interaction_card_question(row);
    // 交互单上没记下问句时（回落到了 title），才用卡片 payload 带的兜底。
    if question == row.title {
        if let Some(fallback) = question_fallback.filter(|s| !s.is_empty()) {
            question = fallback.to_string();
        }
    }
    let card = build_confirm_done_card(
        interaction_card_title(&row.kind),
        &question,
        choice_label,
        operator_label,
        Some(&row.id),
    );
    api.update_card(card_mid, &card).await
}
```

- [ ] 注意 `owned_api` 那段的写法：不能在 `match` 里直接返回 `&FeishuApi`（借用局部值），
      必须先绑定到 `owned_api` 变量延长生命周期，再取引用。照上面写即可。

### 2. `close_interaction_card` 改为转调

现有 `close_interaction_card`（约 1189 行）自己做了「查 row → 查账号 → 拼卡 → update」。
把后三步交给 `patch_card_to_done`，它只负责查 row：

```rust
pub(crate) async fn close_interaction_card(
    state: &Arc<AppState>,
    interaction_id: &str,
    card_message_id: Option<&str>,
    choice_label: &str,
    operator_label: &str,
) -> Result<()> {
    let Some(row) = state.db.get_interaction(interaction_id).await? else {
        return Ok(());
    };
    patch_card_to_done(
        state,
        &row,
        None,
        card_message_id,
        None,
        choice_label,
        operator_label,
    )
    .await
}
```

- [ ] `close_superseded_gate_card`（约 1231 行）保持不变——它已经是
      `close_interaction_card` 的薄封装，签名与 `cli_agent.rs:2721` 的调用点都不用动。

### 3. 步骤④ 改为调 `patch_card_to_done`

把 `server/src/interaction_flow.rs:176` 起的整段（从 `// ④ 改终态卡（仅飞书卡片路径）`
到那个 `if let Some(ref ctx)` 块结束，约 176-211 行）**整体替换**为：

```rust
    // ④ 改终态卡。飞书按钮点过来时用回调自带的 api 与 card_message_id；
    // admin 手动应答（channel_ctx 为 None）也要改——否则管理员替用户拍板后，
    // 群里那张卡片按钮依然亮着，是在骗人。此时按交互单的 account_id 自行解析账号。
    // 标题/文案统一走 interaction_card_* helper，与取消/过期路径同一套约定。
    let operator_label = if channel_ctx.is_some() {
        "你"
    } else {
        "管理员"
    };
    if let Err(e) = patch_card_to_done(
        state,
        &answered_row,
        channel_ctx.as_ref().map(|c| &c.api),
        channel_ctx
            .as_ref()
            .and_then(|c| c.card_message_id.as_deref()),
        channel_ctx
            .as_ref()
            .and_then(|c| c.question_fallback.as_deref()),
        answer,
        operator_label,
    )
    .await
    {
        tracing::warn!(interaction_id, "feishu: patch confirm card failed: {e:#}");
    }
```

- [ ] `channel_ctx` 在步骤⑤ 还要用（`resolve_resume_api(state, &answered_row, channel_ctx.as_ref())`
      等），所以这里只能借用（`.as_ref()`），**不能 move**。上面的写法已经是借用。
- [ ] 替换后确认 `build_confirm_done_card` 这个 import 仍被使用
      （`patch_card_to_done` 里用了），不要误删 import。
- [ ] 替换后 `serde_json::Value` 的 import 是否仍被使用要确认——步骤④ 原来用
      `serde_json::from_str::<Value>` 解析 `resume_ref`，移除后该文件其他地方仍在用
      `Value`（如 `task_gate_card_from_interaction`），所以 import 保留。
      **以编译结果为准**，有 unused 警告就处理。

### 4. 补标题一致性测试

在 `mod tests` 里（`card_title_matches_live_card_titles` 那个测试之后）新增：

```rust
    /// 步骤④ 曾自带一套硬编码标题（其余 kind 一律「待确认」），导致同一张
    /// quant 确认卡应答后叫「待确认」、被取消后叫「高成本操作确认」。
    /// 收敛到 helper 后这里锁住：四种 kind 都不该回落到兜底标题。
    #[test]
    fn answer_path_title_agrees_with_cancel_path() {
        for kind in ["quant_confirm", "ask_user", "task_gate", "team_gate"] {
            assert_ne!(
                interaction_card_title(kind),
                "待确认",
                "{kind} 不应回落到兜底标题"
            );
        }
    }
```

- [ ] test mod 的 `use super::{...}` 已包含 `interaction_card_title`，无需改 import。

## 明确边界

- **不改 `answer_and_resume` 的五步顺序**（抢名额 → claim → 应答 → 改卡 → 派发）。
  本任务只替换步骤④ 的实现，不动它在序列中的位置。文件头注释解释了为什么顺序不能动。
- **不改 `close_superseded_gate_card` 的签名**，`cli_agent.rs:2721` 的调用点不动。
- **不改 `interaction_card_title` / `interaction_card_question` 的返回值**——
  它们已被取消/过期路径使用并有测试锁着，改了会连带影响那三条路径。
- **不动 09A 的成果**：`server/src/feishu/card.rs`、`callback.rs`、`pusher.rs`、
  `feishu_card_actions` 表相关代码一律不碰。
- **不改 `server/src/interactions.rs`**：它调 `answer_and_resume(..., None)` 的方式不变，
  改卡由 `answer_and_resume` 内部完成。
- 不引入新依赖，不改 `Cargo.toml`。
- 保留工作区原有改动，不回退与本任务无关的内容。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo test -p hank-db
```

期望：编译通过；测试全绿。当前基线是 **hank-server 225 passed / 0 failed**，
本任务新增 1 个测试，做完应为 **226 passed**。

特别确认：

- `rg 'patch_card_to_done' server/src/` 应有 3 处：定义、`close_interaction_card` 转调、
  步骤④ 调用。
- `rg '"待确认"' server/src/interaction_flow.rs` 应**只**在 `interaction_card_title`
  的兜底分支出现一次（步骤④ 那套硬编码已删除）。
- 新测试 `answer_path_title_agrees_with_cancel_path` 通过。
- `cargo fmt -p hank-server -- --check 2>&1 | grep 'interaction_flow.rs'` 的结果：
  改动前该文件有 3 处 fmt diff（176 / 240 / 296 行附近，均为**既有欠账**）。
  改完后 176 那处会随代码替换消失（因为那段代码不存在了），
  **剩下两处属于既有欠账，不要去动**。你新增的代码本身必须 fmt 干净。

**已知既有欠账**（不要顺手清理，会污染 diff）：
`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all -- --check`
在改动前即失败（`crates/code-tools/` 6 项 clippy、`hank-db` 6 项 `too_many_arguments`、
`server/src/team_task/` 等多个文件 fmt）。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；错误处理用 `anyhow`；日志用 `tracing`
且字段风格与文件内既有调用一致；注释写"为什么"而非"是什么"。

commit message 建议：

```
fix(feishu): 应答路径收敛到统一改卡实现，admin 应答也改卡片
```
