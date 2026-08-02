# 任务 03：团队任务流水线 — 角色注册表 prompt 与交接输入

> 本任务是 `docs/feature/team-task-pipeline.md` 实施顺序的**第 3 步**，共 9 步。
> 第 1 步（数据层）与第 2 步（状态机纯函数 + 配置段）已完成并合入工作区。
>
> 本任务只写**prompt 构造纯函数**，**不接任何现有链路**：不派发 run、不发卡片、
> 不读写数据库。做完后编译通过、新增单测全绿即可验收。
> 编排器（`orchestrator.rs`）是第 4 步，本任务**不要**提前实现。

## 背景与目标

### 背景

团队任务流水线把代码任务拆成**开发 → 评审 → 测试**三个角色，每个角色一次独立的
hank-cli run。关键约束（设计文档 §3）：

> **角色间用产物交接，不共享上下文**：评审轮的 prompt 注入开发轮的产物，
> 而不是 resume 开发的 thread。这是刻意的——评审要独立视角，
> 共享 thread 会让评审顺着开发的思路走。

因此**每个角色的 prompt 必须自包含**：拿到 prompt 的模型是一个全新 thread，
它不知道目标是什么、上一个角色做了什么，全部信息都得由 prompt 带进去。

第 2 步已建好 `server/src/team_task/mod.rs`，其中 `RoleDef` 目前有
`id` / `label` / `running_status` / `needs_verdict` 四个字段，**还没有 `prompt` 字段**
（设计文档 §6.2 里有）。本任务补上它。

### 本任务目标

1. 给 `RoleDef` 加 `prompt` 字段，把三个角色的 prompt 构造函数挂进注册表。
2. 新建 `server/src/team_task/roles.rs`，实现三个角色的 prompt。
3. 定义 prompt 输入结构 `RolePromptInput` / `UpstreamHandoff`。
4. 单测覆盖：自包含性、交接段格式与 `parse_handoff` 的往返一致性、
   打回轮次带上评审意见、工作区边界不泄露 server 路径。

### 做完之后的可观察效果

1. `cargo build --workspace` 通过，无新增 warning。
2. `cargo test -p hank-server team_task` 全绿（第 2 步的 34 项 + 本任务新增）。
3. `role_def("reviewer").unwrap().prompt(&input)` 能产出一段自包含的中文 prompt。
4. 现有功能行为**完全不变**（prompt 函数尚无调用方）。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `server/src/team_task/roles.rs` | **新建**。`RolePromptInput` / `UpstreamHandoff`、三个 prompt 函数、公共约束段、单测 |
| `server/src/team_task/mod.rs` | 加 `pub mod roles;`；给 `RoleDef` 加 `prompt` 字段；`ROLE_DEFS` 三行各挂上对应函数 |

**没有其他文件需要改。**

## 实现步骤

### 步骤 1：输入结构

- [ ] **1.1** 在 `server/src/team_task/roles.rs` 定义（`Verdict` 从 `super` 引入）：

```rust
//! 三个角色的 prompt 构造。
//!
//! 每个角色跑在**独立的 CLI thread** 上（设计文档 §3：角色间用产物交接、
//! 不共享上下文，评审要独立视角）。因此每段 prompt 必须自包含——
//! 拿到它的模型不知道任务目标，也看不到上一个角色的对话。

use super::Verdict;

/// prompt 构造入参。字段多，用结构体避免位置参数。
#[derive(Debug, Clone)]
pub struct RolePromptInput<'a> {
    /// 用户原始诉求（team_tasks.goal）
    pub goal: &'a str,
    /// 闸门第一轮产出的四段分析（team_tasks.analysis）。可能为空。
    pub analysis: Option<&'a str>,
    /// trace_code / quant_code / general_task，用于追加项目特定约束
    pub agent_kind: &'a str,
    /// 本角色第几轮（开发被打回后是 2、3……）
    pub round: i32,
    /// 上游角色的交接产物。开发首轮为 None；
    /// 评审看开发的、测试看评审的、打回后的开发看评审的。
    pub upstream: Option<UpstreamHandoff<'a>>,
}

/// 上游角色交接给本角色的产物（来自 team_task_runs 的 handoff / summary 列）。
#[derive(Debug, Clone)]
pub struct UpstreamHandoff<'a> {
    /// 上游角色 id，如 "developer"
    pub role: &'a str,
    pub summary: Option<&'a str>,
    pub verdict: Option<Verdict>,
    pub blocking: Option<&'a str>,
    pub changed_files: Option<i32>,
}
```

