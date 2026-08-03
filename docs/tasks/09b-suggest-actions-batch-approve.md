# 09B suggest_actions 工具与 quant 批量同意按钮

> 三份文档的第二份。**必须先完成 09A** 并通过验收，本文档依赖 A 建立的
> `TaskCardAction` / `feishu_card_actions` / `task_suggest` 回调分支。
> 如果 A 尚未完成，停下来先做 A。

## 背景与目标

A 阶段给终态卡建好了按钮区，但只有一个「查看详情」。本文档补两件事：

1. **agent 可以在收尾时提议后续动作**。新增 `suggest_actions` 工具：agent 声明
   若干「建议下一步」，每个带自己写的一段 prompt。用户点按钮 = 以该 prompt 起新一轮。
   例如 review 完之后给出 `[补单元测试] [只修 P0]`，点一下就继续干，
   不用用户再复述一遍需求。
2. **quant 高成本确认卡加「本会话全部同意」按钮**。现在批量授权只能打字
   「确认5次」（`server/src/chat.rs` 的 `parse_quant_confirmation`，上限 50），
   卡片上没有对应按钮。

**做完之后的可观察效果**：

- agent 在最终回复前调用 `suggest_actions`，飞书终态绿卡上除「查看详情」外，
  多出最多 3 个 agent 自拟的按钮（如「补单元测试」）。点击后话题内以该 prompt
  起新一轮，出现新的蓝色进度卡。
- 高成本 quant 操作的确认卡从 2 个按钮变 3 个：`确认` / `本会话全部同意` / `否`。
  点「本会话全部同意」等价于打字「确认50次」，本会话后续高成本操作不再逐次弹卡。
- agent 不调 `suggest_actions` 时，终态卡只有「查看详情」，与 A 阶段完全一致。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `crates/code-tools/src/suggest_actions.rs` | **新文件**：`SuggestActionsTool` |
| `crates/code-tools/src/lib.rs` | 注册新模块 |
| `crates/code-agent/src/types.rs` | 新增 `AgentEvent::SuggestedActions` |
| `crates/code-agent/src/session.rs` | 拦截 `suggest_actions` 调用并 emit 事件；`quant_confirm_prompt` 加第三个选项 |
| `server/src/chat.rs` | 注册工具；`event_for_stream` 补新变体分支 |
| `server/src/feishu/pusher.rs` | 暂存 SuggestedActions，终态卡渲染建议按钮 |
| `server/src/feishu/card.rs` | quant 确认卡第三个按钮的提示文案调整 |
| `server/src/feishu/callback.rs` | 填实 A 阶段预留的 `task_suggest` 分支 |
| `docs/feishu.md` | 用法表补两条 |

**不许碰**：`server/src/weixin/`（微信不做按钮，靠既有文字路径；
`quant_confirm_prompt` 的 weixin 分支保持两个选项不变）、
`admin/`、`client/`、`team/`、`quant/`、`crates/code-tools/src/ask_user.rs`（C 阶段）。
保留工作区 `quant/` 下未提交改动，不回退。

## 实现步骤

### 1. `suggest_actions` 工具

- [ ] 新建 `crates/code-tools/src/suggest_actions.rs`，参照
      `crates/code-tools/src/ask_user.rs` 的结构（同样是「execute 是 no-op，
      真正逻辑由 agent 循环拦截」的模式）。

```rust
//! agent 在收尾时提议「下一步可以做什么」，渠道渲染成可点按钮。
//!
//! 与 ask_user 的区别：ask_user 会**中断** agent 循环等用户回答；
//! suggest_actions 不中断，只记录建议，agent 继续跑到结束。
//! 所以它不需要 tool_use_id 配对的 resume 逻辑。

pub struct SuggestActionsTool;
```

schema：

```json
{
  "type": "object",
  "properties": {
    "actions": {
      "type": "array",
      "description": "Suggested follow-up actions, at most 3. Each becomes a clickable button.",
      "items": {
        "type": "object",
        "properties": {
          "label": { "type": "string", "description": "Button text shown to the user, at most 12 characters" },
          "prompt": { "type": "string", "description": "The instruction to run when the user clicks this button. Write it as a complete, self-contained instruction — the user will not retype anything." }
        },
        "required": ["label", "prompt"]
      }
    }
  },
  "required": ["actions"]
}
```

