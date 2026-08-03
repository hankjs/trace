# 09A 飞书卡片标题与终态按钮区

> 三份文档的第一份，**先执行这份**。
> A：卡片标题带任务摘要 + 终态卡按钮区骨架（本文档）
> B：quant 批量同意按钮 + suggest_actions 工具
> C：ask_user 多问题作答
>
> B 依赖 A 建立的按钮区基础设施，C 依赖 A 与 B。不要提前做 B / C 的内容。

## 背景与目标

飞书派单后会回一张蓝色进度卡（`server/src/feishu/pusher.rs`），2s 节流刷新，完成时改绿。
当前有两个问题：

1. **卡片标题写死「Agent 任务」**（`pusher.rs` 里四处 `build_task_card` 全部硬编码）。
   同一个话题里派几次单，卡片长得一模一样，翻历史分不清哪张是哪个任务。
2. **终态卡没有任何按钮**。`build_task_card` 的 body 只有一个 markdown 元素，没有 action 区。
   任务跑完只能干看，无法「展开完整总结」，也无法一键触发后续动作。

本文档解决这两点，并为 B / C 建立按钮区基础设施。

**做完之后的可观察效果**：

- 飞书发「帮我 review 用户登录的实现」，卡片标题显示 `任务 · 帮我 review 用户登录的实现`
  （超长截断到 24 字），而不是「Agent 任务」。
- 任务完成后绿卡底部出现一个「查看详情」按钮。点击后**在话题内另发一条消息**，
  内容是完整总结全文（不受进度卡长度限制）。原卡片保持不变。
- 反复点「查看详情」会反复发送，每次都带 toast 提示「详情已发送」——这是有意的，
  用户主动点就该有响应，不做去重。
- 未绑定用户点按钮得到 toast 提示，不会触发任何动作。

**本文档不做**（属于 B / C，不要提前实现）：

- 「由 agent 决定的动作按钮」（B：需要新的 `suggest_actions` 工具）
- quant 确认卡的第三个按钮（B）
- 多问题作答（C）

但**要建好** `task_suggest` 的回调分支与表结构，B 只需往里填内容。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `crates/hank-db/src/lib.rs` | 新表 `feishu_card_actions` + 建表语句 + CRUD + 启动清理 |
| `server/src/feishu/card.rs` | `TaskCardOptions` 加 `title` 语义说明与 `actions` 字段；`build_task_card` 渲染 action 区 |
| `server/src/feishu/pusher.rs` | `spawn` 加 `task_title` 入参；四处 `build_task_card` 复用同一 title；终态卡写入 actions |
| `server/src/feishu/router.rs` | 派发点传入任务摘要 |
| `server/src/interaction_flow.rs` | 两处 `pusher::spawn` 适配新签名 |
| `server/src/team_task/orchestrator.rs` | 一处 `pusher::spawn` 适配新签名 |
| `server/src/feishu/callback.rs` | 新增 `task_detail` / `task_suggest` 两个分支 |
| `docs/feishu.md` | 「三、用法」补卡片按钮说明 |

**不许碰**：`server/src/weixin/`（微信有自己的 pusher，签名不同，不要动）、
`crates/code-agent/`、`crates/code-tools/`（那是 B / C 的范围）、
`admin/`、`client/`、`team/`、`quant/`。
保留工作区原有改动（`quant/` 下有未提交的 scheduler 改动，**不要回退**）。

## 实现步骤

### 1. 新表 `feishu_card_actions`

**为什么需要这张表**：飞书按钮的 callback value 由客户端回传，把要执行的 prompt
明文放进 value，等于让客户端能改写待执行指令。所以 value 里只放一个 `id`，
真正的 payload 存服务端，回调时按 id 查。详情全文同理（也避免 value 超长）。

- [ ] 在 `crates/hank-db/src/lib.rs` 的建表区（`agent_interactions` 建表语句附近，
      约 1296 行那块 `CREATE TABLE IF NOT EXISTS` 群）追加：

```sql
CREATE TABLE IF NOT EXISTS feishu_card_actions (
    id VARCHAR(36) PRIMARY KEY,
    session_id VARCHAR(36) NOT NULL,
    /// detail = 展开总结全文；suggest = 以 payload 为 prompt 起新一轮
    kind VARCHAR(32) NOT NULL,
    label VARCHAR(64) NOT NULL DEFAULT '',
    payload MEDIUMTEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT NOW(),
    INDEX idx_fca_session (session_id, created_at),
    INDEX idx_fca_created (created_at)
) DEFAULT CHARSET=utf8mb4
```