### 步骤 2：公共段落

三个 prompt 共用的部分抽成私有函数，避免三处各写一遍后漂移。

- [ ] **2.1 工作区约束**。措辞对齐 `server/src/cli_agent.rs` 的 `local_agent_prompt`
  （约 2262 行），**不要**出现 `/opt/hank` 之类 server 绝对路径——
  client-only 会话跑在用户本机，泄露 server 路径会让模型去找不存在的目录：

```rust
/// 工作区与安全边界。所有角色共用。
fn workspace_constraints(agent_kind: &str) -> String {
    let mut s = String::from(
        "\n\n运行约束：\n\
         - 只操作 hank-cli 提供的当前工作目录及其子目录。\n\
         - 遵循目录中的 AGENTS.md / CLAUDE.md 等项目规则。\n\
         - 不要读取或修改凭据、密钥或本机 Agent 认证配置。\n",
    );
    if agent_kind == "quant_code" {
        s.push_str("- 修改 quant 前必须读取 quant/AGENTS.md，并遵守禁止交易能力的产品边界。\n");
    }
    s
}
```

- [ ] **2.2 任务背景**。把 goal 与 analysis 注入，这是「自包含」的核心：

```rust
/// 任务背景：目标 + 第一轮分析。每个角色的 prompt 都以它开头。
fn task_context(input: &RolePromptInput<'_>) -> String;
```

产出形如：

```markdown
## 任务目标
{goal}

## 先前的只读分析
{analysis}
```

`analysis` 为 `None` 或空白时**整段省略**（不要留一个空的「先前的只读分析」标题，
模型会以为分析丢了）。

- [ ] **2.3 交接段要求**。这段必须与 `mod.rs` 的 `parse_handoff` 严格对齐，
  否则解析不到会让评审/测试的 verdict 变成 Unknown、任务直接 failed。

  **`parse_handoff` 实际接受的格式**（已实现，不要改它）：
  - 标题行：`##` 或 `###` 开头且**包含「交接」二字**
  - 键名：`verdict` / `changed_files` / `summary` / `blocking`，大小写不敏感
  - 分隔符：半角 `:` 或全角 `：`
  - 同一键出现多次取第一次

```rust
/// 交接段要求。needs_verdict 为 false 的角色（开发）不要求 verdict 行。
fn handoff_requirement(needs_verdict: bool) -> String;
```

需要 verdict 时产出：

```markdown
## 交接
在回复的**最后**输出下面这段，键名与格式不要改动：

## 交接
verdict: pass 或 reject
changed_files: 本轮改动的文件数（纯数字）
summary: 一句话说明判定理由
blocking: 阻塞项；没有就写 none
```

不需要 verdict 时省略 `verdict` 那一行，其余相同。

> 实现提示：`verdict` 只允许 `pass` / `reject` 两个值，要在 prompt 里写死。
> 模型写别的（如「基本通过」）会被 `Verdict::parse` 判成 Unknown，
> 按状态机规则任务直接 failed——所以这里的措辞要强。

### 步骤 3：三个角色的 prompt

- [ ] **3.1 开发角色**

```rust
/// 开发角色：按分析执行改动。
///
/// round > 1 表示被评审打回，此时必须把评审意见注入——否则模型在新 thread 上
/// 看不到自己上一轮做了什么，也不知道为什么被打回，会从头再来一遍。
pub fn developer_prompt(input: &RolePromptInput<'_>) -> String;
```

结构：
1. `【本轮角色：开发】` 开头
2. `task_context`
3. `round == 1`：`本轮请按上面的分析执行代码修改。`
4. `round > 1`：`## 上一轮被评审打回` + 评审的 `summary` 与 `blocking`，
   并要求「针对打回意见修改，不要重做无关部分」
5. 要求完成后自行验证（编译 / 跑相关测试）
6. `workspace_constraints`
7. `handoff_requirement(false)`

- [ ] **3.2 评审角色**

```rust
/// 评审角色：独立审查开发产出。
///
/// 刻意不 resume 开发的 thread，所以这里要告诉它自己去看 diff——
/// 它拿到的是一个干净 thread，只有本 prompt 里的信息。
pub fn reviewer_prompt(input: &RolePromptInput<'_>) -> String;
```

结构：
1. `【本轮角色：评审】` 开头
2. `task_context`
3. `## 开发的自述` + 上游 `summary` / `changed_files`
   （`upstream` 为 None 时写「上一轮没有留下交接说明，请完全依据 diff 判断」）
