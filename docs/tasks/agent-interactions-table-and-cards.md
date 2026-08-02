# 任务 A：交互单落表 + 卡片补 task_id（三份任务的第一份）

## 拆分说明（先读这段）

本次改造共三份文档，**执行顺序 A → B → C**，一次只做一份：

| 文档 | 内容 | 依赖 |
|---|---|---|
| **A（本文）** | 新建 `agent_interactions` 表 + hank-db CRUD；把现有 `quant_confirm` 与 `ask_user` 从「进程内 map / session 字段」迁到表；卡片补 task_id 与 admin 深链 | 无 |
| B | admin `/interactions` 页面：列表、详情、手动应答、取消 | A |
| C | 两阶段任务闸门：第一轮只分析 → 大卡片 → 点「开始修」resume thread 跑第二轮 | A、B |

本文只做 A。**不要**在本文里实现两阶段闸门或 admin 页面。

## 一、背景与目标

### 现状：三套并行的交互机制，只有一套落了表

| 机制 | 存储位置 | 有稳定 ID | admin 可见 |
|---|---|---|---|
| deployment 审批 | `deployments` 表 + 状态机 | 有 | 否 |
| `quant_confirm` 高成本闸门 | **进程内 map**（`QuantPendingConfirmStore`），5 分钟 TTL，重启即失效 | **无** | 否 |
| 普通 `ask_user` | `sessions.pending_ask_user` 字段 | **无** | 否 |

后两者都以 `session_id` 为 key（`crates/code-tools/src/quant_grant.rs:80/86/94`、
`server/src/chat.rs:687/1234/1250`），而 `session_id` 的生命周期由飞书话题复用策略
（`router.rs:1528` `reuse_policy_for_session_metadata`）决定。

**这已经导致一个实测确认的阻塞缺陷**：`quant_research` 会话的 metadata 既无
`agent_location` 也无 `server_agent`（后者是故意不写的），落到
`SessionReusePolicy::Recreate` —— 每条后续消息都会删掉 `feishu_chats` 映射并新建
session。于是用户点确认卡片的「确认」时，答案投给了一个全新 session，**待确认单和
授权全部成为孤儿，回测永远不会执行**。

临时探针实测（已回滚）：

```text
RESEARCH POLICY = Recreate
CONV POLICY     = Recreate
CONV+SA POLICY  = ReuseManaged
```

曾考虑过让 `quant_research` 走 `ReuseManaged` 来绕过，但那只是把 session 摁住不让它
变，**没有根治**：只要交互状态寄生在 session 上，任何会话生命周期变化都会再次踩雷。
本任务的做法是让交互单拥有自己的主键与持久化，从结构上消除这类缺陷。

### 目标

1. 新建 `agent_interactions` 表，交互单成为一等实体，有稳定 `id`。
2. `quant_confirm` 与 `ask_user` 的待确认状态从进程内 map / session 字段迁到表。
3. 恢复执行所需的上下文冻结在交互单的 `resume_ref` 上，不再从 session metadata 现读。
4. 飞书卡片展示 task_id、session_id 与 admin 深链。

### 做完之后的可观察效果

1. 飞书里触发高成本 quant 操作 → 确认卡片上能看到 `任务编号`、`会话` 与 admin 深链。
2. 点「确认」后回测真正执行 —— 即使这期间 session 被重建，交互单仍能定位。
3. server 重启后未应答的交互单**仍在表里**（不再凭空消失），状态为 `pending`。
4. 直接查库 `SELECT * FROM agent_interactions` 能看到完整交互历史。
5. 微信与 Trace 网页聊天的确认行为不变（同一套表，`channel` 区分）。

## 二、涉及文件清单

