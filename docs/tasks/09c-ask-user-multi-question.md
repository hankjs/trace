# 09C ask_user 多问题作答

> 三份文档的第三份，**最复杂的一份**。必须先完成 09A 与 09B 并通过验收。
> 本文档引入「部分应答」中间态，是三份里唯一改动交互单状态机的。

## 背景与目标

现在 `ask_user` 是**单问题单选**：`{question, options}`，飞书渲染成一行按钮，
一次答一个。用户想要的是多问题批量作答——agent 一次问两三个问题，
用户回「1A 2B」或逐题点按钮。

**做完之后的可观察效果**：

- agent 调 `ask_user` 传新的 `questions` 数组时，飞书出一张多问题卡：
  每题一行题干 + 一组按钮（按钮文案形如 `1A main`）。
- 逐题点击：点第 1 题后卡片原地刷新，已答的题显示为「✓ 1A main」，
  未答的题按钮仍可点。**所有题答完**才真正应答交互单并 resume agent。
- 文字作答：用户直接回「1A 2B」一次答完。格式错误（题号不存在、选项越界、
  漏题）会得到明确提示，不会把垃圾串塞给模型。
- **旧的单问题调用完全不受影响**：`{question, options}` 照原样工作，
  网页端与微信端行为不变。

## 关键设计决定（先读完再动手）

### 1. schema 向后兼容，不新增工具

`ask_user` 的 `question` / `options` 与新的 `questions` 都是可选，但**至少要有一组**。
两组都传时以 `questions` 为准（并记一条 warn，说明模型用法有歧义）。

### 2. `options` 列存扁平答案全集，结构存 `resume_ref`

交互单的 `options` 列（JSON 数组）现在存 `["A","B"]`，被 admin 手动应答的白名单
校验依赖（`server/src/interactions.rs`）。**不要改这一列的语义**。

多问题时：
- `options` 列存**扁平的合法答案全集**：`["1A","1B","2A","2B"]`
- 多问题结构存 `resume_ref.questions`
- 已累积的部分答案存 `resume_ref.partial_answers`（对象，如 `{"1":"A"}`）

这样 `answer_interaction` 的原子性、admin 白名单校验都不用改。

### 3. `answer` 列是 `VARCHAR(64)`，多问题完整答案串可能超长

`crates/hank-db/src/lib.rs:1325` 是 `answer VARCHAR(64)`。
`"1A 2B 3C"` 这种短串没问题，但题目多时会超。

**处理**：完整作答串写入 `answer` 前按 60 字符截断，**完整版另存
`resume_ref.final_answer`**，resume 时读 `resume_ref` 那份。
不要改列宽（改列宽要迁移，且 64 对单问题场景够用）。
在代码注释里写明这个取舍。

### 4. 逐题点击不能走现有 `answer` 回调分支

`answer_interaction` 是一次性原子应答（`WHERE status='pending'` → `answered`）。
点第 1 题就调它，会把整单标 answered 并 resume agent，剩下的题就丢了。

**所以多问题卡片的按钮用新 action `answer_multi`**，回调时：
1. 把该题选择累积到 `resume_ref.partial_answers`
2. 检查是否所有题都答完
3. **未答完**：`update_card` 刷新卡片，回 toast「已记录第 N 题」，**不**动交互单状态
4. **已答完**：拼出完整作答串，走**现有的** `answer_and_resume`（复用抢名额→
   claim→应答→改卡→派发的完整顺序，不要另写一套）

### 5. 并发点击的原子性

两个人同时点不同题、或同一人快速点两次，`partial_answers` 会互相覆盖
（读-改-写竞态）。

**处理**：新增的 DB 方法用**条件更新**做乐观并发，而不是先读后写：

```sql
UPDATE agent_interactions
   SET resume_ref = JSON_SET(resume_ref, ?, ?), updated_at = NOW()
 WHERE id = ? AND status = 'pending'
```

MySQL 的 `JSON_SET` 在单条 UPDATE 内是原子的，路径形如 `$.partial_answers."1"`。
**注意**：JSON 路径里的题号要做白名单校验（只允许 `resume_ref.questions` 里
真实存在的 id），否则是 JSON 路径注入。

## 涉及文件清单

