# 任务 A：因子草稿沉淀链路（save_draft 放开 + backfill skill）

| 属性 | 内容 |
|------|------|
| 任务 ID | `TASK-FACTOR-A-DRAFT-PIPELINE` |
| 上游需求 | `quant/docs/factor-research-requirements.md` §4.3（R3.1–R3.6）、§5 A 层 |
| 执行顺序 | 第 1 份（共 3 份：A → B → C），B/C 后续另发 |
| 工作目录 | `/Users/admin/projects/hank/trace/quant` |
| 语言约定 | 中文注释、中文 commit message（遵循 `trace/CLAUDE.md`） |

---

## 1. 背景与目标

### 现状（已核对代码）

quant 的因子研究链路在「草稿沉淀」这一环断成三截：

1. **存不下来**：`factor.save_draft` 要求 admin。拦截发生在两处 —
   `app/a2a/server.py:169-170` 的全局授权检查，以及 `app/a2a/skills/factor_save_draft.py:39-40`
   的 skill 内二次校验。普通研究会话（`can_client`）产出的因子无法落库。
2. **算不出来**：A2A 侧没有回填能力。`quant_factor_daily` 的写入只有
   REST `POST /api/factors/backfill`（`app/api/factors.py:410`，`require_admin`），
   且不在 Agent Card 上，agent 调不到。
3. **读为空**：存了草稿后 `quant_factor_daily` 里没有该 key 的值，
   之后按 `factor_key` 走 `factor.evaluate` 时
   `_load_saved_factor_values`（`app/factors/evaluation.py:176`）返回空 dict，
   最终以「有效样本过少」失败。

另外 `quant_factor_def` **没有归属列**。当前 admin-only 掩盖了这个问题；
一旦放开给 `can_client`，任何用户都能改删他人草稿和系统因子 —— 所以
归属列是放开权限的前提，不能省。

### 目标

非 admin 研究会话能在一个会话内跑通：
`factor.save_draft` → `factor.backfill` → `factor.evaluate`（按 `factor_key`）。

做完之后的可观察效果：

- 用 `can_client`（非 admin）token 调 `factor.save_draft` 成功返回 `factor_draft`，
  且 `enabled=false` / `is_system=false` / `owner_id` = 调用者。
- 新增 skill `factor.backfill` 出现在 Agent Card 上，`can_client` 可调，
  高成本（走 `confirmed=true` 闸门 + 日配额 + 单任务互斥）。
- 回填后 `quant_factor_daily.values` 里出现该草稿 key，
  再按 `factor_key` 调 `factor.evaluate` 不再报「有效样本过少」。
- 非 admin 回填系统因子 / 他人草稿 / 全量（`factor_key` 省略）时被明确拒绝。

### 边界（不变）

草稿始终 `enabled=false`，不进选股池；`enable` 只能人工在看板操作。
本任务放开的是「存」和「算」，不是「用」。

---

## 2. 涉及文件清单

**新增：**

| 文件 | 内容 |
|------|------|
| `alembic/versions/0027_factor_def_owner.py` | 给 `quant_factor_def` 加 `owner_id` / `parent_factor_key` 两列，回填存量行 |
| `app/a2a/skills/factor_backfill.py` | 新 skill `factor.backfill` 的 handler |
| `tests/test_factor_draft_pipeline.py` | 本任务的行为测试（详见 §4 步骤 9） |

**修改：**

