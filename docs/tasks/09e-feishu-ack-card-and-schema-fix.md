# 09E 飞书首响卡片、重复卡片与终态按钮失效修复

> 线上实测（`7795ee3` 部署后）发现的三个问题，一并修复。
> 前两个是体验问题，第三个是 09A 的**实现缺陷**——「查看详情」按钮线上从未真正出现过。

## 背景与目标

线上日志（UTC 2026-08-03 08:48~08:52）与用户截图暴露三个问题。

### 问题 1：首条卡片要等 60~90 秒才出现（体验问题）

日志时间线（同一条消息）：

```
08:48:20.944  收到消息「现在quant 的自我研究开发情况」
08:48:21.707  Sending request to Anthropic API   ← 路由 Agent 开始分类
08:49:55.268  new topic workspace decision       ← 94 秒后才拿到分类结果
08:49:56.382  Agent loop iteration 0             ← 这之后才发第一张卡
```

原因在 `server/src/feishu/router.rs::dispatch_task_content`：新话题必须先调
`create_and_map_feishu_session` → `decide_new_topic`（`router.rs:1027`），
那是一次**真实 LLM 调用**（`provider_registry::resolve_default` + Anthropic 请求）
用来决定 `agent_kind` / `agent_backend`。这次调用耗时 60~94 秒，
期间用户完全没有任何反馈——看起来像机器人没收到消息。

进度卡片是在 `pusher::spawn` 里发的（`pusher.rs:120` 附近），而 `pusher::spawn`
在 `run_chat_turn` 返回之后才调用，所以必然排在路由 LLM 之后。

### 问题 2：同一条消息产生两张一模一样的卡片（体验问题）

用户截图里 16:50 有两张完全相同的「运行中 0% 正在启动执行引擎」卡片。

日志显示飞书对**同一 `event_id`** 重复投递：

```
08:48:20.944  收到消息 event_id=89e128ac… message_id=om_x100b6831138c2cb4de20cfb48d3d83c
08:48:36.557  收到消息 event_id=89e128ac… message_id=om_x100b6831138c2cb4de20cfb48d3d83c  ← 同一 event
08:48:36.752  duplicate inbound message ignored                                          ← 被正确拦截
08:48:53.771  收到消息 event_id=57362ba5… message_id=om_x100b6831118310b0c3aff9bd94974be ← 用户重发
08:49:10.088  收到消息 event_id=57362ba5… (同上)
08:49:10.xxx  duplicate inbound message ignored
08:49:55.268  new topic workspace decision  ← 第一条的路由结果
08:50:45.204  new topic workspace decision  ← 第二条的路由结果 → 第二张卡
```

`insert_channel_message` 的去重**工作正常**（拦住了 event 级重复投递）。
真正的两张卡来自**用户在等不到响应时重复发送**（两条不同 `message_id`）：
第一条还卡在路由 LLM 里、尚未 `try_acquire` 抢到派发名额，第二条就通过了并发检查。

这是问题 1 的直接后果——修好首响延迟后此问题会大幅缓解，但**并发窗口依然存在**，
需要一并收紧。

### 问题 3：终态卡的「查看详情」按钮从未成功渲染（09A 实现缺陷）

```
08:51:50  WARN feishu: update card failed: 飞书更新卡片失败 code=230099
          msg=Failed to create card content,
          ext=ErrCode: 200861; ErrPath: ROOT -> body -> elements -> [1](tag: action);
          ErrMsg: cards of schema V2 no longer support this capability;
          ErrorValue: unsupported tag action;
```

飞书 **schema 2.0 不支持 `"tag": "action"` 元素**。09A 在
`server/src/feishu/card.rs:145` 给 `build_task_card` 加的正是这个：

```rust
elements.push(json!({ "tag": "action", "actions": buttons }));
```

后果：**成功终态卡的 `update_card` 整体失败**，卡片永远停在最后一次成功的
「运行中 XX%」状态，既看不到「已完成」也没有「查看详情」按钮。
本地单测只断言了 JSON 结构，没有真实 API 校验，所以没发现。

