# 任务 B：admin 交互单管理页（三份任务的第二份）

## 拆分说明

三份文档，顺序 A → B → C，一次一份：

| 文档 | 内容 | 状态 |
|---|---|---|
| A | `agent_interactions` 表 + CRUD；交互迁到表；卡片补 task_id | **已完成**（含 `interaction-answer-rollback-fix.md` 修正） |
| **B（本文）** | admin `/interactions` 页：列表、筛选、详情、手动应答、取消 | 本次 |
| C | 两阶段任务闸门：第一轮只分析 → 大卡片 → 点「开始修」resume 跑第二轮 | 待 B 完成 |

本文只做 B。**不要**实现两阶段闸门，**不要**改渠道侧的卡片渲染逻辑。

## 一、背景与目标

任务 A 已让交互单落表，但目前**只能用 SQL 查**。缺两件事：

1. **可观测**：交互单的状态流转（`pending → answered → done`）没有界面。
   任务 C 的两阶段闸门会大量产生交互单，没有列表页会很难调试。
2. **可救援**：飞书卡片丢失、用户误关、或出现 A 修正里那类「派发未完成」的
   边缘情况时，交互单会停在 `pending`。目前唯一出路是 `/new` 重开话题，
   丢掉整个会话上下文。

### 目标

admin 新增 `/interactions` 页：列表 + 筛选 + 详情 + **手动应答** + 取消。

### 做完之后的可观察效果

1. admin 侧栏出现「交互单」，列表显示 task_id、类型、状态、标题、渠道、时间。
2. 可按 `status` / `kind` / `channel` 筛选，分页。
3. 点开某行看到完整详情：`goal`、`analysis` 全文、`options`、`resume_ref`、
   `answer`、`result` / `error`。
4. 对 `pending` 交互单可**代替用户应答**（选一个 option），任务真的会继续执行 ——
   不只是改数据库状态。
5. 对 `pending` 交互单可取消，状态变 `cancelled`，卡片不再可点。
6. 渠道卡片里的 admin 深链（A 已生成，格式 `{admin_base_url}/#/interactions/{id}`）
   点开能直达该交互单详情。

## 二、涉及文件清单

| 文件 | 改什么 |
|---|---|
| `crates/hank-db/src/lib.rs` | 新增 `list_interactions`（带筛选+分页）、`cancel_interaction` |
| `server/src/interactions.rs` | **新建**：admin REST handler |
| `server/src/main.rs` | 注册 4 条 admin 路由；`mod interactions;` |
| `admin/src/composables/api.ts` | `AgentInteraction` 类型 + 4 个 API 方法 |
| `admin/src/views/Interactions.vue` | **新建**：列表页 |
| `admin/src/main.ts` | 两条路由（列表 + 详情锚点） |
| `admin/src/App.vue` | 侧栏菜单项 |

**不许碰**：

- `server/src/feishu/pusher.rs`、`card.rs`、`router.rs`（渠道渲染逻辑不动）
- `server/src/chat.rs`（交互单写入与 resume 消费逻辑不动）
- `server/src/cli_agent.rs`、`deployment.rs`（C 的范围 / 无关）
- `crates/code-agent/`、`crates/code-tools/`
- `quant/`、`client/`

**例外**：`server/src/feishu/callback.rs` 允许**只提取**手动应答需要复用的派发函数
（见实现步骤 3），不得改动其回调主流程的顺序与语义 —— 那套顺序
（抢名额 → claim → 应答 → 改卡 → 派发）是 A 修正的核心，动了会重新引入缺陷。

**保留工作区原有改动**：A 的改动可能尚未提交，**不要回退**，在其之上继续。
`docs/tasks/*.md` 都不要删。

## 三、实现步骤

### 1. hank-db：列表查询与取消