| 文件 | 改什么 |
|------|--------|
| `app/models.py` | `FactorDef` 加 `owner_id` / `parent_factor_key` 字段 |
| `app/factors/defs.py` | `FactorDefSnapshot` 加两个字段，`_snapshot()` 同步 |
| `app/a2a/server.py` | 去掉 `factor.save_draft` 的 admin 门；`factor.backfill` 无需额外门（授权在 skill 内做） |
| `app/a2a/skills/factor_save_draft.py` | 去掉 admin 校验，写 `owner_id`，支持 `parent_factor_key`，artifact 补两个字段 |
| `app/a2a/skills/__init__.py` | 注册 `factor.backfill` |
| `app/a2a/card.py` | 新增 `factor.backfill` skill 条目；改 `factor.save_draft` 的 description（不再是 admin only） |
| `app/a2a/tasks.py` | `HIGH_COST_SKILLS` 与 `SKILL_TO_QUANT_TASK_TYPE` 加 `factor.backfill` |
| `app/factors/backfill.py` | `run_factor_backfill_task` 支持 `owner_id` 归属守卫参数 |
| `app/api/factors.py` | `_factor_out` 输出 `owner_id` / `parent_factor_key`；REST 创建时写 `owner_id` |
| `crates/code-tools/src/quant_tools.rs`（在 trace 根，非 quant 目录） | 注册 `quant_backfill_factor` 工具；改 `quant_save_factor_draft` 描述 |
| `server/skills/quant-research/SKILL.md`（trace 根） | 因子链路强制约束补回填步骤 |
| `quant/docs/a2a-design.md` | §8.12 授权改写；新增 §8.12a `factor.backfill` 契约 |

**不许碰：**

- `app/factors/engine.py`、`app/factors/evaluation.py` 的统计逻辑
  （横截面与相关性是 C / B 层的事，本任务只让 `factor_key` 路径读到值）
- `app/strategy/` 下任何文件（operators / compiler / spec 全部不动）
- `web/` 前端（本任务不含前端；`owner_id` 只在 API 输出，前端后续再用）
- 已有的 `alembic/versions/0001` ~ `0026`（只允许新增 0027）
- 工作区里与本任务无关的既有改动一律保留，不要 `git checkout` / `git stash`

---

## 3. 关键设计决定（照做，不要自行改口径）

### 3.1 `owner_id` 用 NOT NULL + 哨兵，不用 NULL

与 `quant_pool` / `quant_strategy` 完全一致（见 `alembic/versions/0011_pool_owner_and_grant.py`
的说明与 `app/models.py:623` 注释）：系统因子归哨兵
`SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"`（已在 `app/models.py:480` 定义）。

**注意**：需求文档 §4.3 R3.2 写的是「`owner_id`（可空，系统因子为 NULL）」，
本任务**刻意不采纳可空方案**，改用哨兵 + NOT NULL。理由与 0011 迁移里记录的一致：
NULL 会让归属判断散落成 `(owner_id IS NULL) OR (owner_id = :uid)`，
且与 Pool/Strategy 两处既有归属模型不一致。可见性判断统一为
`is_system OR owner_id == me`。

### 3.2 归属守卫的判定函数集中一处

在 `app/factors/defs.py` 里新增一个纯函数，两个调用方（save_draft 的改删路径、
backfill skill）都用它，不要各写一遍：

```python
def can_write_factor(def_: FactorDef | FactorDefSnapshot, *, user_id: str,
                     is_admin: bool) -> bool:
    """能否改动这个因子定义。admin 全通;普通用户只能动自己的非系统因子。"""
```

### 3.3 `factor.backfill` 是高成本 skill

全市场逐交易日计算，成本与 `factor.evaluate` 同级。因此：

- 加入 `HIGH_COST_SKILLS`（`app/a2a/tasks.py:37`）→ 自动获得
  `confirmed=true` 闸门、日配额计 1、`client_request_id` 幂等
- 加入 `SKILL_TO_QUANT_TASK_TYPE`，映射到既有 quant_task type `factor_backfill`
- 走 `submit_task`（`app/tasks.py:76`）→ 自动获得单任务互斥
- 复用 `_common.wait_for_task` 等待终态，与 `factor_evaluate.py:132` 同款

### 3.4 区间上限复用回测校验

`factor.backfill` 的 `start`/`end` 直接调
`app/backtest/validation.py:validate_backtest_window`（10 年上限、禁未来日），
与 `factor.evaluate` 同口径，不要自己写日期校验。

### 3.5 回填的归属守卫放在 task handler 里，不只放在 skill 层