注意：SQL 里不能写 `///` 注释，上面那行改成 `-- ` 或直接去掉，把说明写在 Rust 侧注释里。

- [ ] 新增行模型与 CRUD（放在交互单 CRUD 之后，约 4700 行区域）：

```rust
/// 飞书卡片按钮的服务端 payload。
///
/// 按钮 callback value 只带这张表的 id：value 是客户端回传的，
/// 把 prompt 明文放进去等于允许客户端改写待执行指令。
#[derive(Debug, Clone, FromRow)]
pub struct FeishuCardAction {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub label: String,
    pub payload: String,
    pub created_at: DateTime<Utc>,
}

/// 批量写入一张卡片的按钮 payload，返回各自的 id（顺序与入参一致）。
pub async fn create_feishu_card_actions(
    &self,
    session_id: &str,
    items: &[(String, String, String)], // (kind, label, payload)
) -> Result<Vec<String>>

pub async fn get_feishu_card_action(&self, id: &str) -> Result<Option<FeishuCardAction>>

/// 清理 30 天前的卡片 payload。卡片本身在飞书侧不会消失，
/// 但点很久以前的按钮属于异常操作，查不到 payload 时回 toast 提示即可。
/// 返回删除行数。
pub async fn cleanup_feishu_card_actions(&self) -> Result<u64>
```

`create_feishu_card_actions` 用 `Uuid::new_v4()` 生成 id，逐条 INSERT
（条数上限 4，不必做批量 INSERT 优化）。全部走 `db_retry!`。

- [ ] 在 `server/src/main.rs` 的启动收尾块（`expire_stale_interactions` 那段之后）
      追加一次清理，与既有风格一致：

```rust
match state.db.cleanup_feishu_card_actions().await {
    Ok(n) if n > 0 => tracing::info!(count = n, "启动收尾：清理过期飞书卡片按钮 payload"),
    Ok(_) => {}
    Err(e) => tracing::warn!("清理飞书卡片按钮 payload 失败: {e:#}"),
}
```

### 2. `card.rs`：`build_task_card` 支持 action 区

- [ ] `TaskCardOptions` 的 `title` 字段补注释说明它现在承载任务摘要，并新增：

```rust
/// 终态卡按钮。运行中态必须传空 vec——进度卡上出现按钮会诱导用户
/// 在任务还没跑完时点击。
pub actions: Vec<TaskCardAction>,

/// 卡片按钮：label 给用户看，action_id 是 feishu_card_actions 主键。
pub struct TaskCardAction {
    pub label: String,
    /// 回调 action 名：task_detail / task_suggest
    pub action: String,
    pub action_id: String,
}
```

- [ ] `build_task_card` 在 body 的 markdown 元素之后，`actions` 非空时追加 action 区。
      按钮 callback value 与现有 `answer` 按钮同构，但用**独立的 action 名**，
      避免和交互单应答混淆：

```rust
let mut elements = vec![json!({ "tag": "markdown", "content": /* 原内容 */ })];
if !opts.actions.is_empty() {
    let buttons: Vec<Value> = opts.actions.iter().enumerate().map(|(i, a)| {
        json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": a.label },
            "type": if i == 0 { "primary" } else { "default" },
            "behaviors": [{
                "type": "callback",
                "value": {
                    "action": a.action,
                    "action_id": a.action_id,
                    "session_id": opts.session_id,
                    "chat_id": opts.chat_id,
                    "topic_id": opts.topic_id,
                }
            }]
        })
    }).collect();
    elements.push(json!({ "tag": "action", "actions": buttons }));
}
```

- [ ] 这意味着 `TaskCardOptions` 还要新增 `session_id` / `chat_id` / `topic_id`
      三个字段（callback value 需要）。运行中态这三个照常填，只是没按钮用不到。

- [ ] `title` 渲染：header 的 `plain_text` **不解析 markdown**，但会被换行和控制字符
      弄坏布局。新增纯函数并单测：

```rust
/// 任务摘要 → 卡片标题。header 是 plain_text，换行/控制字符会弄坏布局，
/// 必须压成单行；超长按 chars() 截断（不能按 byte，中文会截出半个字）。
pub fn build_task_title(summary: &str) -> String {
    const MAX: usize = 24;
    let one_line: String = summary
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return "Agent 任务".to_string();
    }
    let short: String = trimmed.chars().take(MAX).collect();
    if trimmed.chars().count() > MAX {
        format!("任务 · {short}…")
    } else {
        format!("任务 · {short}")
    }
}
```

- [ ] 单测覆盖：普通短文本、超长截断（确认中文不被截坏）、含换行/制表符被压平、
      全空白回落「Agent 任务」。