```rust
/// 交互单列表（admin）。筛选项均为可选，传 None 表示不限。
/// 返回 (当页数据, 总条数)，与 list_channel_messages 的既有约定一致。
pub async fn list_interactions(
    &self,
    status: Option<&str>,
    kind: Option<&str>,
    channel: Option<&str>,
    page: u32,
    per_page: u32,
) -> Result<(Vec<AgentInteraction>, i64)>

/// 取消待确认交互单。只动 pending，不覆盖终态。
pub async fn cancel_interaction(&self, id: &str, operator: &str) -> Result<bool>
```

`list_interactions` 注意：

- 动态 WHERE 用 `QueryBuilder`，或者按本仓库既有做法拼固定分支。**不要**用
  字符串拼接绑定值（SQL 注入）。`status` / `kind` / `channel` 都是枚举型短字符串，
  但仍必须走 `.bind()`。
- `ORDER BY created_at DESC`，分页 `LIMIT ? OFFSET ?`。
- SELECT 列表必须与 `AgentInteraction` 的 `FromRow` 字段完全一致，
  含 `` `options` AS `options` `` 的反引号处理（A 里已有先例，照抄）。
- 列表页不需要 `analysis` 全文（可能几十 KB × 50 行）。但为避免维护两套
  SELECT 列表出错，**本次仍返回完整行**；由前端列表只渲染摘要。
  若将来出现性能问题再拆 `list` / `detail` 两个 SQL。

`cancel_interaction` 的 SQL：

```sql
UPDATE agent_interactions
   SET status = 'cancelled', answered_by = ?, answered_at = NOW(), updated_at = NOW()
 WHERE id = ? AND status = 'pending'
```

`db_retry!` 包裹，返回 `rows_affected() == 1`。

### 2. server：admin REST

新建 `server/src/interactions.rs`，照 `server/src/channel_records.rs` 的
`list_messages`（`channel_records.rs:73`）风格写：

```rust
GET    /api/admin/interactions          — 列表，query: status/kind/channel/page/per_page
GET    /api/admin/interactions/{id}     — 详情
POST   /api/admin/interactions/{id}/answer   — 手动应答，body: {"answer": "确认"}
POST   /api/admin/interactions/{id}/cancel   — 取消
```

约定：

- 用 `crate::response::{self as R}`：`R::ok(...)` / `R::bad_request(...)` /
  `R::not_found(...)` / `R::internal_error(e)`。
- 列表返回 `PaginatedResponse { data, total, page, per_page }`，
  `per_page` 用 `.clamp(1, 200)`，`page` 用 `.max(1)`（与 `channel_records.rs:87` 一致）。
- 筛选参数做**白名单校验**，非法值返回 `bad_request` 而不是静默忽略：
  - `status` ∈ `pending|answered|executing|done|failed|expired|cancelled`
  - `kind` ∈ `quant_confirm|ask_user|task_gate`
  - `channel` ∈ `feishu|weixin|trace_chat`
- 空字符串视为「不限」（前端下拉「全部」会传空串）。

路由注册在 `main.rs`，**放进已有的 admin 路由段**（`main.rs:566` 附近的
scheduler admin routes 旁边），这样自动继承 `admin_required` 中间件
（`main.rs:107`，要求 `claims.can_admin`）。**务必确认注册在 admin 段内** ——
交互单能触发高成本操作，放到普通鉴权段等于给所有登录用户开了后门。

### 3. 手动应答必须真的派发（本任务的关键点）

**只把状态改成 `answered` 是错的。** 那正是 A 修正里修掉的缺陷形态：
交互单标记已应答，但没有任何东西去消费它，任务永远不执行，且因为
`answer_interaction` 的 `WHERE status = 'pending'` 而再也无法重试。

所以 `POST /answer` 必须完成完整链路：

1. 读交互单，校验 `status == "pending"`（否则 `bad_request` 说明当前状态）。
2. 校验 `answer` 在该交互单的 `options` 数组内 —— **不接受任意字符串**。
   `options` 是 JSON 文本，解析后比对。不在其中则 `bad_request`。
