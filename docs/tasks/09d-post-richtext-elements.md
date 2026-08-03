# 09D 飞书 post 富文本元素解析补全

> 独立小任务，与 09C 无依赖关系，**可在 09C 之前或之后执行**。
> 建议先做这份（改动小、风险低），再做 09C。

## 背景与目标

`docs/book/agent-os/09_适配器模式 & 多轮对话支持.md` 指出一个真实缺陷：用户把日志或代码
粘成**飞书代码块**发给机器人时，提示词会变成空字符串。

原因在 `server/src/feishu/router.rs:113` 的 `extract_text`：post 富文本的元素只认
`text` 和 `at` 两种 tag，其余（`code_block`、`code`、`a`、`md`、`br`）走 `_ => {}` 被丢弃。

```rust
// server/src/feishu/router.rs:136 现状
match el["tag"].as_str() {
    Some("text") => { /* 取 text */ }
    Some("at")   => { /* 取 user_name */ }
    _ => {}          // ← code_block / a / md / br 全部丢掉
}
```

**后果**：用户在飞书里贴一段报错日志（飞书客户端会自动转成 `code_block` 元素）
并 @机器人，`extract_text` 返回空串，然后 `handle_message` 走到
`if msg.text.is_empty()` 分支回一句「收到，但当前只支持文字和图片消息」——
用户明明发了文字，却被告知不支持。这是**静默丢失用户输入**，比报错更糟。

**做完之后的可观察效果**：

- 飞书里贴代码块 / 引用链接 / markdown 片段并 @机器人，机器人能正常收到内容并派单，
  不再回「只支持文字和图片消息」。
- 代码块内容作为提示词的一部分交给 agent，日志和堆栈能直接粘贴排查。
- `br` 换行元素渲染成真正的换行，多行日志不会被拼成一整行。
- 纯 `at`（只 @机器人不说话）仍然按空文本处理，行为不变。

## 参考实现与本项目的差异

参考文档给的 TypeScript 实现是：

```typescript
function renderPostElement(element: PostElement): string {
  if (element.tag === "at") return element.user_id ?? "";
  if (element.tag === "br") return "\n";
  if (["text", "a", "code", "code_block", "md"].includes(element.tag ?? "")) {
    return element.text ?? "";
  }
  return "";
}
```

**本项目有两处必须保留的既有差异，不要照抄**：

1. **`at` 取 `user_name` 而不是 `user_id`**。参考实现返回 `user_id`，本项目现有代码
   取 `el["user_name"]` 并拼成 `@{name}`。本项目在 `resolve_mentions` 里还有一层
   `@_user_N` 占位符还原逻辑（`router.rs:159`），与 `user_name` 配套。
   改成 `user_id` 会让斜杠命令解析（`parse_command` 允许 `@MyBot /status` 这种前缀）
   失效，并破坏既有测试 `extract_text_from_post_message`。
2. **段落之间的连接**。参考实现是 `.join("\n")`，本项目现有代码是段落内外都直接
   `push_str` 拼接（无分隔）。改成按段落换行是**行为变更**，会影响现有测试
   `extract_text_from_post_message` 的期望值。本任务**保持现有的段落拼接方式**，
   只补元素类型——把范围严格限制在「补 tag」，不要顺带改段落语义。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `server/src/feishu/router.rs` | `extract_text` 的元素 match 补 `code_block` / `code` / `a` / `md` / `br`；补单测 |
| `docs/feishu.md` | 「三、用法」补一行说明可以粘代码块 |

**只改这两个文件。** 不许碰：`server/src/feishu/` 下的其他文件、
`server/src/weixin/`、`crates/`、`admin/`、`client/`、`team/`、`quant/`。
特别是**不要动** `resolve_mentions`、`parse_command`、`archive_inbound_content`——
它们依赖 `extract_text` 的现有输出格式。

保留工作区原有改动，不回退与本任务无关的内容。

## 实现步骤

### 1. 补元素类型

- [ ] 把 `server/src/feishu/router.rs:136` 那段 match 改为：

```rust
                for el in elements {
                    match el["tag"].as_str() {
                        // code_block / code / a / md 都带 text 字段，语义上都是用户输入的
                        // 正文。漏掉 code_block 会让「贴一段报错日志 @机器人」变成空文本，
                        // 然后被当成"不支持的消息类型"回绝——静默丢用户输入比报错更糟。
                        Some("text") | Some("a") | Some("code") | Some("code_block")
                        | Some("md") => {
                            if let Some(t) = el["text"].as_str() {
                                out.push_str(t);
                            }
                        }
                        // 富文本换行是独立元素，不补的话多行日志会被拼成一整行。
                        Some("br") => out.push('\n'),
                        Some("at") => {
                            if let Some(name) = el["user_name"].as_str() {
                                out.push_str(&format!("@{name}"));
                            }
                        }
                        _ => {}
                    }
                }
```

- [ ] 同步更新函数的文档注释（`router.rs:112`）：

```rust
/// 从消息 content 提取纯文本。
///
/// text 直接取；post 遍历富文本段落，取 text / a / code / code_block / md 的正文，
/// br 转换行，at 转 `@显示名`。代码块必须收——用户常把报错日志粘成 code_block。
```

- [ ] **不要**改 `at` 分支取 `user_id`，**不要**给段落之间加 `\n`（见上文「差异」小节）。

### 2. 补单测

在 `mod tests`（约 1964 行）里，紧跟现有的 `extract_text_from_post_message`
之后新增。现有三个 post 测试必须继续通过，不要修改它们的期望值。

