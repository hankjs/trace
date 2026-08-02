# 任务 03a（修正）：交接段被「说明标题」抢先匹配，导致解析全空

> 这是任务 03 的**修正任务**，范围很小：改 1 个函数 + 1 段 prompt 文案 + 加 2 项单测。
> 任务 01/02/03 的其余成果全部保留，不要回退。

## 背景：一个已复现的解析失败

任务 03 的 `handoff_requirement`（`server/src/team_task/roles.rs:80`）产出的 prompt
里有**两个** `## 交接` 标题：第一个是「说明」段的标题，第二个是要模型照抄的模板。

而 `extract_handoff_section`（`server/src/team_task/mod.rs:520`）的实现是
**取第一个**匹配的标题，并在**下一个 markdown 标题处截断**。

于是当模型把 prompt 里的说明段一起抄回来时（这是很常见的行为），回复形如：

```markdown
好的，我按格式输出。

## 交接
在回复的**最后**输出下面这段，键名与格式不要改动：

## 交接
verdict: pass
changed_files: 4
summary: 真实结论
blocking: none
```

`extract_handoff_section` 匹配到第一个 `## 交接`，向下取到第二个 `## 交接` 就截断，
拿到的正文只有「在回复的最后输出下面这段…」这一行——里面没有任何 `key: value`。

**我已实测确认**（临时探针，已删除）：

```
PROBE verdict=None changed=None summary=None
```

三个字段全空。对评审/测试角色，`verdict = None` 会被编排器归一成
`Verdict::Unknown`，按状态机规则（任务 02 步骤 5.3 第 7 条）**任务直接 failed**。

这个失败模式很难查：模型明明输出了正确的交接段，日志里 verdict 却是 Unknown，
任务莫名 failed。所以要在接编排器（第 4 步）之前修掉。

任务 03 现有的 3 项 roundtrip 单测都只喂了**单个** `## 交接` 的回复，
所以没有暴露这个问题。

## 目标

1. `extract_handoff_section` 改成**取最后一个**匹配的标题。
2. prompt 的说明段不再用 `## 交接` 当标题，避免自己制造双标题。
3. 补单测锁定这两点。

做完之后：上面那段带回声的回复能正确解析出
`verdict=Pass, changed_files=4, summary="真实结论"`。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `server/src/team_task/mod.rs` | `extract_handoff_section` 改取最后一个标题；补 1 项单测 |
| `server/src/team_task/roles.rs` | `handoff_requirement` 的说明段标题改掉；补 1 项单测 |

**没有其他文件需要改。**

## 实现步骤

### 步骤 1：解析器取最后一个交接标题

- [ ] **1.1** 改 `server/src/team_task/mod.rs` 的 `extract_handoff_section`：
  把「找到第一个就 `break`」改成**继续扫完、保留最后一个匹配**。
  截断逻辑（取到下一个 markdown 标题或文末）保持不变。

  即把现在的：

  ```rust
  if (2..=3).contains(&hash_count) {
      start = Some(i + 1);
      break;          // ← 取第一个
  }
  ```

  改成不 `break`，让循环把 `start` 一路覆盖到最后一个匹配。

- [ ] **1.2** 在该函数的文档注释里写清**为什么取最后一个**：

  > 取**最后一个**「交接」标题，不是第一个。prompt 要求模型「在回复的最后输出」，
  > 而模型常把 prompt 里的格式说明一起抄回来，形成两个 `## 交接`——
  > 取第一个会拿到说明文字（没有 key: value），三个字段全空，
  > 评审 verdict 变 Unknown 导致任务莫名 failed。正文里顺口提到「交接」
  > 的散段同理会被前面的匹配挡掉。

### 步骤 2：prompt 不再自己制造双标题

- [ ] **2.1** 改 `server/src/team_task/roles.rs` 的 `handoff_requirement`：
  说明段的标题从 `## 交接` 改成 `## 输出格式要求`，
  **只保留一个** `## 交接`（即要模型照抄的那个模板）。

  改完后 prompt 里 `## 交接` 只出现一次。这是第二道防线——
  即使解析器取最后一个已经能兜住回声，也不该主动在 prompt 里埋两个同名标题。

- [ ] **2.2** 模板里的占位值改得更像占位、更不像答案，降低模型整段照抄的概率：

  ```
  ## 交接
  verdict: <pass 或 reject，二选一，不要写其他词>
  changed_files: <纯数字>
  summary: <一句话说明判定理由>
  blocking: <阻塞项；没有就写 none>
  ```

  注释说明：尖括号占位符照抄回来时 `changed_files` 解析成 `None`、
  `Verdict::parse` 判 Unknown，**这是期望行为**——模型没真的填值就该失败，
  而不是被当成有效结论。

## 单测

- [ ] **3.1** `server/src/team_task/mod.rs`：新增
  `parse_handoff_takes_last_section_on_echoed_instruction`，
  直接用背景里那段带回声的回复（两个 `## 交接`），断言
  `verdict == Some(Verdict::Pass)`、`changed_files == Some(4)`、
  `summary == Some("真实结论")`。这是本修正的回归测试。

- [ ] **3.2** `server/src/team_task/roles.rs`：新增
  `prompt_has_single_handoff_header`，对三个角色的 prompt 各断言
  `p.matches("## 交接").count() == 1`。

- [ ] **3.3** 任务 02 与任务 03 的现有单测**一项都不能改、不能删**，且必须全绿。
  特别是 `parse_handoff_first_key_wins`（同一**键**出现多次取第一次）——
  本次改的是同一**标题**出现多次取最后一个，两者不冲突，该测试应保持通过。

## 明确边界

**不许碰**：
- `crates/`（hank-db 657 行是第 1 步成果）
- `server/src/config.rs`、`server/src/main.rs`（第 2 步成果）
- `admin/`、`client/`、`quant/`、`cli/`、`config.toml`、`Cargo.toml`、`CLAUDE.md`、`docs/`
- `server/src/` 下除 `team_task/mod.rs` 与 `team_task/roles.rs` 之外的任何文件

**不许做**：
- 不要改 `decide_next`、`parse_handoff_fields`、`split_kv`、`truncate_chars`、
  状态常量、`Verdict`、`RoleDef`、`ROLE_DEFS`
- 不要改三个角色 prompt 的其余部分（只改 `handoff_requirement`）
- 不要写编排器 / 卡片 / REST / 看板（第 4–8 步）
- 不要删除或修改任务 02、03 的既有单测

## 验收标准

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets
cargo test -p hank-server team_task
cargo test --workspace
git status --short
```

期望结果：
- 编译成功；`server/src/deployment.rs` 那 5 个既有 `never used` warning 属正常
- clippy 基线 46 个 warning，无新增
- `cargo test -p hank-server team_task` **≥ 50 项**全绿
  （任务 02 的 34 + 任务 03 的 14 + 本次 2）
- `cargo test --workspace` 全绿，server 那组 ≥ 177
- 改动只涉及 `server/src/team_task/mod.rs` 与 `server/src/team_task/roles.rs`

## 约定

- 中文注释，两处「为什么」必须写清：解析器为何取最后一个、
  prompt 为何不用 `## 交接` 当说明标题
- 中文 commit message，形如
  `fix(team-task): 交接段取最后一个标题，避免说明回声抢先匹配`