3. 抢派发名额 → `answer_interaction` → 派发 → 失败则回滚。
   **这四步的顺序和错误处理必须与 `callback.rs` 的按钮回调完全一致。**

为避免把那套来之不易的顺序抄错，**从 `callback.rs` 提取一个可复用函数**，
让飞书按钮回调与 admin 手动应答共用同一条实现。建议签名：

```rust
/// 应答一张交互单并派发 resume：抢名额 → 原子应答 → 派发 → 失败回滚。
/// 飞书按钮回调与 admin 手动应答共用，避免两处各写一遍顺序而漂移。
///
/// 返回 Err 表示未能应答（名额被占、已被抢答、已过期），调用方转成用户可读提示。
pub async fn answer_and_resume(
    state: &Arc<AppState>,
    interaction_id: &str,
    answer: &str,
    operator_user_id: &str,
    channel_ctx: Option<ChannelCardContext>,   // 飞书传卡片上下文；admin 传 None
) -> Result<()>
```

`channel_ctx` 为 `None` 时跳过所有卡片相关操作（改终态卡、失败恢复卡），
其余逻辑一致。放在哪个模块由实现方决定 —— 若 `callback.rs` 里最自然就留在那儿并
`pub`，若更适合新建 `server/src/interaction_flow.rs` 也可以。**唯一硬要求：
飞书回调与 admin 应答走同一个函数，不得复制两份。**

提取后 `callback.rs` 的行为必须逐字不变。判定标准：`cargo test -p hank-server`
全过，且回调主流程的顺序注释（① 抢名额 ② claim ③ 应答 ④ 改卡 ⑤ 派发）仍然成立。

**如果发现提取会让 `callback.rs` 大幅重构**（比如卡片上下文与派发逻辑耦合太深，
拆不干净），那就**停下来，在文档里记下原因，改为 admin 应答只支持
`kind = "ask_user"` 的简单场景**，把 `quant_confirm` 的手动应答留到后续任务。
宁可少做一个场景，也不要为了复用把回调流程改坏。

`POST /cancel`：调 `cancel_interaction`，无需派发。若交互单带
`card_message_id` 且渠道是飞书，**本任务不改卡片** —— 卡片仍显示可点，
但点了会因状态非 `pending` 被拒并 toast「这个操作已经提交过了」，
行为正确、不会误执行。（把取消同步回卡片属于优化，不在本次范围。）

### 4. admin 前端

`admin/src/composables/api.ts`：加 `AgentInteraction` interface（字段与 Rust struct
对齐，`snake_case`）与 4 个方法，照 `listJobs` / `jobRuns`（`api.ts:635`）的写法。

`admin/src/views/Interactions.vue`：抄 `Jobs.vue`（241 行）的骨架 ——
`<script setup lang="ts">`、顶部注释说明这个页面存在的理由、`loading` /
`actionError` / `notice` 三个状态、表格 + 展开行详情。

必须有：

- **顶部说明块**（`Jobs.vue:135` 那种边框 div）：说清交互单是什么、
  手动应答的用途与风险。文案要点：「手动应答会代替用户做出选择并真的推进任务执行；
  高成本量化操作（回测 / trial / 因子评估）会真实消耗配额，确认前请核对标题与目标」。
- 筛选：三个 `<select>`（状态 / 类型 / 渠道），含「全部」选项。
- 表格列：任务编号（前 8 位 + 完整值 title 属性）、类型、状态、标题、渠道、
  创建时间、应答时间、操作。
- 状态用颜色区分，但**不能只靠颜色**（`quant/PRODUCT.md` 的无障碍口径同样适用）：
  文字标签必须写明中文状态名。`pending` 用 `text-yellow-500`，
  `failed` / `expired` 用 `text-red-400`，`done` 用 `text-text-primary`，
  其余 `text-text-secondary`。