**为什么 `build_confirm_card` 等用同样 `tag: action` 却没报错**：需要在实现时确认。
线上日志里只有 `update card failed`，没有 `reply card failed`——即
`reply_card`（POST 新建消息）接受 `tag: action`，而 `update_card`
（PATCH `/open-apis/im/v1/messages/{id}`）拒绝。两个端点校验规则不同。
**所以本任务只改 `build_task_card`（唯一走 update_card 且带按钮的卡片），
不要动 confirm / task_gate / deployment 三个卡片构建器**——它们只经 `reply_card`
发送，改了反而可能破坏现在正常的路径。

**做完之后的可观察效果**：

- @机器人发消息后 **2 秒内**出现一张「已收到」卡片（灰/蓝色，说明正在分析任务类型），
  路由完成后同一张卡片原地更新为「运行中 0%」，不新增消息。
- 同一条消息不再产生两张卡片；用户在路由期间重复发送会收到「上一条还在处理中」提示，
  而不是起第二个 run。
- 任务完成后卡片正常变绿显示「已完成」，并出现可点的「查看详情」按钮，
  日志中不再有 `230099 / unsupported tag action`。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `server/src/feishu/card.rs` | `build_task_card` 的按钮改用 schema 2.0 支持的写法；新增「已收到」态 |
| `server/src/feishu/router.rs` | 派发前先发首响卡片；把 card_message_id 传给 pusher；收紧并发窗口 |
| `server/src/feishu/pusher.rs` | `spawn` 接受已存在的 card_id，复用而非新建 |
| `docs/feishu.md` | 用法表补首响卡片说明 |

**不许碰**：`build_confirm_card` / `build_task_gate_card` / `build_deployment_card`
（它们经 `reply_card` 发送且工作正常）、`server/src/weixin/`、`crates/`、
`admin/`、`client/`、`team/`、`quant/`。
保留工作区原有改动，不回退无关内容。

## 实现步骤

### 1. 修 schema 2.0 按钮渲染（问题 3，**最高优先级**）

- [ ] 先查飞书 schema 2.0 的正确按钮写法。**已知 `tag: action` 不被 `update_card` 接受**。
      schema 2.0 的做法是把 button 直接作为 body element，或放进
      `column_set` / `form`。推荐先试**直接作为 element**：

```rust
// schema 2.0 不支持 tag:action 容器（update_card 会报 200861）。
// 按钮直接作为 body element 平铺；多个按钮各占一行。
for a in &opts.actions {
    elements.push(json!({
        "tag": "button",
        "text": { "tag": "plain_text", "content": a.label },
        "type": if 首个 { "primary" } else { "default" },
        "width": "default",
        "behaviors": [{
            "type": "callback",
            "value": { /* 同现有 */ }
        }]
    }));
}
```

- [ ] **必须真机验证**。单测只能保证 JSON 形状，无法发现 API 拒绝。
      验证方式二选一：
      - 在飞书里真实派一个任务，等它完成，确认卡片变绿且按钮出现、日志无 `230099`；
      - 或写一个一次性脚本调 `update_card` 打一张测试卡（需要真实 token）。

      **不要只跑 `cargo test` 就宣称修好了**——这正是 09A 漏掉这个 bug 的原因。

- [ ] 若「按钮直接作为 element」仍被拒，退而用 `column_set`
      （`build_confirm_card` 的基本信息区已在用 `column_set`，说明该 tag 被支持）：
      每个按钮一个 `column`，内含 button。把最终可行的写法**连同飞书报错原文**
      写进 `card.rs` 的注释，避免以后有人改回 `tag: action`。

- [ ] 更新 `task_card_with_actions` 单测：断言 body 里**不存在** `tag == "action"`
      的元素，且按钮的 `behaviors[0].value.action` 仍正确。加一句注释说明
      「schema 2.0 的 update_card 拒绝 tag:action，这里锁住不要改回去」。