- [ ] 代码块：

```rust
    #[test]
    fn extract_text_from_post_with_code_block() {
        // 用户把报错日志粘成代码块：漏掉 code_block 会让文本变空，
        // 然后被当成"不支持的消息类型"回绝。
        let content = r#"{"title":"","content":[[{"tag":"text","text":"这个报错怎么修 "},{"tag":"code_block","text":"panic at line 3"}]]}"#;
        assert_eq!(
            extract_text("post", content),
            "这个报错怎么修 panic at line 3"
        );
    }
```

- [ ] 其余元素类型与换行：

```rust
    #[test]
    fn extract_text_from_post_with_link_md_and_br() {
        let content = r#"{"title":"","content":[[{"tag":"a","text":"文档","href":"https://x"},{"tag":"br"},{"tag":"code","text":"cargo test"},{"tag":"md","text":" 看这里"}]]}"#;
        assert_eq!(extract_text("post", content), "文档\ncargo test 看这里");
    }
```

- [ ] 未知 tag 仍被忽略（锁住 `_ => {}` 的兜底行为）：

```rust
    #[test]
    fn extract_text_from_post_ignores_unknown_tag() {
        let content = r#"{"title":"","content":[[{"tag":"text","text":"前"},{"tag":"emotion","emoji_type":"SMILE"},{"tag":"text","text":"后"}]]}"#;
        assert_eq!(extract_text("post", content), "前后");
    }
```

注意最后这个测试的期望值：`emotion` 元素没有 `text` 字段，被忽略后是 `"前后"`
（中间无空格），这与「段落内直接拼接」的现有语义一致。

### 3. 文档

- [ ] `docs/feishu.md`「三、用法」表格，在「发送截图」那行**之后**插入：

```
| 粘贴代码块 / 链接 | post 富文本里的 `code_block`、`code`、`a`、`md` 都会作为正文收下，`br` 转换行——可以直接把报错日志粘进来 |
```

## 关于参考文档的另两部分：**本任务不做**

`docs/book/agent-os/09_适配器模式 & 多轮对话支持.md` 还讲了两件事，
它们在本项目**已经实现**，不需要新任务：

1. **CLI 适配层**（`CliAdapter` / `buildArgs` / `buildResumeArgs` / `parseEvent`）：
   本项目对应的是 `cli/src/agent.rs` 里按 backend 分支构造 `CommandSpec`
   （约 495 行起，codex / claude / grok / kimi 四个分支各自给出
   `program` / `args` / `write_prompt_to_stdin` / `session_id`），
   加上 `server/src/cli_agent.rs` 的 JSONL 事件解析。职责拆分与参考文档一致，
   只是用 enum 分支而非 trait object——Rust 侧没有多态调用需求，
   现在这样更直接。**不要为了对齐文档去引入 trait**。
2. **多轮对话 `--resume`**：本项目已有。`cli/src/agent.rs:510` 与 `:547` 会在
   `thread_id` 存在时 `args.extend(["--resume", thread_id])`，否则用
   `--session-id` 新建；`server/src/cli_agent.rs:2898` 把 CLI 返回的
   thread_id 写回 `session.metadata.agent_thread_id` 持久化。
   参考文档的 `cliSessionId` 对应本项目的 `metadata.agent_thread_id`，
   `setCliSessionId` 对应 `cli_agent.rs` 的 metadata 写回。
   `/status` 命令也已经能显示会话信息。

所以参考文档带来的**唯一真实缺口**就是 post 富文本元素解析，即本任务。

## 明确边界

- 只补元素 tag，**不改** `at` 的取值字段、**不改**段落拼接方式。
- 不改 `resolve_mentions` / `parse_command` / `archive_inbound_content`。
- 不引入 trait 化的 CLI 适配层（现有 enum 分支结构保持不动）。
- 不动 `--resume` 相关逻辑（已实现且在用）。
- 不新增依赖，不改 `Cargo.toml`。
- 保留工作区原有改动。**注意**：工作区当前有 09B 的未提交改动
  （`crates/code-agent/`、`crates/code-tools/`、`server/src/chat.rs`、
  `server/src/feishu/pusher.rs`、`server/src/feishu/callback.rs`、
  `server/src/interaction_flow.rs`、`docs/feishu.md`，以及新文件
  `crates/code-tools/src/suggest_actions.rs`），**一律不要回退或改动**。
  `docs/feishu.md` 你要改，但只加上面那一行，不要碰 09B 已改的两行。

## 验收标准

```bash
cargo build --workspace
cargo test -p hank-server
```

期望：编译通过。当前基线 **hank-server 227 passed / 0 failed**，
本任务新增 3 个测试，做完应为 **230 passed**。

特别确认：

- 现有三个 post 测试 `extract_text_from_post_message`、
  `extract_text_from_post_with_locale_wrapper`、`extract_text_unknown_type_is_empty`
  **全部仍通过**（期望值未被修改）。
- `cargo fmt -p hank-server -- --check 2>&1 | grep 'feishu/router.rs'` 无输出
  （该文件改动前是 fmt 干净的，改完必须保持）。

**已知既有欠账**（不要顺手清理）：`cargo clippy --workspace --all-targets -- -D warnings`
与 `cargo fmt --all -- --check` 在改动前即失败（`crates/code-tools/` clippy、
`hank-db` 的 `too_many_arguments`、`server/src/team_task/` 等文件 fmt）。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；注释写"为什么"而非"是什么"。

commit message 建议：

```
fix(feishu): post 富文本收下代码块与链接，避免贴日志被当空消息
```