- 展开行详情：`goal`、`analysis`（`whitespace-pre-wrap`，`analysis` 可能是
  markdown 源文，本任务**不渲染 markdown**，纯文本展示即可）、
  `options`、`resume_ref`、`answer`、`result`、`error`、关联 `session_id`
  （给一个跳 `/sessions/{id}` 的链接）。
- 操作：`status === 'pending'` 时显示每个 option 的应答按钮 + 「取消」；
  其余状态不显示操作按钮。应答与取消都要 `confirm()` 二次确认
  （`Jobs.vue:72` 有先例）。
- 分页控件。若 admin 已有可复用的分页组件（先在 `admin/src/components/` 找），
  用它；没有就照 `ChatRecords.vue` 的做法写。

**轮询**：`Jobs.vue` 有 `hasRunning` 驱动的条件轮询。交互单列表**不要**默认轮询
（`pending` 是常态，会变成永久轮询）。给一个手动「刷新」按钮即可。

`admin/src/main.ts` 加路由：

```ts
{ path: '/interactions', component: () => import('./views/Interactions.vue') },
{ path: '/interactions/:id', component: () => import('./views/Interactions.vue') },
```

两条都指向同一组件；带 `:id` 时进入页面后自动展开该行（渠道卡片深链要用）。
若该 id 不在当前筛选/分页结果里，**直接按 id 拉详情并展开**，不要静默什么都不发生。

`admin/src/App.vue` 侧栏（`App.vue:23` 附近）加：

```ts
{ to: '/interactions', label: '交互单', icon: '✋' },
```

放在「定时任务」旁边。

### 5. 测试

- Rust：`interactions.rs` 里的筛选白名单校验抽成纯函数并加单测
  （合法值通过、非法值拒绝、空串视为不限）。
- 前端：`admin/` 目前**没有测试基建**（没有 vitest 配置），
  **不要为此引入**。靠 `npm run build` 的 TypeScript 检查保证。
- 既有测试一条都不改、不删。

## 四、验收标准

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test -p hank-server
cargo test -p code-tools
cargo test -p code-agent
cd admin && npm run build
```

期望结果：

- 全部通过。`admin` 的 build 是严格 TypeScript 检查，类型必须与后端返回对齐。
- clippy 警告数**不得超过 58**（A 完成后的当前值）。不要引入新的
  `too_many_arguments` —— 列表查询 5 个参数没问题，但若继续加筛选项就改入参 struct。
- 既有测试全部通过。

**人工验收**（我来跑）：

1. admin 打开 `/interactions`，看到 A 阶段产生的历史交互单。
2. 筛选 `status=pending`、`kind=quant_confirm` 生效。
3. 飞书触发一次高成本操作但**不点卡片按钮**，在 admin 里手动点「确认」→
   回测真的执行，飞书话题收到结果，交互单状态流转到 `done`。
4. 再触发一次，在 admin 里点「取消」→ 状态 `cancelled`，
   回飞书点原卡片按钮 → toast 提示已提交过，不会执行。
5. 从飞书卡片的 admin 深链点进来 → 直达对应交互单并展开。

## 五、约定

- 遵循 `CLAUDE.md`：前端 `<script setup lang="ts">` + Composition API +
  Tailwind utility classes；API 调用集中在 `admin/src/composables/api.ts`；
  中文注释与 commit message；后端错误用 `anyhow`。
- 注释写**为什么**。`answer_and_resume` 的提取处必须写清「为什么飞书回调与 admin
  必须共用：两处各写一遍顺序会漂移，漏掉回滚就会让确认被静默吞掉」。
- 颜色不是唯一信息载体（无障碍）。
- commit message 建议：`feat(admin): 交互单管理页，支持手动应答与取消`
- 不新增依赖，不改 `Cargo.toml` / `package.json` / `config.toml`。
- 新建的 `Interactions.vue` 控制在 300 行以内；超了就把详情抽成
  `admin/src/components/` 下的子组件。