| 文件 | 改什么 |
|---|---|
| `crates/hank-db/src/lib.rs` | 建表 SQL + `AgentInteraction` struct + CRUD |
| `server/src/chat.rs` | `quant_confirm` / `ask_user` 待确认单改为写表；`resolve_pending_ask_user` 改为读表 |
| `server/src/feishu/card.rs` | `ConfirmCardOptions` 增加 `interaction_id` / `session_id` / `admin_url`，卡片正文渲染基本信息区 |
| `server/src/feishu/pusher.rs` | 构造确认卡片时传入新字段 |
| `server/src/feishu/callback.rs` | 按钮回调携带 `interaction_id`，按 id 而非 session 定位交互单 |
| `server/src/config.rs` | 新增 `[server] admin_base_url`（生成深链用，可选） |
| `config.example.toml` | 补 `admin_base_url` 注释 |
| `docs/feishu.md` | 「确认闸门升级」段落改为描述交互单落表 |

**不许碰**：

- `crates/hank-db` 里 `deployments` 表与其 CRUD（独立语义，保留不动，见下）
- `server/src/deployment.rs`
- `crates/code-agent/src/session.rs`（闸门触发逻辑正确，不动）
- `admin/`（是任务 B）
- `server/src/cli_agent.rs`（是任务 C）
- `quant/`、`client/`

**保留工作区原有改动**：`docs/tasks/cargo-fmt-whole-project.md` 是未跟踪文件，不要删除或提交。

**无需考虑历史兼容**：用户已明确不要兼容。旧的 `sessions.pending_ask_user` 字段
**保留列不删**（避免动 schema 引发连带改动），但代码不再读写它。进程内的
`QuantPendingConfirmStore` 直接废弃删除。

## 三、实现步骤

### 1. hank-db：建表

在 `crates/hank-db/src/lib.rs` 的建表序列里（紧跟 `deployments` 建表之后，
约 `lib.rs:1099` 处）加：

```rust
        // Agent 交互单：确认闸门 / ask_user / 任务闸门的统一载体。
        //
        // 为什么要落表：此前 quant_confirm 存进程内 map、ask_user 存 sessions 字段，
        // 两者都以 session_id 为 key。飞书话题会话一旦重建（reuse policy 判 Recreate），
        // 待确认单与授权立刻成为孤儿，用户点了确认却永远不会执行。交互单有自己的主键，
        // 恢复执行所需上下文冻结在 resume_ref 里，session 怎么变都不影响。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agent_interactions (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                channel VARCHAR(32) NOT NULL,
                account_id VARCHAR(36) DEFAULT NULL,
                chat_id VARCHAR(128) DEFAULT NULL,
                topic_id VARCHAR(128) DEFAULT NULL,
                kind VARCHAR(32) NOT NULL,
                title VARCHAR(255) NOT NULL,
                goal TEXT DEFAULT NULL,
                analysis MEDIUMTEXT DEFAULT NULL,
                options JSON NOT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'pending',
                answer VARCHAR(64) DEFAULT NULL,
                answered_by VARCHAR(36) DEFAULT NULL,
                answered_at DATETIME DEFAULT NULL,
                resume_ref JSON DEFAULT NULL,
                card_message_id VARCHAR(256) DEFAULT NULL,
                expires_at DATETIME DEFAULT NULL,
                result MEDIUMTEXT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_ai_status (status, updated_at),
                INDEX idx_ai_session (session_id, created_at),
                INDEX idx_ai_user (user_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;
```

字段语义（写进注释，别只写在文档里）：

| 字段 | 说明 |
|---|---|
| `id` | 交互单主键，即卡片上展示的「任务编号」 |
| `session_id` | 关联会话，**仅用于回溯与派发**，不再是身份来源 |
| `kind` | `quant_confirm` / `ask_user`（本任务两种；`task_gate` 留给任务 C） |
| `options` | 按钮文案数组，如 `["确认","否"]` |
| `status` | `pending` / `answered` / `executing` / `done` / `failed` / `expired` / `cancelled` |
| `resume_ref` | 恢复执行所需上下文。`quant_confirm` 与 `ask_user` 存 `{"tool_use_id":"…","source":"…"}` |
| `expires_at` | `NULL` = 不过期。微信写 `now + 5min`；飞书与网页写 `NULL` |
| `goal` / `analysis` | 本任务留空，任务 C 的 `task_gate` 才会用 |