- [ ] `execute` 返回一句确认文本（如 `"Suggested N follow-up actions"`），
      **不做业务逻辑**——真正处理在 `session.rs` 拦截。
- [ ] 描述里要讲清语义边界，避免模型误用成 ask_user：

```
Propose follow-up actions after finishing the current task. Does NOT pause execution —
use ask_user when you need an answer before continuing. Each action becomes a button;
clicking it starts a NEW turn with your `prompt` as the instruction.
```

- [ ] `crates/code-tools/src/lib.rs` 加 `pub mod suggest_actions;`（按字母序插入，
      现有列表是有序的：放在 `str_replace` 之后、`test_runner` 之前）。

### 2. 新事件 `SuggestedActions`

- [ ] `crates/code-agent/src/types.rs` 在 `AskUser` 变体附近新增：

```rust
/// Agent 提议的后续动作（不中断循环，由渠道渲染成按钮）
SuggestedActions {
    actions: Vec<SuggestedAction>,
},
```

以及配套结构（与 `AgentEvent` 同一 serde 风格，需 `Serialize` / `Deserialize` / `Clone` / `Debug`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedAction {
    pub label: String,
    pub prompt: String,
}
```

- [ ] `crates/code-agent/src/session.rs` 在工具分发处拦截。位置：`ask_user`
      检测（`if name == "ask_user"` 那段）**之后**，`loop_detector` 之前。
      关键区别——**不设 `ask_user_triggered`、不 `break`**：

```rust
// suggest_actions 不中断循环：它只是记录建议，agent 继续跑完本轮。
// 拦截而非让工具自己 execute，是因为要把结构化 actions 送进事件流。
if name == "suggest_actions" {
    let actions = parse_suggested_actions(input);
    if !actions.is_empty() {
        let _ = event_tx
            .send(AgentEvent::SuggestedActions { actions })
            .await;
    }
    tool_results.push(ContentBlock::ToolResult {
        tool_use_id: id.clone(),
        content: "已记录建议动作，将在任务结束时展示给用户".to_string(),
        is_error: false,
    });
    continue;
}
```

- [ ] 新增纯函数 `parse_suggested_actions(input: &Value) -> Vec<SuggestedAction>`
      做上限裁剪，并写单测：

```rust
/// 解析并裁剪 agent 提议的动作。
///
/// 超限截断而不报错：模型多写了几个不该让整轮失败。
/// - 最多 3 个（卡片一行放得下，加上「查看详情」共 4 个）
/// - label 最多 12 字符（飞书按钮宽度），按 chars() 截断
/// - prompt 最多 2000 字符
/// - label 或 prompt 为空的条目直接丢弃
```

单测覆盖：正常 2 条；超过 3 条只留前 3；label 超长按字符截断（含中文）；
空 label / 空 prompt 被丢弃；`actions` 不是数组时返回空 vec。

- [ ] `crates/code-agent` 内所有对 `AgentEvent` 的**穷尽 match** 都要补新分支。
      至少 `server/src/chat.rs` 的 `event_for_stream`（约 1426 行，
      `AskUser` 那一行附近）要加：

```rust
AgentEvent::SuggestedActions { .. } => "suggested_actions",
```

编译器会指出其余遗漏点，逐个补齐（可能在 `admin` 的事件落库、
`weixin/pusher.rs`、`team_task` 等处有 match）。**微信 pusher 收到该事件
直接忽略**（不渲染按钮），不要为它加文本降级。

### 3. 注册工具

- [ ] `server/src/chat.rs` 在 `t.push(Arc::new(AskUserTool::new()));`（约 421 行）
      之后追加 `SuggestActionsTool`。import 加在文件头 `ask_user::AskUserTool` 同一
      use 块内。
- [ ] **不要**注册到 `quant_research` 那个精简工具集（约 330 行
      `let mut t: Vec<Arc<dyn Tool>> = vec![Arc::new(AskUserTool::new())];`）——
      quant 研究话题是纯查询，不该提议代码动作。如果你判断该注册，
      在文档里说明理由，不要默默加。

### 4. `pusher.rs`：暂存并渲染建议按钮

- [ ] 在 `run` 的状态变量区（`let mut final_text = String::new();` 附近）新增：

```rust
// suggest_actions 可能在最终回复前若干轮就调用了，先存着，
// 等 RunComplete 时和总结一起渲染进终态卡。
let mut suggested: Vec<hank_agent::SuggestedAction> = Vec::new();
```

（实际 crate 路径按 `code-agent` 的导出名写，参照文件内既有 `AgentEvent` 的引用方式。）

- [ ] 新增事件分支：

```rust
AgentEvent::SuggestedActions { actions } => {
    // 同一轮多次调用以最后一次为准：模型改主意时不该叠加出 6 个按钮。
    suggested = actions;
}
```

- [ ] 成功终态处（A 阶段写「查看详情」payload 的那段）扩展为一次性写入
      详情 + 建议动作，共用一次 `create_feishu_card_actions`：

```rust
// 顺序即按钮顺序：建议动作在前（用户更可能点），查看详情兜底在最后。
let mut items: Vec<(String, String, String)> = suggested
    .iter()
    .map(|a| ("suggest".to_string(), a.label.clone(), a.prompt.clone()))
    .collect();