| 文件 | 要改什么 |
|------|----------|
| `crates/code-tools/src/ask_user.rs` | schema 加可选 `questions` |
| `crates/code-agent/src/types.rs` | `AskUser` 事件加可选多问题字段 |
| `crates/code-agent/src/session.rs` | 解析 `questions`，emit 时带上 |
| `crates/hank-db/src/lib.rs` | `set_interaction_partial_answer` + 读取辅助 |
| `server/src/chat.rs` | 落表时写扁平 options 与 `resume_ref.questions`；文字「1A 2B」解析 |
| `server/src/feishu/card.rs` | `build_multi_question_card` |
| `server/src/feishu/pusher.rs` | AskUser 分支识别多问题走新卡 |
| `server/src/feishu/callback.rs` | 新增 `answer_multi` 分支 |
| `docs/feishu.md` | 用法与边界说明 |

**不许碰**：`server/src/weixin/`（微信走文字路径，自动获得能力，无需改动）、
`admin/`、`client/`、`team/`、`quant/`。
保留工作区 `quant/` 下未提交改动。

## 实现步骤

### 1. 工具 schema 扩展

- [ ] `crates/code-tools/src/ask_user.rs` 的 `input_schema` 增加 `questions`，
      并把 `required` 去掉（改为在解析侧校验「至少一组」）：

```json
"questions": {
  "type": "array",
  "description": "Ask several questions at once. Each needs a short id (\"1\", \"2\"), the question text, and its options. Use this instead of question/options when you need multiple answers. At most 5 questions, at most 4 options each.",
  "items": {
    "type": "object",
    "properties": {
      "id": { "type": "string" },
      "question": { "type": "string" },
      "options": { "type": "array", "items": { "type": "string" } }
    },
    "required": ["id", "question", "options"]
  }
}
```

- [ ] `description` 补一句：`Provide either question+options (single) or questions (multiple).`

### 2. 事件与 agent 循环

- [ ] `crates/code-agent/src/types.rs` 的 `AskUser` 变体新增：

```rust
/// 多问题作答。为空表示单问题（走 question / options）。
#[serde(default, skip_serializing_if = "Vec::is_empty")]
questions: Vec<AskUserQuestion>,
```

配套结构（`Serialize` / `Deserialize` / `Clone` / `Debug`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestion {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
}
```

**注意**：`AskUser` 有多个构造点（`session.rs` 的 quant 确认分支与 ask_user 分支），
两处都要补这个字段（quant 确认传空 vec）。

- [ ] `session.rs` 的 `ask_user` 分支新增解析，并写纯函数 + 单测：

```rust
/// 解析多问题。裁剪规则（超限截断不报错，同 suggest_actions 口径）：
/// 最多 5 题、每题最多 4 个选项；id 去重（重复的丢弃后者）；
/// id / question / options 任一为空的题丢弃。
fn parse_ask_user_questions(input: &Value) -> Vec<AskUserQuestion>
```

- [ ] **id 必须是安全字符**：只允许 `[A-Za-z0-9_-]`，长度 ≤ 8。
      这是因为 id 会进 MySQL JSON 路径（`$.partial_answers."1"`）与飞书 callback value。
      不合规的 id 直接丢弃该题，并记 warn。
- [ ] 两组都空时保持原有行为（emit 空 options 的单问题，现有代码即如此）。

### 3. DB：部分答案累积

- [ ] `crates/hank-db/src/lib.rs` 新增：

```rust
/// 记录多问题交互单的一题答案（乐观并发：JSON_SET 在单条 UPDATE 内原子，
/// 避免「读 resume_ref → 改 → 写回」的丢更新）。
///
/// `question_id` 必须由调用方按 resume_ref.questions 白名单校验过——
/// 它会拼进 JSON 路径，未校验即为 JSON 路径注入。
///
/// 返回 false 表示交互单已不是 pending（被抢答/取消/过期），调用方据此回 toast。
pub async fn set_interaction_partial_answer(
    &self,
    id: &str,
    question_id: &str,
    answer: &str,
) -> Result<bool>
```

实现要点：

```rust
// 路径用 JSON_UNQUOTE(JSON_EXTRACT(...)) 读回时才好比对；写入用 JSON_SET。
// resume_ref 可能为 NULL：先 COALESCE 成 '{}' 再 JSON_SET，否则 JSON_SET(NULL,…) 返回 NULL
// 会把整个 resume_ref 抹掉（连 tool_use_id 一起丢），后续 resume 直接坏掉。
let path = format!("$.partial_answers.\"{question_id}\"");
db_retry!(sqlx::query(
    "UPDATE agent_interactions
        SET resume_ref = JSON_SET(COALESCE(resume_ref, '{}'), ?, ?), updated_at = NOW()
      WHERE id = ? AND status = 'pending'"
)...)
```

`question_id` 里的 `"` 和 `\` 即使经过白名单也建议再 reject 一次（白名单已限
`[A-Za-z0-9_-]`，这里做防御性断言即可）。