`expires_at` 允许 NULL 是有意为之：5 分钟 TTL 是**微信渠道特性**
（`chat.rs` 的 `quant_confirm_expired` 只对 weixin 生效），飞书卡片没有理由过期。
把 TTL 从「写死在代码里」变成「按渠道写进这一行」。

### 2. hank-db：struct 与 CRUD

`AgentInteraction` struct 放在 `Deployment`（`lib.rs:143`）附近，字段顺序与表一致，
`#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]`。
`options` / `resume_ref` 用 `String`（JSON 文本），与 `Deployment.targets` 的既有做法一致。

CRUD 照 `create_deployment` / `get_deployment` / `approve_deployment`
（`lib.rs:3937`~`4030`）的风格实现，**必须用 `db_retry!` 宏包裹**：

```rust
pub async fn create_interaction(&self, input: NewInteraction<'_>) -> Result<AgentInteraction>
pub async fn get_interaction(&self, id: &str) -> Result<Option<AgentInteraction>>
pub async fn set_interaction_card(&self, id: &str, card_message_id: &str) -> Result<()>
/// 原子应答：仅 pending 且未过期时成功，防重复点击。返回 None 表示已被抢答或过期。
pub async fn answer_interaction(&self, id: &str, answer: &str, answered_by: &str)
    -> Result<Option<AgentInteraction>>
pub async fn update_interaction_status(&self, id: &str, status: &str,
    result: Option<&str>, error: Option<&str>) -> Result<()>
/// 某会话最近一条 pending 交互单（渠道回文本消息而非点按钮时用）
pub async fn latest_pending_interaction(&self, session_id: &str)
    -> Result<Option<AgentInteraction>>
/// 进程重启收尾：把已过期的 pending 收成 expired，返回条数
pub async fn expire_stale_interactions(&self) -> Result<u64>
```

`create_deployment` 的入参已经有 11 个，本表字段更多。**用一个
`NewInteraction<'a>` 入参 struct**，不要堆 15 个位置参数（clippy 的
`too_many_arguments` 在本仓库已有 7 处告警，不要再加）。

`answer_interaction` 的 SQL 必须是原子条件更新，照 `approve_deployment` 的写法：

```sql
UPDATE agent_interactions
   SET status = 'answered', answer = ?, answered_by = ?, answered_at = NOW(), updated_at = NOW()
 WHERE id = ? AND status = 'pending'
   AND (expires_at IS NULL OR expires_at > NOW())
```

`rows_affected() == 0` → 返回 `Ok(None)`，调用方据此回「这个操作已经提交过了」
或「已超时」。注意 `expires_at IS NULL` 分支——不能写成 `expires_at > NOW()`，
那会让飞书的不过期交互单永远无法应答。

`expire_stale_interactions` 在 server 启动时调一次，位置与
`fail_stale_running_job_runs`（`server/src/scheduler/mod.rs:92`）同一处理时机；
但**不要塞进 scheduler**（它受 `scheduler_enabled` 开关约束，关了就不跑）。
放 `main.rs` 启动序列里，与其他一次性收尾同级。

### 3. chat.rs：待确认单改为写表

删除 `QuantPendingConfirmStore` 的使用（`chat.rs:687`、`1223`、`1234`、`1250`），
以及 `AppState.quant_pending_confirms` 字段（`main.rs:74`、`198`）。
`crates/code-tools/src/quant_grant.rs` 里的 `QuantPendingConfirmStore`
一并删除（`QuantGrantStore` **保留**，会话级授权计数仍是进程态，符合设计）。

`AgentEvent::AskUser` 的处理分支（`chat.rs:673` 附近）改为统一落表：