items.push(("detail".to_string(), "查看详情".to_string(), body.clone()));
```

拿到 ids 后按下标配 `TaskCardAction`：`kind=suggest` → `action: "task_suggest"`，
`kind=detail` → `action: "task_detail"`。写库失败时退化成无按钮（A 阶段已有的
`tracing::warn!` + `vec![]` 处理方式），不要让它整轮失败。

- [ ] **失败终态不渲染建议按钮**：任务失败时 agent 的建议是基于未完成的状态，
      点下去容易跑偏。保持 A 阶段的「失败卡无按钮」。

### 5. `callback.rs`：填实 `task_suggest`

A 阶段已建好分支骨架与归属校验，本阶段填派发逻辑。

- [ ] 查到 `kind=suggest` 的 payload 后，以 `payload` 为文本走
      `router::dispatch_task`。构造 `IncomingMessage` 的方式与 A 阶段
      `task_detail` 分支一致（`message_id` 用卡片 `open_message_id`，
      chat/thread 从 value 取）。
- [ ] **校验 kind**：`task_suggest` 只接受 `kind == "suggest"` 的行，
      `task_detail` 只接受 `kind == "detail"`。混用直接回 toast 拒绝——
      否则伪造 action_id 可以让「查看详情」的全文被当 prompt 执行。
- [ ] 派发前查并发：该 session 正在跑任务时，`dispatch_task` 内部已有名额判定，
      但要给用户一个明确 toast（「任务正在执行中，请稍候」）而不是静默失败。
      参照 `router.rs` 里既有的"任务在跑时的回复"逻辑。
- [ ] 点击后回 toast「已开始执行」，新一轮的进度卡由 `pusher::spawn` 自然产生
      （标题会是该 prompt 的摘要，A 阶段的 `build_task_title` 自动生效）。
- [ ] **不改写原卡**：原终态卡保持原样，按钮可再点（用户可能想连着点两个建议）。
      如果你认为该置灰，在文档里说明，不要默默改。

### 6. quant 确认卡第三个按钮

- [ ] `crates/code-agent/src/session.rs` 的 `quant_confirm_prompt`（约 39 行）
      **非 weixin 分支**的 options 从 `["确认","否"]` 改为
      `["确认", "本会话全部同意", "否"]`。

**为什么必须改这里而不只改卡片**：options 会写进交互单的 `options` 列，
admin 手动应答走白名单校验（`server/src/interactions.rs` 的
`if !options.iter().any(|o| o == answer)`）。只在卡片加按钮而不改 options，
admin 侧会拒绝这个答案，两条路径就分叉了。

- [ ] **weixin 分支保持两个选项不动**（微信不支持批量授权，
      `parse_quant_confirmation` 对 weixin 已有专门处理，且有单测
      `test_parse_quant_confirmation_weixin_no_batch` 锁着）。

- [ ] `server/src/chat.rs` 的 `parse_quant_confirmation`（约 1356 行）要能识别
      `"本会话全部同意"` 并等价于 50 次授权：

```rust
// 飞书卡片按钮的文案，等价于「确认50次」。
// 与打字路径共用同一个 grant 上限，不引入第二套配额语义。
if text.trim() == "本会话全部同意" {
    return (50, "用户已确认批量授权本会话后续高成本量化操作".to_string());
}
```

放在现有「确认N次」解析之前。**weixin source 必须排除**（与既有 weixin 禁止批量
的逻辑一致）——照 `source` 参数判断，别让微信用户打这六个字绕过限制。

- [ ] 补单测：`parse_quant_confirmation("本会话全部同意", "feishu")` 返回 50；
      同样输入 `source="weixin"` 时**不**返回 50（按 weixin 既有语义处理）。

- [ ] `server/src/feishu/pusher.rs` 的 quant 分支 hint 文案（约 370 行
      `Some("点击按钮或回复文字作答；回复「确认N次」…")`）更新：既然有按钮了，
      提示改为说明按钮含义 + 「不同意可直接回复你的意见」。
      `server/src/interaction_flow.rs` 的 `confirm_card_from_interaction` 里
      有一份**重复的** hint 文案（卡片恢复路径），两处要同步改，否则
      派发失败恢复卡片后提示会变回旧文案。

### 7. 文档

- [ ] `docs/feishu.md`「三、用法」表补两行：
  - 终态卡的 agent 建议按钮：点击以该建议为指令起新一轮
  - 高成本 quant 确认：三个按钮，「本会话全部同意」等价「确认50次」
- [ ] 「高成本 quant 工具」那行原文提到「文字回复『确认5次』可批量授权」，
      更新为按钮 + 文字两种方式。

## 明确边界

- `suggest_actions` **不中断** agent 循环。不要给它加 `ask_user_triggered`
  或 resume 逻辑——那会让 agent 卡住等一个不存在的回答。
- 微信渠道不渲染建议按钮，收到 `SuggestedActions` 事件直接忽略。
- 不改 `ask_user` 工具（C 阶段）。
- 不改 `build_confirm_card` 的按钮生成逻辑：它已经按 `choices` 数组渲染，
  options 从 2 个变 3 个会自动多一个按钮，**不需要**特殊处理。
- 不新增依赖。
- 保留工作区 `quant/` 下未提交改动。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
cargo test -p code-agent
cargo test -p code-tools
```