`run_factor_backfill_task` 的 params 里带上 `owner_id` 与 `is_admin`，
handler 内部再校验一次。原因：REST 与 A2A 两条入口共用这个 handler，
只在 skill 层校验会留下 REST 侧的空档。REST 侧本来就 `require_admin`，
传 `is_admin=True` 即可，行为不变。

---

## 4. 实现步骤

### 步骤 1：`app/models.py` 加两列

在 `FactorDef`（`app/models.py:301`）里，`is_system` 之后加：

```python
    # 归属模型与 Pool / Strategy 一致(见 alembic 0011):可见性 =
    # is_system OR owner_id 是我。系统因子归 SYSTEM_OWNER_ID 哨兵,
    # 不用 NULL 表达「无主」。
    owner_id: Mapped[str] = mapped_column(
        String(36), nullable=False, default=SYSTEM_OWNER_ID, index=True,
    )
    # 变体溯源:与策略侧 parent_strategy_id 对齐,记录本因子从哪个因子派生。
    # 只存 key 不存 id:因子 key 是稳定的业务标识,且不加外键(父因子可被删)。
    parent_factor_key: Mapped[str | None] = mapped_column(
        String(64), nullable=True,
    )
```

`SYSTEM_OWNER_ID` 定义在同文件 `app/models.py:480`，位置在 `FactorDef` 之后 —
把常量定义**上移**到 `FactorDef` 之前（模块级常量，移动无副作用），
或在 default 处用 lambda 延迟取值。二者择一，不要重复定义常量。

### 步骤 2：`alembic/versions/0027_factor_def_owner.py`

`down_revision = "0026_factor_eval_neutralize"`。参照
`alembic/versions/0011_pool_owner_and_grant.py` 的 `batch_alter_table` 写法
（SQLite 测试库需要 batch 模式）。

```python
"""因子定义归属与谱系:owner_id + parent_factor_key

owner_id NOT NULL + 哨兵而非可空:与 quant_pool / quant_strategy 的归属模型
保持一致(见 0011),可见性统一为 is_system OR owner_id 是我,避免每处查询
重复 NULL 判断。存量行按 is_system 分流回填:系统因子归哨兵,自定义因子
(历史上只有 admin 能建)也归哨兵 —— 无法追溯真实创建者,归系统等价于
「仅 admin 可改」,与放开前的行为一致,不会误把他人草稿判给某个用户。

Revision ID: 0027_factor_def_owner
Revises: 0026_factor_eval_neutralize
"""
SYSTEM_OWNER_ID = "00000000-0000-0000-0000-000000000000"


def upgrade() -> None:
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.add_column(sa.Column("owner_id", sa.String(36), nullable=True))
        batch.add_column(sa.Column("parent_factor_key", sa.String(64),
                                   nullable=True))
    op.execute(sa.text(
        "UPDATE quant_factor_def SET owner_id = :sys"
    ).bindparams(sys=SYSTEM_OWNER_ID))
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.alter_column("owner_id", existing_type=sa.String(36),
                           nullable=False)
        batch.create_index("ix_quant_factor_def_owner_id", ["owner_id"])


def downgrade() -> None:
    with op.batch_alter_table("quant_factor_def") as batch:
        batch.drop_index("ix_quant_factor_def_owner_id")
        batch.drop_column("parent_factor_key")
        batch.drop_column("owner_id")
```

### 步骤 3：`app/factors/defs.py`

1. `FactorDefSnapshot` 加 `owner_id: str` 与 `parent_factor_key: str | None`
   两个字段（放在 `is_system` 之后，`created_at` 之前）。
2. `_snapshot()` 里补 `owner_id=row.owner_id or SYSTEM_OWNER_ID`、
   `parent_factor_key=row.parent_factor_key`。
3. 新增 §3.2 的 `can_write_factor()`，并加进 `__all__`。

### 步骤 4：`app/a2a/skills/factor_save_draft.py` 放开权限