```rust
AgentEvent::AskUser { question, options, tool_use_id, kind } => {
    // 两类 ask_user 统一落 agent_interactions：此前 quant_confirm 走进程内 map、
    // 普通 ask_user 走 sessions 字段，都以 session_id 为 key，会话重建即丢单。
    let (interaction_kind, source) = match kind.as_deref() {
        Some(k) if k.starts_with("quant_confirm:") => (
            "quant_confirm",
            k.strip_prefix("quant_confirm:").unwrap_or("").to_string(),
        ),
        _ => ("ask_user", channel_source.clone()),
    };
    // 微信 5 分钟 TTL 是渠道特性；飞书/网页不过期。
    let expires_at = (source == "weixin")
        .then(|| chrono::Utc::now() + chrono::Duration::minutes(5));
    // …create_interaction，resume_ref = {"tool_use_id":…,"source":…}
}
```

`resolve_pending_ask_user`（`chat.rs:1215` 附近）改为
`db.latest_pending_interaction(session_id)`，返回结构保持现有调用方所需的形状
（`tool_use_id` / `question` / `options` / `kind`），把 DB 行映射过去即可，
**不要改调用方的签名**。

`handle_quant_confirmation`（`chat.rs:1226`）的超时判断改为读交互单的 `expires_at`，
删掉 `quant_confirm_expired` 里按 source 硬编码 5 分钟的逻辑
（TTL 现在是数据，不是代码）。`parse_quant_confirmation` 与
`normalize_quant_confirm` **保持不动**，它们的白名单与批量解析是对的。

应答成功后调 `answer_interaction`；返回 `None` 时给出「已提交过 / 已超时」的可读文案。

### 4. card.rs：确认卡片补基本信息区

`ConfirmCardOptions`（`card.rs:103`）增加三个字段：

```rust
pub struct ConfirmCardOptions {
    pub title: String,
    pub question: String,
    pub choices: Vec<String>,
    pub interaction_id: String,   // 新增：卡片展示的任务编号
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    /// 新增：admin 详情深链；配置缺失时为 None，此时不渲染该行
    pub admin_url: Option<String>,
    pub hint: Option<String>,
}
```

`build_confirm_card` 的 body 在 question 之后、按钮之前插入基本信息区。用飞书
`column_set` 做两列（与 `build_deployment_card` 的既有做法一致，参考
`card.rs:211` 起的实现，**照那个写法来，不要自创结构**）：

```
**基本信息**
任务编号 `<interaction_id>`     状态 待确认
会话     `<session_id 前 8 位>`  来源 <渠道中文名>
```

按钮的 callback value **必须加 `interaction_id`**：

```rust
"value": {
    "action": "answer",
    "interaction_id": opts.interaction_id,   // 新增：回调按 id 定位，不再靠 session
    "session_id": opts.session_id,
    "chat_id": opts.chat_id,
    "topic_id": opts.topic_id,
    "choice": choice,
}
```

`question` 那个截断放进 value 的字段（`card.rs:122`）可以**删掉** ——
终态卡片现在能从交互单读回 question，不需要塞进 callback payload。

`build_confirm_done_card` 增加 `interaction_id` 参数，终态卡片同样展示任务编号。

### 5. pusher.rs / callback.rs

`pusher.rs:301` 附近构造 `ConfirmCardOptions` 时：`interaction_id` 从事件对应的
交互单取。**注意时序**：`chat.rs` 的事件转发分支负责写表，`pusher` 消费同一事件流
渲染卡片。为避免 pusher 读到还没写完的行，让 `AgentEvent::AskUser` **携带
`interaction_id`** —— 在 `crates/code-agent` 的事件定义里加字段成本较高，
改用更简单的办法：`chat.rs` 写表后把 `interaction_id` 放进
`state.tasks` 的会话快照（或一个 `Arc<DashMap>` 之类的轻量映射），
pusher 按 `session_id` + `tool_use_id` 取。