期望：编译通过，测试全绿。特别确认：

- `parse_suggested_actions` 的裁剪单测通过（含中文 label 截断不出现半个字）。
- `parse_quant_confirmation("本会话全部同意", …)` 的新单测通过，且既有的
  `test_parse_quant_confirmation_weixin_no_batch` 仍通过。
- `AgentEvent` 新变体在所有穷尽 match 处已补齐（编译通过即证明）。
- 手动确认：`grep -n '本会话全部同意' -r server/ crates/` 应命中
  `session.rs`（options）与 `chat.rs`（解析），两处一致。

**已知既有欠账**（不要顺手清理）：`cargo clippy --workspace --all-targets -- -D warnings`
与 `cargo fmt --all -- --check` 在改动前即失败（`code-tools` 6 项 clippy、
`hank-db` 6 项 `too_many_arguments`、`team_task/` 等 10 文件 fmt）。
你新增的代码本身要 fmt 干净（对碰过的 crate 跑 `cargo fmt -p <crate>` 确认新增部分无 diff）。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；`anyhow` 错误处理；`tracing` 日志。
新工具的 schema description 用英文（与 `ask_user.rs` 等既有工具一致，
那是给模型看的），Rust 侧注释用中文。

commit message 建议：

```
feat(feishu): agent 可提议后续动作，quant 确认加批量同意按钮
```