### 3. `pusher.rs`：贯通 title 与终态 actions

- [ ] `pub fn spawn` 新增入参 `task_title: String`，放在 `session_id` 之后
      （参数已有 8 个，加到 9 个会触发 `clippy::too_many_arguments`——
      给 `spawn` 和 `run` 都加 `#[allow(clippy::too_many_arguments)]`，
      并在注释里说明这是渠道上下文的直传，拆结构体收益不大）。
- [ ] `run` 内部把四处 `build_task_card` 的 `title` 全部改成同一个变量
      （`let card_title = build_task_title(&task_title);` 算一次，四处复用），
      不要各写一遍。
- [ ] 运行中态（`push_running`、启动那张蓝卡、失败态）`actions: vec![]`。
- [ ] **成功终态**（约 493 行 `TaskStatus::Success` 那处）：在 `updater.finish(...)`
      之前，把完整总结写入 `feishu_card_actions` 并拿到 id：

```rust
// 详情全文进表而不进 callback value：value 有大小限制，且客户端可改。
// 写库失败不影响卡片主体，只是没有详情按钮——不能让它整轮失败。
let detail_actions = match state
    .db
    .create_feishu_card_actions(
        &session_id,
        &[("detail".to_string(), "查看详情".to_string(), body.clone())],
    )
    .await
{
    Ok(ids) => ids
        .into_iter()
        .next()
        .map(|id| TaskCardAction {
            label: "查看详情".to_string(),
            action: "task_detail".to_string(),
            action_id: id,
        })
        .into_iter()
        .collect(),
    Err(e) => {
        tracing::warn!(session_id, "写入卡片详情 payload 失败: {e:#}");
        vec![]
    }
};
```

`body` 是 `extract_file_markers` 之后的正文（不含 `[file:]` 标记）。
**注意**：要存的是**未截断**的全文，不是 `truncate_final` 之后的——详情按钮的价值
就在于绕过进度卡的长度限制。

- [ ] 终态卡的 `actions: detail_actions`。失败终态不加按钮（失败时没有总结可展开）。

### 4. 四处 `pusher::spawn` 调用点适配

`pusher::spawn` 共 4 个飞书调用点（**微信那处 `weixin/router.rs:925` 是另一个
pusher，签名不同，不要动**）：

- [ ] `server/src/feishu/router.rs:1338`：传用户本轮原话。该函数能拿到派发文本
      （`dispatch_task` 的 `text` 参数 / `dispatch_task_content` 的 content）。
      **content 可能是图片多模态块**，此时 `text_from_blocks` 类逻辑取不到文本，
      传空串让 `build_task_title` 回落到「Agent 任务」即可，不要 panic。
      如果该话题已有 `goal`（task_gate 路径），优先用 `goal`。
- [ ] `server/src/interaction_flow.rs:595`（task_gate resume 第二轮）：
      传交互单的 `goal`，为空回落 `title`。
- [ ] `server/src/interaction_flow.rs:957`（confirm resume）：
      传交互单的 `goal` 或 `resume_ref.question`，可复用本仓已有的
      `interaction_card_question(&row)`。
- [ ] `server/src/team_task/orchestrator.rs:843`：传 `task.title`
      （`TeamTask` 有 `title` 字段）。

### 5. `callback.rs`：两个新分支

在 `handle_card_action` 里现有 `deploy_approval` 分支之后、`answer` 判断之前插入。

- [ ] **公共前置**：两个分支都需要「操作者已绑定」+「session 归属校验」。
      现有 `answer` 分支的绑定检查在函数后半段，这两个新分支在它之前，
      所以要各自做一次绑定查询（或把绑定查询提到分支之前——**推荐后者**，
      但注意不要改动 `answer` 分支既有的 toast 文案与顺序）。

- [ ] **安全边界（必须做）**：`task_suggest` 会真的起一轮执行，而它没有交互单兜底
      身份（`answer` 路径靠交互单的 `user_id`）。必须显式校验
      **session 属于点击者**：

```rust
// A 用户不能点 B 用户卡片上的按钮触发执行。
// answer 路径靠交互单的 user_id 兜住身份，task_suggest 没有交互单，必须显式查。
let session = state.db.get_session(&action.session_id).await?;
let owned = session.as_ref().and_then(|s| s.user_id.as_deref()) == Some(binding.user_id.as_str());
if !owned {
    return Ok(json!({
        "toast": { "type": "error", "content": "这不是你的任务，无法操作" }
    }));
}
```

`session` 的用户字段名请按 `hank_db::Session` 实际定义写（可能是 `user_id: Option<String>`
或 `String`，照实处理，不要猜）。