1. 删掉 `handle()` 开头的 `if not ctx.claims.get("can_admin")` 分支。
2. `enabled=true` 的拒绝、保留字段冲突检查、表达式校验、key 冲突处理**全部保留不变**。
3. 新增 `parent_factor_key` 处理：payload 里给了就校验父因子存在且当前用户可读
   （`is_system` 或 `owner_id == ctx.user_id`），不可读则
   `raise ValueError(f"parent_factor_key {key} 不存在或不可读")`。
   参照 `app/a2a/skills/strategy_save_draft.py:34-42` 的写法。
4. 构造 `FactorDef` 时加 `owner_id=ctx.user_id`、
   `parent_factor_key=parent_factor_key`。
5. `_factor_out()` 补 `"owner_id"` 与 `"parent_factor_key"` 两个键。
6. 落库成功后调 `invalidate_factor_cache()` —— 现有代码漏了这一步，
   REST 的 create/patch/delete 都调了（`app/api/factors.py:239` 等），
   A2A 侧不调会让新草稿在 60s 内对 `load_all_defs` 不可见，
   紧接着的 backfill 会找不到它。

### 步骤 5：`app/a2a/server.py` 去掉 admin 门

删掉 `app/a2a/server.py:169-170`：

```python
            if skill == "factor.save_draft" and not can_admin(claims):
                raise ValueError("factor.save_draft 仅管理员可用")
```

`system.gap_summary` 的 scope 检查保留。若 `can_admin` 因此变成未使用的导入，
检查其它引用点后再决定是否从 import 里去掉（`_audit_info` 等处可能仍在用）。

### 步骤 6：`app/factors/backfill.py` 加归属守卫

`run_factor_backfill_task` 里，取出 `factor_key` 之后、加载 defs 的位置：

```python
    owner_id = params.get("owner_id")
    is_admin = bool(params.get("is_admin"))

    if factor_key:
        def_ = db.execute(
            select(FactorDef).where(FactorDef.key == factor_key)
        ).scalar_one_or_none()
        if def_ is None:
            raise ValueError(f"因子 {factor_key} 不存在")
        # 归属守卫:非 admin 只能回填自己的非系统因子。owner_id 为 None 表示
        # 调用方未声明身份(REST admin 入口),按 is_admin 处理。
        if owner_id is not None and not can_write_factor(
            def_, user_id=owner_id, is_admin=is_admin,
        ):
            raise ValueError(f"无权回填因子 {factor_key}")
        defs = [def_]
    else:
        if owner_id is not None and not is_admin:
            raise ValueError("回填全部启用因子仅管理员可用")
        defs = load_enabled_defs(db)
```

REST 入口（`app/api/factors.py:410` 的 `backfill_factors`）的 `params` 里
补 `"owner_id": user_id, "is_admin": True` —— 该端点已是 `require_admin`，
行为不变，只是让 handler 拿到一致的入参形状。

### 步骤 7：新增 `app/a2a/skills/factor_backfill.py`

结构照 `app/a2a/skills/factor_evaluate.py` 写（同样是「提交 quant_task → 等待终态」）。
注意 `factor_backfill` 的 handler 已经在 `app/factors/backfill.py:190` 用
`register_handler` 注册过了，**不要重复注册**，也不要给它包一层新的 task type。

