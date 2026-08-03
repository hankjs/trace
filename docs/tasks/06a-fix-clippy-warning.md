# 任务 06a（修正）：清掉新增的 clippy warning

> 任务 06 的收尾修正，范围极小：改 1 行。
> 06 的其余成果全部保留，不要回退。

## 背景

任务 06 的验收标准写的是「clippy 基线 **46** 个 warning，无新增」。
实测是 **47** 个，多出来的那一个指向本次新增的代码：

```
warning: redundant guard
   --> server/src/team_task/card.rs:348:18
    |
348 |         other if other.is_empty() => "飞书派单".into(),
    |                  ^^^^^^^^^^^^^^^^
    = note: `#[warn(clippy::redundant_guards)]` on by default
```

其余 46 个都是既有代码的（`deployment.rs` 的 5 个 `never used`、
`weixin/kimi.rs` 的 `useless format!` 等），与本次无关。

这个 warning 本身无害（不影响行为），但基线一旦破了，
以后就分不清「新引入的」和「一直都有的」，下次 review 会失去这个判据。

## 实现步骤

- [ ] **1.** 按 clippy 给的建议改 `server/src/team_task/card.rs:348`：

```rust
// 改前
other if other.is_empty() => "飞书派单".into(),

// 改后
"" => "飞书派单".into(),
```

注意这是个 `match` 分支，改完确认分支顺序与可达性不变
（`""` 是具体模式，必须在通配分支之前；如果原本它就在 `other` 通配之前，
位置不用动）。

- [ ] **2.** 若改完出现「unreachable pattern」或类型推导问题，
  说明这个 `match` 的结构与我预期不同 —— 此时改用另一种等价写法：
  把 guard 挪进分支体，或用 `if s.is_empty() { ... } else { ... }`。
  **以「clippy 干净 + 行为不变」为准，不必拘泥于具体写法。**

## 验收标准

```bash
cargo build --workspace
cargo clippy -p hank-server --all-targets 2>&1 | grep -cE '^warning: '
cargo clippy -p hank-server --all-targets 2>&1 | grep -E '^\s+--> server/src/team_task'
cargo test -p hank-server team_task
cargo test --workspace
```

期望结果：
- warning 总数回到 **46**
- 第二条命令**无输出**（`server/src/team_task/` 下零 warning）
- `cargo test -p hank-server team_task` **83 项**全绿（数量不变）
- `cargo test --workspace` 全绿
- `git diff --stat` 只列出 `server/src/team_task/card.rs`，且只有 1 行改动

## 明确边界

**只允许改 `server/src/team_task/card.rs` 的那一行。**

不要碰任何其他文件、不要顺手修其他既有 warning
（`weixin/kimi.rs` 的 `useless format!`、`deployment.rs` 的 `never used`
都是既有的，属于独立技术债，混进来会让这次改动不好 review）。

## 约定

- 中文 commit message，形如
  `fix(team-task): 清掉主卡构造里的 redundant guard`