- [ ] **`task_detail` 分支**：

```rust
if value["action"].as_str() == Some("task_detail") {
    // 查 payload → 话题内另发一条完整总结。
    // 不做去重：用户主动点就该有响应，反复点反复发是可接受的。
    let action_id = value["action_id"].as_str().unwrap_or("");
    let Some(row) = state.db.get_feishu_card_action(action_id).await? else {
        return Ok(json!({
            "toast": { "type": "warning", "content": "详情已过期（超过 30 天）" }
        }));
    };
    // 归属校验（同上）
    // 发送：飞书单条文本有长度上限，超长要分段发，不能静默截断
    ...
    return Ok(json!({ "toast": { "type": "success", "content": "详情已发送" } }));
}
```

**分段发送**：`pusher.rs` 里 `MAX_FINAL_TEXT_CHARS` 是单条上限。详情全文可能超出，
按该上限切成多条顺序发送，每条前缀 `（N/M）`。段数上限 5 段，超出部分提示
「剩余内容请到 web 端查看」——这是详情按钮的能力边界，要在文档和 toast 里讲清楚，
不能假装发全了。

回复用 `api.reply_text(&card_message_id_or_origin, ..., in_thread)`。
`in_thread` 由 value 里的 `topic_id != "main"` 推出，与既有写法一致。

- [ ] **`task_suggest` 分支**：本文档只建骨架。查 payload → 归属校验 →
      以 `payload` 为文本调 `router::dispatch_task`。

```rust
if value["action"].as_str() == Some("task_suggest") {
    // B 阶段的 suggest_actions 会往 feishu_card_actions 写 kind=suggest 的行；
    // A 阶段没有任何入口产生这种行，此分支实际走不到，但先建好以固定契约。
    ...
}
```

派发前必须构造 `IncomingMessage`（`dispatch_task` 需要它定位话题与回复目标）。
`message_id` 用卡片的 `open_message_id`，`chat_id` / `thread_id` 从 value 取。
参考现有 `answer` 分支里 `router::` 相关调用的构造方式，不要新造一套。

- [ ] 两个分支都要 `tracing::info!` 记录 operator / action_id / session，
      与现有 `answer` 分支的日志字段风格一致。

### 6. 文档

- [ ] `docs/feishu.md`「三、用法」表格里，`@机器人 帮我做 xxx` 那行的说明更新为：
      卡片标题带任务摘要，完成后可点「查看详情」在话题内获取完整总结。
- [ ] 「架构」小节的 pusher 那行补一句：终态卡按钮的 payload 存
      `feishu_card_actions`（不进 callback value，避免客户端改写指令），30 天清理。

## 明确边界

- 不动微信 pusher（`server/src/weixin/pusher.rs`、`weixin/router.rs`）。
- 不动 `crates/code-agent/`、`crates/code-tools/`——B / C 才碰。
- 不新增依赖，不改 `Cargo.toml`。
- `build_confirm_card` / `build_task_gate_card` / `build_confirm_done_card` 三个卡片
  构建器**不改**：本文档只扩展 `build_task_card`。
- `answer` 分支与 `deploy_approval` 分支的既有逻辑、顺序、toast 文案一律不动。
- 保留工作区 `quant/` 下的未提交改动，不回退。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo test -p hank-db
```

期望：编译通过；新增单测通过。特别确认：

- `pusher::spawn` 的 4 个飞书调用点全部适配新签名（`rg 'pusher::spawn' -n` 确认
  只剩 weixin 那处用旧的独立签名）。
- `build_task_title` 的单测覆盖超长中文截断（不出现半个字）与控制字符压平。
- 运行中态卡片的 `actions` 为空（可用一个断言锁住：`build_task_card` 传空 actions
  时 body 只有 1 个 element）。

**已知的既有欠账**（不要在本任务里顺手清理，会污染 diff）：
`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all -- --check`
在改动前就是失败的（`crates/code-tools/` 的 6 项 clippy、`hank-db` 的 6 项
`too_many_arguments`、`team_task/` 等 10 个文件的 fmt）。
**要求**：你新增/修改的代码本身必须是 `cargo fmt` 干净的（改完对你碰过的文件跑
`cargo fmt -p hank-server`，确认新增部分无 diff），但不要去动既有欠账。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；错误处理用 `anyhow`；日志用 `tracing`
且字段风格与文件内既有调用一致；注释写"为什么"而非"是什么"。

commit message 建议：

```
feat(feishu): 卡片标题带任务摘要，终态卡加查看详情按钮
```