```python
"""factor.backfill skill:把因子草稿的历史值算进 quant_factor_daily。"""
from __future__ import annotations

from datetime import date
from typing import Any

from sqlalchemy import select

from ...backtest.validation import validate_backtest_window
from ...factors.defs import can_write_factor
from ...models import FactorDef
from ...tasks import TaskConflictError, submit_task
from ._common import A2AContext, wait_for_task


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """提交 factor_backfill 类型 quant_task 并等待完成。

    只允许回填自己的 disabled 草稿:回填是全市场逐日写库,放开给任意因子
    等于让普通用户覆盖系统因子的历史值。
    """
    factor_key = payload.get("factor_key")
    if not factor_key:
        raise ValueError("factor_key 必填;回填全部启用因子仅管理员可用,请指定单个因子")

    is_admin = bool(ctx.claims.get("can_admin"))
    def_ = ctx.db.execute(
        select(FactorDef).where(FactorDef.key == str(factor_key))
    ).scalar_one_or_none()
    if def_ is None:
        raise ValueError(f"因子 {factor_key} 不存在")
    if not can_write_factor(def_, user_id=ctx.user_id, is_admin=is_admin):
        raise ValueError(f"无权回填因子 {factor_key}:只能回填自己保存的草稿")
    if def_.enabled and not is_admin:
        raise ValueError(
            f"因子 {factor_key} 已启用,回填会改动夜间管道读取的因子值,仅管理员可操作"
        )

    start = date.fromisoformat(str(payload["start"]))
    end = date.fromisoformat(str(payload["end"]))
    validate_backtest_window(start, end)

    codes = payload.get("codes")
    if codes:
        codes = [str(c).lower() for c in codes]

    try:
        task = submit_task(
            ctx.db,
            user_id=ctx.user_id,
            type="factor_backfill",
            title=f"factor backfill · {factor_key}",
            params={
                "factor_key": str(factor_key),
                "start": start.isoformat(),
                "end": end.isoformat(),
                "codes": codes,
                "owner_id": ctx.user_id,
                "is_admin": is_admin,
                "client_request_id": payload.get("client_request_id"),
            },
        )
    except TaskConflictError as exc:
        raise ValueError(str(exc)) from exc

    wait_for_task(ctx.db, task, cancel_event)
    if task.status != "done":
        raise ValueError(task.error or "因子回填执行失败")

    result = task.result or {}
    return {
        "factor_backfill": {
            "factor_key": str(factor_key),
            "window": {"start": start.isoformat(), "end": end.isoformat()},
            "days": result.get("days", 0),
            "rows_written": result.get("rows_written", 0),
            "skipped": result.get("skipped", 0),
            "detail_ref": {"task_id": task.id},
        }
    }


__all__ = ["handle"]
```

**注意 artifact 形状**：`run_factor_backfill_task` 返回的是裸 dict
（`{"days":..., "rows_written":..., "factors":..., "skipped":...}`），
不是 artifact 形状。而 `factor_evaluate.py` 的 task handler 返回的已经是
artifact。所以这里必须由 skill 把裸 dict 包成 `factor_backfill` artifact，
不要改 `run_factor_backfill_task` 的返回值（REST 侧在用）。

### 步骤 8：注册与 Card

**`app/a2a/skills/__init__.py`**：import `factor_backfill`，
`SKILLS` 里加 `"factor.backfill": factor_backfill.handle`。

**`app/a2a/tasks.py`**：
- `HIGH_COST_SKILLS` 加 `"factor.backfill"`
- `SKILL_TO_QUANT_TASK_TYPE` 加 `"factor.backfill": "factor_backfill"`

**`app/a2a/card.py`**：在 `factor.save_draft` 条目之后插入：

```python
            {
                "id": "factor.backfill",
                "name": "Backfill factor history",
                "description": "Compute and persist historical daily values for one's own factor draft into quant_factor_daily, so later factor.evaluate by factor_key has data to read. Own non-system drafts only. Long-running, high-cost. Requires confirmed=true.",
                "tags": ["factor", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
```

同时把 `factor.save_draft` 的 description 从
`"Persist disabled factor definition. Admin only. No confirmation."`
改为
`"Persist disabled factor definition owned by the caller. Optional parent_factor_key for lineage. Always enabled=false. No confirmation. Follow with factor.backfill before evaluating by factor_key."`

Card 与 `SKILL_IDS` 的一致性由 `tests/test_a2a.py:184-186` 断言，改漏会红。

### 步骤 9：测试 `tests/test_factor_draft_pipeline.py`

在 `tests/test_a2a.py` 的夹具风格基础上写（可 `from tests.test_a2a import ...`
复用 `_token` / `CLIENT_CLAIMS` / `ADMIN_CLAIMS`，或直接照抄夹具；
优先直接 import 复用，避免夹具漂移）。必须覆盖：

1. `test_save_draft_allows_client_and_records_owner`
   非 admin token 保存草稿成功，artifact 里 `enabled=false`、
   `owner_id` == 该用户 sub、`is_system=false`。