### 2. 首响「已收到」卡片（问题 1）

思路：把卡片创建**提前**到路由 LLM 之前，pusher 复用这张卡而不是新建。

- [ ] `card.rs` 的 `TaskStatus` 新增一个变体：

```rust
/// 已收到消息、正在判定任务类型（路由 Agent 分类中）。
/// 分类要调一次 LLM，实测可能 60~90s，必须先给用户反馈。
Received,
```

`style()` 返回 `("blue", "已收到")`。**注意**：`TaskStatus` 的 match 可能在多处，
编译器会指出需要补的地方。

- [ ] `router.rs::dispatch_task_content` 在**查 session 之前**（函数开头、
      拿到 `topic` 之后）先发卡：

```rust
// 首响：路由 Agent 分类要调 LLM（实测 60~90s），必须先给用户可见反馈，
// 否则用户以为机器人没收到、重复发送 → 冒出多张卡片。
// 这张卡后续由 pusher 原地更新为运行中/终态，不新增消息。
let ack_card_id = api
    .reply_card(
        &msg.message_id,
        &build_task_card(&TaskCardOptions {
            title: build_task_title(&<用户文本>),
            status: TaskStatus::Received,
            progress: 0,
            detail: "已收到，正在判断任务类型".to_string(),
            activities: vec![],
            footer: None,
            actions: vec![],
            session_id: String::new(),   // 尚未有 session
            chat_id: msg.chat_id.clone(),
            topic_id: topic.clone(),
        }),
        msg.in_thread(),
    )
    .await
    .ok();   // 发卡失败不阻断任务，退化为原有行为
```

`<用户文本>` 的取法与 09A 里 `task_title` 相同（从 content 的 Text 块拼接）。
**把 task_title 的计算移到函数开头**，首响卡和 pusher 共用同一个值。

- [ ] **失败处理**：`reply_card` 失败时 `ack_card_id = None`，后续 pusher
      按原逻辑自己新建卡片。不要因为发卡失败就中断任务。

- [ ] 斜杠命令路径（`handle_command`）**不发**首响卡——那些是同步快速返回的。

### 3. pusher 复用首响卡片（问题 1 续）

- [ ] `pusher::spawn` 与 `run` 新增参数 `existing_card_id: Option<String>`，
      放在 `task_title` 之后。参数已多，`#[allow(clippy::too_many_arguments)]` 已有。
- [ ] `run` 里原本无条件 `api.reply_card(...)` 新建卡片的那段（`pusher.rs:120` 附近）
      改为：

```rust
// 复用 router 已发的首响卡（同一张卡从「已收到」原地更新到终态，
// 不给用户多发消息）；没有则退化为自己新建。
let card_id = match existing_card_id {
    Some(id) => Some(id),
    None => match api.reply_card(&message_id, &build_task_card(...), in_thread).await {
        Ok(id) => Some(id),
        Err(e) => { /* 原有的退化为纯文本逻辑 */ }
    },
};
```

- [ ] 四个飞书 `pusher::spawn` 调用点适配新参数：
      - `feishu/router.rs`：传 `ack_card_id`
      - `interaction_flow.rs` 两处（resume 路径）：传 `None`
        （resume 不经过路由 LLM，没有首响卡）
      - `team_task/orchestrator.rs`：传 `None`
      **微信 `weixin/router.rs:925` 是另一个 pusher，不要动。**

### 4. 收紧并发窗口（问题 2）

现状：`try_acquire` 在 `create_and_map_feishu_session`（含路由 LLM）**之后**才调用，
所以路由期间到达的第二条消息会通过检查。

- [ ] 把派发名额的抢占**提前到路由之前**。注意 `try_acquire` 的 key 是 `session_id`，
      而新话题此时还没有 session。处理方式：**按话题维度**加一层抢占。

推荐做法：在 `state.tasks` 上增加一个以 `account_id:chat_id:topic_id` 为 key 的
「话题级」占位，语义是"这个话题正在初始化会话"：