- [ ] 返回 `rows_affected() == 1`。

### 4. `chat.rs`：落表与文字解析

- [ ] **落表**（约 700-750 行 `create_interaction` 那段）：`questions` 非空时
      - `options` 列写**扁平全集**：`["1A","1B","2A","2B"]`（`{id}{选项序号字母}`，
        选项序号按 A/B/C/D 顺序，与卡片按钮文案一致）
      - `resume_ref` 增加 `questions` 字段（原样存结构）
      - `title` 仍取第一题题干首行（现有逻辑复用）

- [ ] **扁平化规则要写成纯函数并单测**（飞书卡片、文字解析、白名单校验三处共用，
      不能各写一遍）：

```rust
/// 多问题的合法答案全集：每题每选项一个 token，形如 "1A"。
/// 选项超过 26 个的题按前 26 个处理（A-Z 用尽）——实际上限是 4，不会触发。
pub fn flatten_question_options(questions: &[AskUserQuestion]) -> Vec<String>

/// 解析用户的文字作答，如 "1A 2B" / "1a2b" / "1A，2B"。
/// 返回 Ok(题号→选项文案) 或 Err(给用户看的中文错误)。
///
/// 容错：忽略大小写、允许中英文逗号/空格/无分隔符；
/// 拒绝：未知题号、选项越界、有题未作答（错误信息要指出缺哪题）。
pub fn parse_multi_answer(
    questions: &[AskUserQuestion],
    text: &str,
) -> Result<Vec<(String, String)>, String>
```

单测覆盖：`"1A 2B"`、`"1a2b"`（无分隔+小写）、`"1A，2B"`（中文逗号）、
漏第 2 题（错误信息含「第 2 题」）、题号 `3` 不存在、选项 `1C` 越界（该题只有 A/B）、
完全无法解析的乱串。

- [ ] **文字应答路径接入**（约 494-520 行 `if let Some(ref pending_json)` 那段）：
      `pending` 的 `resume_ref.questions` 非空时，先用 `parse_multi_answer` 校验。
      - 校验失败：**不调** `answer_interaction`，把错误提示作为 tool_result 内容
        回给模型？**不行**——那会让模型自己乱猜。正确做法是走现有
        `answer_blocked` 机制（那段已有「返回可读文案且不应答」的先例），
        把提示直接回给用户，交互单保持 pending 等重答。
      - 校验通过：拼完整作答串（`"1A 2B"`）调 `answer_interaction`，
        `resume_ref.final_answer` 写完整版（见「关键设计决定 3」），
        tool_result 内容用**人类可读**形式（`"用哪个分支？→ main；要跑测试吗？→ 要"`），
        而不是 `"1A 2B"`——模型需要知道选了什么，不是选了第几个。

### 5. 飞书多问题卡片

- [ ] `server/src/feishu/card.rs` 新增 `build_multi_question_card`：

```rust
pub struct MultiQuestionCardOptions {
    pub interaction_id: String,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub questions: Vec<AskUserQuestion>,
    /// 已作答：题号 → 选项文案。渲染成 ✓ 行，该题不再出按钮。
    pub answered: std::collections::HashMap<String, String>,
    pub admin_url: Option<String>,
}
```

渲染：每题一个 markdown（题干）+ 该题一组按钮；已答的题不出按钮，
markdown 显示 `✓ 1A main`。底部提示「可点按钮逐题作答，或直接回复「1A 2B」一次答完」。

按钮 callback value：

```json
{
  "action": "answer_multi",
  "interaction_id": "...",
  "question_id": "1",
  "choice": "main",
  "choice_token": "1A",
  "session_id": "...", "chat_id": "...", "topic_id": "..."
}
```

- [ ] 单测：3 题全未答时按钮组数 == 3；1 题已答时该题无 action 元素且含 `✓`；
      全答完时无任何 action 元素。