2. `test_save_draft_still_rejects_enabled_true`（保留既有行为，
   把 `tests/test_a2a.py:469` 的用例改成用 client token 也应通过）
3. `test_save_draft_rejects_unreadable_parent`
   `parent_factor_key` 指向不存在的 key → failed，错误含「不存在或不可读」。
4. `test_backfill_rejects_other_users_draft`
   用户 A 存草稿，用户 B 回填 → failed，错误含「无权回填」。
5. `test_backfill_rejects_system_factor_for_client`
   非 admin 回填 seed 的系统因子（如 `mom20`）→ failed。
6. `test_backfill_requires_factor_key`
   省略 `factor_key` → failed，错误含「仅管理员可用」。
7. `test_backfill_is_high_cost`
   不带 `confirmed=true` 调用 → failed，错误含「高成本」。
8. `test_draft_pipeline_end_to_end`
   非 admin：save_draft → backfill（`confirmed=true`）→ 断言
   `quant_factor_daily` 里出现该 key 的值。需要 seed 日线；
   参照 `tests/test_factor_backfill.py:39` 的 `_seed_bars` 与
   `tests/test_a2a.py` 里已有的 bar seeding 工具。
   评估那一步（`factor.evaluate`）可以只断言「不再因读空而失败」，
   或直接断言 `_load_saved_factor_values` 返回非空 —— 不必跑完整评估，
   跑全量评估在测试里太慢。

`tests/test_a2a.py:454` 的 `test_factor_save_draft_requires_admin` 会因本改动失效，
**改写它**（不是删掉）：改成断言 `can_client` 可用、`NOBODY_CLAIMS`（既非 client
也非 admin）被拒。

**`tests/test_schema.py:307` 的 `test_factor_def_shape` 一定会红** ——
它断言 `set(columns) == expected` 精确列集合。必须在 `expected` 里补
`"owner_id"`、`"parent_factor_key"`，并补两条断言：
`owner_id` 类型 `VARCHAR(36)` 且 `nullable is False`；
索引名集合里有含 `owner_id` 的项。这是**预期的测试修改**，不是绕过失败。

### 步骤 10：Trace 侧工具注册（`/Users/admin/projects/hank/trace/crates/code-tools/src/quant_tools.rs`）

在 `quant_save_factor_draft` 条目之后插入新工具（照该文件既有 `mk(ToolSpec{...})` 形状）：

```rust
        mk(
            ToolSpec {
                tool_name: "quant_backfill_factor",
                skill: "factor.backfill",
                description: "把自己保存的因子草稿的历史值算进因子日值表。保存草稿后必须先回填，否则按 factor_key 评估会读空并以「有效样本过少」失败。只能回填自己的草稿。模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "factor_key": { "type": "string", "description": "必填，只能是自己保存的草稿 key" },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["factor_key", "start", "end"]
                }),
                high_cost: true,
                artifact_name: "factor_backfill",
            },
            |input| {
                let key = input["factor_key"].as_str().unwrap_or("?");
                let start = input["start"].as_str().unwrap_or("?");
                let end = input["end"].as_str().unwrap_or("?");
                format!("回填因子 {}，区间 {} ~ {}", key, start, end)
            },
        ),
```

同时：
- 把 `quant_save_factor_draft` 的 description 从「仅 admin 可用」改为
  「保存因子草稿（enabled=false，归属调用者）。可带 parent_factor_key 记录变体谱系。保存后需 quant_backfill_factor 回填历史值才能按 factor_key 评估。」，
  input_schema 的 properties 补 `"parent_factor_key": { "type": ["string", "null"] }`
- 文件头 `/// 一次性构造全部 21 个 quant_* 工具。` 改为 22
- `test_high_cost_tools_delegate_confirmation_to_runtime_gate`（约 1012 行的数组）
  加上 `"quant_backfill_factor"`

### 步骤 11：文档同步