4. 明确要求：**先用 `git diff`（必要时 `git status`）看清本轮实际改动**，
   再对照任务目标判断。措辞要点出「开发的自述可能与实际改动不符，以 diff 为准」
5. 判定口径（写清什么该 reject）：偏离目标、引入明显缺陷、
   改了任务范围外的文件、破坏既有行为
6. `【本轮只读】` 约束：评审**不要改代码**。理由与闸门第一轮相同——
   CLI 以 bypass-approvals 启动，沙箱不会拦写操作，只能靠指令约束
7. `workspace_constraints`
8. `handoff_requirement(true)`

- [ ] **3.3 测试角色**

```rust
/// 测试角色：跑测试并验证行为。
pub fn tester_prompt(input: &RolePromptInput<'_>) -> String;
```

结构：
1. `【本轮角色：测试】` 开头
2. `task_context`
3. `## 评审结论` + 上游 `summary`（评审已 pass 才会走到这里）
4. 要求：按项目约定跑测试（先读 CLAUDE.md / AGENTS.md 找命令，
   **不要凭猜测编命令**），跑与改动相关的测试矩阵
5. 判定口径：测试失败、或改动没有达到任务目标 → reject；
   **只有测试确实通过才写 pass**
6. 允许写测试文件，但**不要为了让测试通过而改业务代码**——
   那是开发的职责，测试角色改业务代码会让「测试通过」失去意义
7. `workspace_constraints`
8. `handoff_requirement(true)`

### 步骤 4：接进注册表

- [ ] **4.1** 在 `server/src/team_task/mod.rs` 顶部加 `pub mod roles;`。

- [ ] **4.2** 给 `RoleDef` 加字段：

```rust
    /// 该角色的 prompt 构造函数。函数指针而非 Box<dyn Fn>，
    /// 因为 ROLE_DEFS 是 const，且 prompt 构造是纯函数无需捕获环境。
    pub prompt: fn(&roles::RolePromptInput<'_>) -> String,
```

- [ ] **4.3** `ROLE_DEFS` 三行各补 `prompt: roles::developer_prompt` 等。
  **不要改动** `id` / `label` / `running_status` / `needs_verdict` 的现有值，
  第 2 步的 34 项单测依赖它们。

- [ ] **4.4** 加一个便捷函数：

```rust
/// 按角色 id 构造 prompt；未知角色返回 None（调用方转用户可见错误，不 panic）。
pub fn role_prompt(role: &str, input: &roles::RolePromptInput<'_>) -> Option<String> {
    role_def(role).map(|d| (d.prompt)(input))
}
```

### 步骤 5：单测

放在 `server/src/team_task/roles.rs` 的 `#[cfg(test)] mod tests`。

- [ ] **5.1 自包含性**（每个角色各一条）：prompt 里必须出现 goal 原文与
  analysis 原文（analysis 非空时）。这是防「以为能 resume 上一轮」的回归测试。

- [ ] **5.2 交接段与 `parse_handoff` 往返一致**。这是本任务最重要的测试：
  按 prompt 里给的格式**手写一段模型可能的回复**，喂给 `super::parse_handoff`，
  断言四个字段都解析出来。prompt 里的格式说明一旦与解析器漂移，
  线上表现是「评审 verdict 全是 Unknown、任务全部 failed」，而且不好查。
  - 需要 verdict 的角色：解析出 `Some(Verdict::Pass)`
  - 开发角色的 prompt 里**不应**要求 verdict 行（断言不含 `verdict:`）

- [ ] **5.3 打回轮次带评审意见**：`round = 2` + `upstream` 带
  `verdict: Reject` / `summary: "漏了错误处理"` / `blocking: "缺单测"`，
  断言 prompt 里出现「打回」字样、出现 summary 与 blocking 原文。

- [ ] **5.4 `upstream = None` 时的降级**：评审角色在没有上游交接时
  仍能产出可用 prompt（不 panic、不出现 `None` 字面量、
  含「依据 diff」之类兜底措辞）。

- [ ] **5.5 `analysis = None` 时不留空标题**：断言 prompt 不含
  「先前的只读分析」这个标题。

- [ ] **5.6 工作区边界**（仿 `cli_agent.rs` 的
  `local_agent_prompt_does_not_embed_server_workspace_paths`）：
  三个角色的 prompt 都不含 `/opt/hank`、不含 `/workspace`，且都含 `hank-cli`。

- [ ] **5.7 `quant_code` 追加约束**：`agent_kind = "quant_code"` 时
  prompt 含 `quant/AGENTS.md`；`trace_code` 时不含。