- [ ] `server/src/feishu/pusher.rs` 的 `AskUser` 分支：在 `is_task_gate` 判断之后、
      普通 confirm 卡之前插入多问题判断（`questions` 非空 → 走新卡，`answered` 传空 map）。
      落卡后照现有逻辑 `set_interaction_card`。

### 6. `callback.rs`：`answer_multi` 分支

- [ ] 新分支放在 `answer` 判断之前。流程：

1. 绑定校验（复用现有逻辑）
2. 读交互单，取 `resume_ref.questions`
3. **白名单校验** `question_id` 在 questions 内、`choice_token` 在
   `flatten_question_options` 结果内——不通过直接 toast 拒绝
4. `set_interaction_partial_answer`，返回 false 时 toast「这张卡已失效」
5. 重读交互单，比对 `partial_answers` 是否覆盖所有题
6. **未答完**：`build_multi_question_card`（带最新 `answered`）→ `update_card`
   → toast「已记录，还剩 N 题」
7. **已答完**：拼完整串 → 调 `interaction_flow::answer_and_resume`
   （带 `ChannelCardContext`，与现有 `answer` 分支同样的构造方式）

- [ ] 第 7 步之后**不要**再自己 `update_card`——`answer_and_resume` 步骤④
      会把卡片改成终态（这是 A 之前那两轮已经统一好的路径）。
- [ ] `answer_and_resume` 的 options 白名单：完整串 `"1A 2B"` **不在**
      `options` 列里（那里是 `["1A","1B",...]`）。飞书路径不校验 options
      （只有 admin 路径校验），所以能过。但为了 admin 也能手动应答，
      **把完整作答串也加进 `options` 列**——即落表时 options =
      扁平全集 + 一个占位说明。**这里要你判断**：
      更干净的做法是 admin 侧对多问题单跳过白名单校验（改
      `server/src/interactions.rs`）。**选后者**，并在 `interactions.rs`
      加注释说明多问题单的合法答案是组合而非枚举，逐一枚举会组合爆炸。

### 7. 边界与文档

- [ ] **重启后的部分应答**：`expire_stale_interactions` 的僵尸判定只认
      `pending`/`answered`/`executing`。部分应答的单停在 `pending`，
      `resume_ref.partial_answers` 已持久化，卡片还在飞书侧——重启后继续点
      仍然有效（因为状态全在 DB）。**这是设计意图，在 `docs/feishu.md` 写明**。
      唯一丢失的是进程内的 `active_tasks`，与部分应答无关。
- [ ] `docs/feishu.md` 补：多问题作答的两种方式、逐题点击的中间态、
      「1A 2B」格式与容错、部分应答跨重启有效。

## 明确边界

- 不改 `options` 列的语义（仍是 JSON 数组），不改列宽。
- 不改 `answer_interaction` / `revert_interaction_to_pending` 等既有 DB 方法。
- 单问题路径（`question` + `options`）行为必须**零变化**——
  网页端与微信端不做任何适配。
- 微信收到多问题 AskUser 时：`weixin/pusher.rs` 会走它自己的文本渲染。
  **只需确认它不 panic**（多问题时 `options` 是扁平 token，它会当普通选项打印）。
  不要为微信做专门的多问题渲染，那是后续任务。
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

- `parse_multi_answer` 的容错单测全通过（大小写、无分隔、中文逗号、漏题、越界）。
- `flatten_question_options` 与卡片按钮文案、白名单校验三处用的是同一个函数
  （`rg 'flatten_question_options' -n` 应有 3+ 处调用）。
- `parse_ask_user_questions` 拒绝不安全 id（含 `"`、`.`、超长）的单测通过。
- 单问题回归：现有 `ask_user` 相关测试全部仍通过。
- JSON 路径注入防护：给 `set_interaction_partial_answer` 传
  `question_id = "1\".injected"` 时不会写坏 resume_ref（可写单测直接断言
  白名单函数拒绝该输入，不必真连库）。

**已知既有欠账**（不要顺手清理）：clippy 与 fmt 在改动前即失败
（`code-tools` 6 项、`hank-db` 6 项 `too_many_arguments`、`team_task/` 等 10 文件 fmt）。
新增代码本身要 fmt 干净。

## 约定

遵循 `CLAUDE.md`：中文注释与 commit message；`anyhow`；`tracing`。
工具 schema description 用英文，Rust 注释用中文。

commit message 建议：

```
feat(feishu): ask_user 支持多问题，可逐题点选或回复「1A 2B」
```