```rust
// 新话题的路由分类要 60~90s，期间没有 session_id 可用于 try_acquire。
// 用话题 key 占位，避免用户重复发送时起第二个 run（线上实测会冒出两张卡）。
let topic_key = format!("{}:{}:{}", account.id, msg.chat_id, topic);
```

若 `state.tasks`（`server/src/task_state.rs`）已有可复用的通用抢占 API
（`try_acquire` 接受任意字符串 key），直接用它；否则加一个同构方法。
**先读 `task_state.rs` 确认现有 API 形状**，不要重复造。

- [ ] 抢不到时的回复：`api.reply_text(&msg.message_id, "上一条还在处理中，请稍候", ...)`。
      **不要**沿用 `running_reply(state, &session_id)`——此时还没有 session_id。
- [ ] 拿到 session 后仍走原有的 `try_acquire(&session_id)` 与 `active_tasks` 检查
      （两层保护，别删）。话题级占位要在 session 级名额拿到后释放，
      或用 RAII guard 保证异常路径也释放。
- [ ] **注意死锁风险**：话题级占位若在 `run_chat_turn` 之前不释放，
      而 session 级又在等，会互相卡住。**释放时机**：拿到 `session_id` 并成功
      `try_acquire(&session_id)` 之后立刻释放话题级占位。

### 5. 文档

- [ ] `docs/feishu.md`「三、用法」的 `@机器人 帮我做 xxx` 那行补充：
      新话题先秒回一张「已收到」卡片（路由分类需调 LLM，约 1 分钟），
      同一张卡片随后原地更新为运行中→终态，不新增消息。
- [ ] 在「架构」小节 pusher 那行补一句：首响卡由 router 创建、pusher 复用同一
      `card_message_id`。

## 明确边界

- **不改** `build_confirm_card` / `build_task_gate_card` / `build_deployment_card`。
  它们经 `reply_card`（POST）发送且线上正常；`tag: action` 只在
  `update_card`（PATCH）被拒。动它们会破坏现在可用的确认闸门。
- 不改 `insert_channel_message` 的 event 级去重（它工作正常）。
- 不改路由 Agent 本身的 prompt 或模型选择——**本任务不优化路由耗时**，
  只是不让用户干等。（路由改用更快的模型是独立议题，需要单独评估分类准确率。）
- 不动微信 pusher。
- 不新增依赖。
- 保留工作区原有改动。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo fmt --all -- --check
```

基线：**hank-server 243 passed**，本任务应只增不减。

**但编译和单测不足以验收本任务**。必须补真机验证（问题 3 就是单测漏掉的）：

- [ ] 部署后在飞书新话题 @机器人发一条消息：
      1. **2 秒内**出现「已收到」卡片
      2. 约 1 分钟后**同一张卡片**变成「运行中 0%」（不是新增一张）
      3. 任务完成后同一张卡片变绿「已完成」，出现「查看详情」按钮
      4. 点「查看详情」能收到完整总结
- [ ] `ssh wananyun journalctl -u hank-server -n 200 --no-pager | grep 230099`
      **无输出**（按钮渲染已被飞书接受）
- [ ] 在「已收到」阶段（路由还没完成时）再发一条消息，应收到
      「上一条还在处理中」文本，**不产生第二张卡片**

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；注释写"为什么"。
`card.rs` 里关于 schema 2.0 不支持 `tag: action` 的注释要写上飞书原始报错码
（`230099 / 200861`），这是防止回退的关键信息。

commit message 建议：

```
fix(feishu): 先回已收到卡片，修 schema 2.0 按钮渲染与重复卡片

路由分类要调 LLM（实测 60~90s），此前用户干等无反馈、重复发送导致两张卡；
现在秒回「已收到」卡并由 pusher 原地更新同一张卡。
另修 09A 引入的 tag:action——飞书 schema 2.0 的 update_card 拒绝该容器
（230099/200861），终态卡与「查看详情」按钮线上从未成功渲染。
```