**`quant/docs/a2a-design.md`**：
- §8.12 的授权行「**仅 `can_admin`**」改为「`can_client`；`owner_id` 记调用者，
  `is_system` 固定 false」，并补 `parent_factor_key` 说明
- §8.12 之后新增 §8.12a `factor.backfill`：payload（`factor_key` 必填 /
  `start` / `end` / `codes?` / `confirmed` / `client_request_id`）、
  规则表（授权 = `can_client` 且只能自己的非系统草稿、高成本走闸门与配额、
  互斥槽与 `factor.evaluate` 共用、区间上限 10 年）、
  artifact `factor_backfill` 形状
- 第 245 行「免确认 skill 清单」不含 `factor.backfill`（它是高成本），
  但 §8.12 `factor.save_draft` 仍在清单里，无需改
- 第 1470 行授权总表：把 `factor.save_draft` 从 admin 行挪到 client 行
- 第 1437 行附近的文件树补 `factor_backfill.py`

**`/Users/admin/projects/hank/trace/server/skills/quant-research/SKILL.md`**：
第 31 条强制约束（因子提炼链路）改为：

```
12. **因子提炼必须走完整链路**：`quant_list_factor_evaluations`（先查历史，避免重复烧配额）→ `quant_validate_factor` → `quant_preview_factor`（≤5 标的抽查）→ `quant_evaluate_factor`（高成本须授权）→ 可选 `quant_save_factor_draft` + `quant_backfill_factor`。**保存草稿后若要按 `factor_key` 复用，必须先 `quant_backfill_factor` 回填历史值**：草稿刚存下时因子日值表里没有它的值，直接按 `factor_key` 评估会以「有效样本过少」失败。回填是高成本操作，只能回填自己的草稿。evaluate 摘要必须带样本期、`n_periods`、覆盖率、中性化口径、IC t 值与多重检验提示，禁止说「未来持续有效」。
```

---

## 5. 验收标准

按顺序执行，全部通过：

```bash
cd /Users/admin/projects/hank/trace/quant

# 1. 迁移链在全新库上能跑通（0027 是新 head）
uv run alembic -x db_url=sqlite+pysqlite:///$(mktemp -d)/t.db upgrade head

# 2. 本任务新测试
uv run pytest tests/test_factor_draft_pipeline.py -v

# 3. 受影响的既有测试
uv run pytest tests/test_a2a.py tests/test_factor_api.py \
    tests/test_factor_backfill.py tests/test_factor_evaluation.py \
    tests/test_schema.py -q

# 4. 全量回归（本任务不该动统计与策略，这里应无新增失败）
uv run pytest -q
```

```bash
cd /Users/admin/projects/hank/trace

# 5. Rust 侧工具注册
cargo test -p code-tools quant
cargo build --workspace
```

**期望结果**：

- 步骤 1 输出 `Running upgrade 0026_factor_eval_neutralize -> 0027_factor_def_owner`
- 步骤 2 全绿，8 个用例
- 步骤 3 全绿
- 步骤 4：**改动前基线为 `828 passed`（已实测，无失败无跳过）**。
  改完后总数应为 `828 + 新增用例数`，且失败数为 0。出现任何失败都必须修，
  不得以「本来就红」为由放过
- 步骤 5：**改动前基线为 `18 passed; 0 failed`（已实测）**，改完后应为 18 passed
  （新增工具不新增用例，只是让既有的高成本工具用例多断言一项）

**人工核对项**（无法自动断言，写进 commit message 或回报里）：

- Agent Card `GET /.well-known/agent-card.json` 的 skills 数量 = `len(SKILL_IDS)` = 22
- `quant_tools.rs` 的工具数 = 22

---

## 6. 约定

- 中文注释，注释解释**为什么**而不是复述代码在做什么
- commit message 用中文，形如
  `feat(quant): 因子草稿放开给研究会话，新增 factor.backfill 回填 skill`
- 不新增依赖
- 保留工作区原有改动，只提交与本任务相关的文件
- 遇到与本文档冲突的既有实现，以**既有实现为准**并在回报里说明冲突点，
  不要为了对齐文档去改无关代码