**实现方自行判断哪种方式更干净，但必须满足：pusher 拿到的 `interaction_id` 一定是
已落库的那一行。** 如果发现改 `AgentEvent::AskUser` 加字段反而更直接（只是
`crates/code-agent/src/session.rs` 里几处构造点 + `chat.rs` 消费点），也可以走那条路
—— 那样时序天然正确，优于旁路映射。**优先选时序正确的方案，不要为了少改文件而引入
读到空行的竞态。**

`callback.rs` 的 `answer` 分支（`callback.rs:88` 起）：优先读 `interaction_id`，
用 `answer_interaction` 原子应答；`None` 时回 toast「这个操作已经提交过了」或
「待确认已超时」。现有的 `card_action_claim_id` 幂等去重
（`callback.rs:131`、`callback.rs:305`）**保留** —— 它防的是飞书重复投递，
与交互单的状态机是两层不同的防护。

### 6. config：admin 深链

`ServerConfig` 加：

```rust
    /// admin 后台外部可访问地址，用于在渠道卡片里生成交互单详情深链。
    /// 留空则卡片不渲染深链行（本地 dev 常见）。
    #[serde(default)]
    pub admin_base_url: Option<String>,
```

深链格式 `{admin_base_url}/#/interactions/{id}`，与 admin 的 hash 路由一致
（`admin/src/main.ts` 用的是 hash 模式，注意别写成 history 模式的路径）。
任务 B 才会真正实现那个页面；本任务只管生成链接。

### 7. 单元测试

- `hank-db`：本仓库 db 层没有集成测试基建，**不要**为此新建测试容器。只需保证
  `AgentInteraction` 的 `FromRow` 字段与 SELECT 列一一对应（靠编译期保证）。
- `card.rs`：新增测试，断言 `build_confirm_card` 的输出里含 `interaction_id`、
  按钮 callback value 含 `interaction_id`、`admin_url` 为 `None` 时不出现深链行。
- `chat.rs`：`parse_quant_confirmation` / `normalize_quant_confirm` 的既有测试
  **必须全部保留通过**。删掉的 `quant_confirm_expired` 相关测试
  （`test_quant_confirm_expired_only_weixin`）改为断言「`expires_at` 为 None 时
  永不过期、为过去时刻时判过期」的新纯函数。

## 四、验收标准

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test -p hank-server
cargo test -p code-tools
cargo test -p code-agent
```

注意：server 的包名是 **`hank-server`**，不是 `server`。

期望结果：

- 全部通过。
- clippy 警告数**不得超过 60**（当前 master 与 `2019dba` 都是 60）。那 5 条
  `never used` 是 `deployment.rs` 里已死的部署链路，属既有问题，不要顺手改。
  特别注意新增 CRUD 不要引入新的 `too_many_arguments`。
- 既有测试一条都不改、不删（除第 7 步明确要求替换的那一条）。
- `grep -rn "QuantPendingConfirmStore" --include="*.rs" .` 应无结果（已彻底删除）。

**人工验收**（我来跑）：本地起 server + quant，飞书发「回测策略 42」→ 确认卡片上
能看到任务编号与会话 → 点「确认」→ 回测真正执行 → `SELECT * FROM
agent_interactions` 能看到该行状态流转为 `answered`。重启 server 后未应答的交互单
仍在表里。

## 五、约定

- 遵循 `CLAUDE.md`：中文注释、中文 commit message、`anyhow` 错误处理。
- 注释写**为什么**（本仓库风格是记录踩过的坑），不写做了什么。建表与
  `resume_ref` 处必须写清「为什么不能寄生在 session 上」。
- commit message 建议：`feat(feishu): 交互单落表，卡片展示任务编号`
- 不新增依赖，不改 `Cargo.toml`，不改 `config.toml`。
- SQL 全部走 `db_retry!` 宏；MySQL 保留字加反引号（本表无保留字，但 `options`
  在部分版本下敏感，建表与查询都用反引号包裹更稳）。