- [ ] **5.8 评审只读约束**：`reviewer_prompt` 含「不要改代码」类措辞；
  `developer_prompt` 不含。

- [ ] **5.9 `role_prompt` 查表**：已知角色返回 `Some`，未知角色返回 `None`。

## 明确边界

**不许碰的文件/模块**：
- `crates/` 下所有 crate（第 1 步的 `hank-db` 改动已完成，**不要再改它**）
- `admin/`、`client/`、`quant/`、`cli/`
- `server/src/` 下除 `team_task/roles.rs`（新建）与 `team_task/mod.rs` 之外的
  **任何文件**，特别是 `cli_agent.rs`、`interaction_flow.rs`、`feishu/`、
  `config.rs`、`main.rs`（第 2 步已加好 `mod team_task;`，不用再动）
- `config.toml`、`Cargo.toml`（**不要新增依赖**）
- `CLAUDE.md`、`docs/`

**`mod.rs` 只允许四处改动**：加 `pub mod roles;`、`RoleDef` 加 `prompt` 字段、
`ROLE_DEFS` 三行各挂函数、新增 `role_prompt` 函数。
**不要改** `decide_next`、`parse_handoff`、状态常量、`Verdict`、
或第 2 步的任何单测——它们已通过验收。

**不许做的事**：
- 不要写编排器、飞书卡片、REST、看板（第 4–8 步）
- prompt 函数里不要有 IO、DB、`async`、`AppState`——纯字符串构造
- 不要改 `parse_handoff` 去适配 prompt。方向是反的：
  **prompt 适配已实现的解析器**（解析器已有 8 项单测锁定行为）
- 不要在 prompt 里写 server 绝对路径（`/opt/hank*`、`/workspace`）

**保留工作区原有改动**：
- `crates/hank-db/src/lib.rs`（第 1 步，657 行新增）
- `server/src/config.rs`（第 2 步，112 行新增）、`server/src/main.rs`（第 2 步，1 行）
- `server/src/team_task/mod.rs`（第 2 步，1442 行）——本任务只做上述四处增量
- `docs/feature/team-task-pipeline.md` 与 `docs/tasks/`
- 除本任务涉及的两个文件外不要 `git checkout` 或回退任何内容

## 验收标准

```bash
# 1. 编译
cargo build --workspace

# 2. clippy
cargo clippy -p hank-server --all-targets

# 3. 本模块单测（第 2 步的 34 项必须仍然全绿）
cargo test -p hank-server team_task

# 4. 全量回归
cargo test --workspace

# 5. 改动范围
git diff --stat
git status --short
```

期望结果：
- `cargo build --workspace` 成功。`server/src/deployment.rs` 有 5 个既有的
  `never used` warning，与本任务无关，属正常
- `cargo clippy -p hank-server --all-targets` 无新增 warning
  （改动前基线：46 个 warning，全部指向既有代码）
- `cargo test -p hank-server team_task` 全绿，且**测试数 ≥ 34 + 本任务新增**，
  第 2 步的 34 项一个都不能挂
- `cargo test --workspace` 全绿，总数只增不减
- `git diff --stat` 只列出 `server/src/team_task/mod.rs`；
  `server/src/team_task/roles.rs` 是新文件（在 `git status` 里）。
  `crates/hank-db/src/lib.rs`、`server/src/config.rs`、`server/src/main.rs`
  的行数应与本任务开始前**完全一致**

## 约定

遵循 `CLAUDE.md`：

- **中文注释 + 中文 prompt**。prompt 正文是给模型看的，也用中文
  （与 `cli_agent.rs` 的 `local_agent_analysis_prompt` 一致）
- 以下三处必须注释写清「为什么」：
  1. 每个 prompt 自包含 —— 角色跑在独立 thread，看不到上一轮对话
  2. 评审的只读约束 —— CLI 以 bypass-approvals 启动，沙箱不拦写操作，只能靠指令
  3. `prompt` 用函数指针而非闭包 —— `ROLE_DEFS` 是 `const`，且无需捕获环境
- **中文 commit message**，形如
  `feat(team-task): 三个角色的 prompt 构造与交接输入`
- prompt 构造函数**不返回 `Result`**：输入缺失时降级（省略段落、用兜底措辞），
  没有「构造失败」这个状态
- 单测用同步 `#[test]`
- 测试风格参考 `server/src/cli_agent.rs` 的 `#[cfg(test)] mod tests`
