# 任务 B：因子相关性与正交性（`factor.correlation`）

| 属性 | 内容 |
|------|------|
| 任务 ID | `TASK-FACTOR-B-CORRELATION` |
| 上游需求 | `quant/docs/factor-research-requirements.md` §4.2（R2.1–R2.6）、§5 B 层 |
| 执行顺序 | 第 2 份（A → **B** → C）。与 A 无硬依赖，但**必须在 A 之后执行** |
| 工作目录 | `/Users/admin/projects/hank/trace/quant` |
| 语言约定 | 中文注释、中文 commit message（遵循 `trace/CLAUDE.md`） |

> **执行前置**：本文档假设任务 A 已合并，因此
> `alembic` head 是 `0027_factor_def_owner`，本任务的迁移接在它后面。
> 若 A 尚未合并，把 `down_revision` 改成当时的真实 head 并在回报里说明。

---

## 1. 背景与目标

### 现状（已核对代码）

quant 现在能对**单个**因子给出可信的结论：`factor.evaluate` 有 IC / RankIC /
ICIR、Newey-West t 值（`app/factors/evaluation.py:365`）、行业与市值中性化
（同文件 `:310`）、IC 衰减曲线与 Bonferroni 粗校正。

但**没有任何工具能回答「这个新因子相对已有因子有增量吗」**。agent 跑出第二个
因子时，只能拿两份独立的评估报告做「IC 都是正的」这种无意义比较。后果很具体：
20 日动量和 22 日动量会被当成两个因子分别汇报，而它们本质是同一个信息。

### 目标

新增高成本 skill `factor.correlation`，输入一个待检因子 + 一组对照因子，
输出三层判据：

1. **因子值相关性** —— 逐调仓日截面的 Pearson / Spearman，报时序均值与稳定性
2. **IC 相关性** —— 两因子 IC 序列之间的相关性。因子值低相关但 IC 高相关，
   意味着捕捉的是同一个收益来源，这是单看因子值相关性会漏掉的情况
3. **正交化增量 IC** —— 待检因子对对照因子集做截面回归取残差，报残差的 IC
   与 Newey-West t 值。**这是核心判据：残差 IC 不显著 = 无增量**

做完之后的可观察效果：

- 对 5 日动量 vs 10 日动量这类已知高相关的因子对，报出高 |ρ| 且残差 IC 不显著
- 对一组已知低相关的因子对，报出低 |ρ| 且残差 IC 仍显著
- 结果落 `quant_factor_correlation` 表，可按 id 复查
- 新增只读 skill `factor.correlation_get` 按 id 取详情

### 边界

- 本任务**不做**因子合成与权重优化（多因子打分模型），那是需求 §3 的后续候选
- 本任务**不动**求值引擎与算子表（横截面表达力是 C 层）。因此对照因子与待检
  因子都走**既有**的时序求值路径：`factor_key` 读 `quant_factor_daily`，
  `expression` 现算
- 不引入新依赖，回归用 `numpy.linalg.lstsq`，与 `_neutralize_cross_section` 同款

---

## 2. 涉及文件清单

**新增：**

| 文件 | 内容 |
|------|------|
| `alembic/versions/0028_factor_correlation.py` | 建 `quant_factor_correlation` 表 |
| `app/factors/correlation.py` | 相关性与正交化的全部统计逻辑（domain 层） |
| `app/a2a/skills/factor_correlation.py` | skill `factor.correlation`（高成本，提交 quant_task） |
| `app/a2a/skills/factor_correlation_get.py` | skill `factor.correlation_get`（只读） |
| `tests/test_factor_correlation.py` | 统计逻辑的行为测试（构造已知相关/正交数据） |

**修改：**

| 文件 | 改什么 |
|------|--------|
| `app/models.py` | 新增 `FactorCorrelation` 模型 |
| `app/a2a/skills/__init__.py` | 注册两个新 skill |
| `app/a2a/card.py` | 两个新 skill 条目 |
| `app/a2a/tasks.py` | `HIGH_COST_SKILLS` 与 `SKILL_TO_QUANT_TASK_TYPE` 加 `factor.correlation` |
| `app/factors/__init__.py` | 导出新 domain 函数（照该文件既有风格） |
| `crates/code-tools/src/quant_tools.rs`（trace 根） | 注册 `quant_factor_correlation` / `quant_get_factor_correlation` |
| `server/skills/quant-research/SKILL.md`（trace 根） | 新增判读口径约束（见 §4 步骤 8） |
| `quant/docs/a2a-design.md` | 新增 §8.13b `factor.correlation` 契约 |

**不许碰：**

- `app/factors/evaluation.py` 的既有函数签名与统计口径。**只允许 import 复用**
  （`_ic_series` / `_newey_west_tstat` / `_load_saved_factor_values` /
  `_load_expression_factor_values` / `_resolve_universe` / `_load_price_matrix` /
  `_is_eligible` / `_stocks_for_codes` / `_trading_days` / `_rebalance_dates`），
  不允许改它们的行为。这些是带下划线的模块私有函数，跨模块复用时
  **在 `app/factors/evaluation.py` 的 `__all__` 里补上你要用的名字**，
  不要用 `from ... import _private` 绕过（会被 lint 与后人误删）
- `app/strategy/` 下任何文件
- `app/factors/engine.py`
- A 层交付的 `owner_id` / `factor.backfill` 相关代码
- 已有 alembic revision（只允许新增 0028）
- 工作区里与本任务无关的既有改动一律保留

---

## 3. 关键设计决定（照做，不要自行改口径）

### 3.1 复用 `factor.evaluate` 的截面循环骨架，但不合并两个函数

相关性计算需要的前置和 `evaluate_factor_efficacy` 高度重合：解析评估域 →
价格矩阵 → 调仓日 → 逐期截面取有效样本。**照抄这段骨架到
`app/factors/correlation.py`，不要去重构 `evaluation.py` 把公共部分抽出来。**

理由：`evaluate_factor_efficacy` 是已上线的生产路径，抽公共函数会同时改动
两条链路的行为，风险远大于 100 行重复。需求 §6 R1 明确要求「只允许新增
调用方，不改既有签名与语义」，这里适用同样的原则。

### 3.2 中性化口径必须与对照因子一致，并落库

相关性只有在「两个因子用同一口径算出来」时才有意义。所以：

- `neutralize` 参数与 `factor.evaluate` 同款（`normalize_neutralize` 复用）
- 待检因子与所有对照因子**在同一期截面上用同一 modes 做中性化**
- `neutralize` 必须落到 `quant_factor_correlation` 行里 —— 与
  `0026_factor_eval_neutralize.py` 的迁移说明同理：不留口径，
  两次结果的差异无法归因

### 3.3 残差 IC 是核心判据，回归设计矩阵的构造

对每个调仓日截面：

```
y = 待检因子值（已按 modes 中性化）
X = [1, 对照因子1, 对照因子2, ...]（每列同样已中性化）
残差 = y - X @ lstsq(X, y)
```

然后把逐期残差与**下一期收益**算 IC，得到残差 IC 序列，再过
`_newey_west_tstat` 得 t 值与 p 值。

失败降级规则，与 `_neutralize_cross_section:310` 保持一致：
样本 < 5、设计矩阵奇异、行数 ≤ 列数、残差含非有限值 → **跳过该期**
（不是「原样返回」）。相关性场景下原样返回等于把裸因子当残差，
会得出「有增量」的错误结论，比少一期样本危险得多。跳过的期数要落到
结果里（`residual.skipped_periods`），让 agent 知道样本损失。

### 3.4 对照因子集上限 20，且默认不取全部 enabled

需求 §6 R5 建议对照因子集 ≤20。本任务定为：

- `factor_keys` 显式传入，**最多 20 个**，超出直接报错
- `factor_keys` 省略时取**全部 enabled 系统因子**，但若数量 > 20
  则报错要求显式指定，不静默截断（静默截断会让「对照了哪些」不可复现）
- 待检因子若也在 `factor_keys` 里，剔除它并在结果里记 `note`
  （自己对自己回归残差恒为 0，不报错但要说明）

### 3.5 成本定级与 `factor.evaluate` 同级

加入 `HIGH_COST_SKILLS`：走 `confirmed=true` 闸门、日配额计 1、
`client_request_id` 幂等、`submit_task` 单任务互斥。新增 quant_task
type `factor_correlation`。

**注意需求 §6 R6 的既有约束**：`submit_task` 是全局单任务互斥，
且 Trace 侧 `QUANT_LONG_TIMEOUT=600s`。对照 20 个因子 × 多年区间可能撞
600s 超时。本任务**不解决**这个约束，但要做两件事：

1. 结果里带 `elapsed_seconds`，让超时问题可观测
2. skill 的错误文案在样本量大时提示「可缩小区间或减少对照因子数」

### 3.6 只读 skill 用 `correlation_get`，不并入 `evaluation_list`

需求 §4.2 R2.6 给了「配套只读 skill 或并入 `factor.evaluation_list`」两个
选项。**选前者**：`evaluation_list` 的 summary 形状是围绕 IC/分层设计的
（`app/factors/listing.py:50`），塞进相关性矩阵会让两种结果的字段互相污染。
新增独立的 `factor.correlation_get`，形状自洽。

不做 `correlation_list`：相关性结果是「跑完就看」的，不像评估那样需要
横向对比多轮；真需要再补。

---

## 4. 实现步骤

### 步骤 1：`app/models.py` 新增 `FactorCorrelation`

放在 `FactorEvaluation`（`app/models.py:937`）之后，字段照它的风格：

```python
class FactorCorrelation(Base):
    """因子相关性与正交性检验结果:因子值相关、IC 相关、正交化残差 IC。

    与 FactorEvaluation 分表:两者的核心判据不同(单因子有效性 vs 相对增量),
    塞一张表会让 result JSON 的形状随 kind 分叉,查询与前端都要分支。
    """

    __tablename__ = "quant_factor_correlation"

    id: Mapped[int] = mapped_column(_BIG_PK, primary_key=True, autoincrement=True)
    user_id: Mapped[str] = mapped_column(String(36), nullable=False, index=True)
    # 待检因子:key 与 expression 二选一,与 FactorEvaluation 同口径
    factor_key: Mapped[str | None] = mapped_column(String(64), nullable=True, index=True)
    expression: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    expression_hash: Mapped[str | None] = mapped_column(String(64), nullable=True)
    # 对照因子集:必须落库,否则「相对什么无增量」这个结论不可复现
    benchmark_keys: Mapped[list] = mapped_column(JSON, nullable=False)
    start: Mapped[date] = mapped_column(Date, nullable=False)
    end: Mapped[date] = mapped_column(Date, nullable=False)
    pool_id: Mapped[int | None] = mapped_column(Integer, nullable=True)
    codes: Mapped[list | None] = mapped_column(JSON, nullable=True)
    rebalance: Mapped[str] = mapped_column(String(16), nullable=False)
    neutralize: Mapped[list | None] = mapped_column(JSON, nullable=True)
    universe: Mapped[dict] = mapped_column(JSON, nullable=False)
    result: Mapped[dict | None] = mapped_column(JSON, nullable=True)
    status: Mapped[str] = mapped_column(
        String(16), default="done", nullable=False, index=True,
    )
    error: Mapped[str | None] = mapped_column(Text, nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, default=datetime.now)
    finished_at: Mapped[datetime | None] = mapped_column(DateTime, nullable=True)
```

### 步骤 2：`alembic/versions/0028_factor_correlation.py`

`down_revision = "0027_factor_def_owner"`。建表，列与步骤 1 一一对应，
索引：`user_id`、`factor_key`、`status`。参照
`alembic/versions/0025_a2a_tables.py` 的建表写法。

`tests/test_schema.py` 有逐表结构断言的惯例（见 `test_factor_def_shape`），
**给新表补一个 `test_factor_correlation_shape`**，断言列集合、
关键列类型与可空性、索引存在。

### 步骤 3：`app/factors/evaluation.py` 补 `__all__` 导出

只改 `__all__`，不改任何函数体。把本任务要复用的名字加进去：

```python
    "_ic_series",
    "_is_eligible",
    "_load_expression_factor_values",
    "_load_price_matrix",
    "_load_saved_factor_values",
    "_newey_west_tstat",
    "_rebalance_dates",
    "_resolve_universe",
    "_stocks_for_codes",
    "_trading_days",
```

并在 `__all__` 上方加一行注释说明为什么导出私有名：

```python
# 下划线前缀的名字同样导出:correlation.py 需要与本模块**逐字一致**的截面
# 取样与统计口径。复制一份实现会让两处的 IC 定义悄悄漂移,那比暴露私有名危险。
```

### 步骤 4：`app/factors/correlation.py`（本任务的主体）

模块结构（函数签名照下面写，实现细节自行补全）：

```python
"""因子相关性与正交性检验。

三层判据:因子值截面相关 → IC 序列相关 → 正交化残差 IC。核心结论是残差 IC:
两个因子的裸 IC 都好看不代表有增量,只有对已有因子回归取残差后 IC 仍显著,
才说明带来了新信息。
"""

MAX_BENCHMARKS = 20
# 单期截面回归的最小样本:与 _neutralize_cross_section 的门槛一致
MIN_CROSS_SECTION = 5
# |ρ| 超过这个值算「高相关」,用于统计稳定性占比。0.7 是因子研究常用阈值,
# 不是统计定理 —— 结论里必须把原始 ρ 一并给出,不能只报占比。
HIGH_CORR_THRESHOLD = 0.7


def _pair_correlation(a: np.ndarray, b: np.ndarray) -> tuple[float | None, float | None]:
    """单期截面上两个因子值的 (Pearson, Spearman)。样本不足或零方差返回 None。"""


def _orthogonalize(
    target: np.ndarray, benchmarks: list[np.ndarray],
) -> np.ndarray | None:
    """target 对 benchmarks 做截面回归,返回残差。

    无法可靠求解时返回 None(调用方跳过该期),不返回原值 —— 把裸因子当残差
    会直接得出「有增量」的错误结论。
    """


def compute_factor_correlation(
    db: Session,
    *,
    user_id: str,
    expression: dict | None = None,
    factor_key: str | None = None,
    benchmark_keys: list[str],
    start: date,
    end: date,
    pool_id: int | None = None,
    codes: list[str] | None = None,
    rebalance: str = "weekly",
    neutralize: list[str] | None = None,
    cancel_event: threading.Event | None = None,
) -> FactorCorrelation:
    """计算相关性与正交性并落库,返回 FactorCorrelation 行。

    取消检查点与 evaluate_factor_efficacy 一致:按标的批次与每个调仓日截面
    检查 cancel_event。
    """
```

`compute_factor_correlation` 的流程：

1. 参数校验：`expression` / `factor_key` 恰好一个（照
   `evaluation.py:613` 的写法）；`rebalance ∈ {weekly, monthly}`；
   `validate_backtest_window(start, end)`；`benchmark_keys` 去重、
   剔除自身、非空、`≤ MAX_BENCHMARKS`
2. 对照因子必须都存在于 `quant_factor_def`，缺失的直接报错列出哪些缺
   （不要静默跳过 —— agent 会以为对照过了）
3. 先落 `status="running"` 行（照 `evaluation.py:649`），便于查进度
4. 解析评估域 / 价格矩阵 / 交易日 / 调仓日（复用步骤 3 导出的函数）
5. 载入待检因子值：`factor_key` → `_load_saved_factor_values`；
   `expression` → `_load_expression_factor_values`
6. 载入每个对照因子的值：全部走 `_load_saved_factor_values`
   （对照因子都是已落库的 `factor_key`）
7. **逐调仓日**：取该期同时有「待检因子值 + 全部对照因子值 + 次期收益」
   的股票交集作为样本；样本 < `MIN_CROSS_SECTION` 跳过。
   按 modes 对待检与每个对照因子分别做 `_neutralize_cross_section`。
   然后：
   - 对每个对照因子算 `_pair_correlation` → 累积到该对照的 ρ 序列
   - 算待检因子的 IC（`_ic_series`）→ 累积
   - 算每个对照因子的 IC → 累积（IC 相关性要用）
   - `_orthogonalize` 得残差 → 残差与次期收益算 IC → 累积；
     返回 None 时 `skipped_periods += 1`
8. 汇总：
   - 每个对照因子一条 `pairs[]` 记录：`factor_key`、`pearson_mean`、
     `pearson_std`、`spearman_mean`、`spearman_std`、
     `high_corr_ratio`（|ρ| > 阈值的期数占比）、`n_periods`、
     `ic_correlation`（两条 IC 序列的 Pearson）
   - `residual`：`ic_mean`、`rank_ic_mean`、`ic_t_stat`、`ic_p_value`
     （`_newey_west_tstat`）、`n_periods`、`skipped_periods`
   - `raw`：待检因子自己的 `ic_mean` / `ic_t_stat` / `ic_p_value`，
     供与残差对比
   - `verdict`：**服务端给出判定，不让 agent 自己解读**。取值：
     `no_increment`（残差 IC p ≥ 0.05 或 n_periods < 6）、
     `has_increment`（残差 p < 0.05 且与裸 IC 同号）、
     `inconclusive`（样本不足或 skipped 过半）。
     同时给 `verdict_reason` 中文说明
   - `disclaimer`：固定文案，样本内统计、未扣成本、非投资建议
9. 落库 `status="done"`、`finished_at`，返回行

**关于 `verdict` 由服务端给**：这与 `factor.evaluate` 只给数字、
判读写在 SKILL.md 的做法不同，是刻意的。残差 IC 的判读比 IC 均值更容易被
误读成「裸 IC 好看所以推荐」，需求 §4.2 的判读口径也明确要求
「残差 IC 不显著 → 明确说无增量」。把判定固化在服务端，agent 只能转述。

### 步骤 5：`app/a2a/skills/factor_correlation.py`

照 `app/a2a/skills/factor_evaluate.py` 的结构：注册 quant_task handler
+ `handle()` 提交并等待。

```python
def _factor_correlation_handler(db, task, *, cancel_event=None):
    """quant_task handler：执行相关性检验。"""
    p = task.params or {}
    row = compute_factor_correlation(
        db,
        user_id=task.user_id,
        expression=p.get("expression"),
        factor_key=p.get("factor_key"),
        benchmark_keys=p["benchmark_keys"],
        start=date.fromisoformat(p["start"]),
        end=date.fromisoformat(p["end"]),
        pool_id=p.get("pool_id"),
        codes=p.get("codes"),
        rebalance=p["rebalance"],
        neutralize=p.get("neutralize"),
        cancel_event=cancel_event,
    )
    return _build_correlation_artifact(row)


register_handler("factor_correlation", _factor_correlation_handler,
                 supports_cancel=True)
```

artifact 名 `factor_correlation`，形状：

```python
{
  "factor_correlation": {
    "correlation_id": row.id,
    "factor_key": ..., "expression_hash": ...,
    "benchmark_keys": [...],
    "window": {"start": ..., "end": ..., "rebalance": ...},
    "neutralize": [...],
    "universe": row.universe,
    "pairs": [...],
    "raw": {...},
    "residual": {...},
    "verdict": "...", "verdict_reason": "...",
    "elapsed_seconds": ...,
    "detail_ref": {"correlation_id": row.id},
    "status": row.status, "error": row.error,
  }
}
```

`handle()` 里在提交前就校验 `benchmark_keys` 数量与 `rebalance`、
`neutralize`（照 `factor_evaluate.py:102-107` 的注释所说：非法值应立即报错，
不要等 worker 起来才失败）。

### 步骤 6：`app/a2a/skills/factor_correlation_get.py`

照 `app/a2a/skills/factor_evaluation_get.py` 写（该文件只 29 行）。
domain 查询函数放 `app/factors/correlation.py` 里：

```python
class CorrelationNotFoundError(ValueError):
    """不存在或不属于当前用户(统一按不存在处理,防探测)。"""


def get_correlation(db, *, user_id: str, correlation_id: int) -> dict[str, Any]:
```

非本人的行按不存在处理，与 `listing.py:120` 同口径。

### 步骤 7：注册与 Card

**`app/a2a/skills/__init__.py`**：import 两个模块，`SKILLS` 加
`"factor.correlation"` 与 `"factor.correlation_get"`。

**`app/a2a/tasks.py`**：`HIGH_COST_SKILLS` 加 `"factor.correlation"`
（**不加** `correlation_get`，它是只读）；`SKILL_TO_QUANT_TASK_TYPE` 加
`"factor.correlation": "factor_correlation"`。

**`app/a2a/card.py`**：在 `factor.evaluation_get` 之后插入两条：

```python
            {
                "id": "factor.correlation",
                "name": "Factor correlation and orthogonality",
                "description": "Test whether a candidate factor adds information over a benchmark factor set: per-rebalance cross-sectional Pearson/Spearman correlation with stability, correlation between IC series, and residual IC with Newey-West t-stats after cross-sectional regression on the benchmarks. Residual IC insignificant means no increment over existing factors. Long-running, high-cost. Requires confirmed=true.",
                "tags": ["factor", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.correlation_get",
                "name": "Get factor correlation result",
                "description": "Fetch one own factor correlation result by correlation_id, including per-benchmark correlation pairs and the residual IC verdict.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
```

Card 与 `SKILL_IDS` 一致性由 `tests/test_a2a.py:184-186` 断言。
做完 A + B 后 skill 总数为 **24**（21 + A 的 1 + B 的 2）。

### 步骤 8：`server/skills/quant-research/SKILL.md`（trace 根）

在编号约束末尾（当前第 20 条之后）新增两条：

```
21. **第二个因子起必须查增量**：同一会话跑出第二个因子后，不得只比较各自的 IC —— 必须调用 `quant_factor_correlation`，用待检因子对已有因子集做正交化。`verdict=no_increment` 时明确说「相对已有因子无增量」，即使它的裸 IC 好看也不得推荐；`verdict=inconclusive` 时说样本不足，不得当成有增量。
22. **区分因子值相关与 IC 相关**：`pairs[].pearson_mean` 低但 `pairs[].ic_correlation` 高，说明两个因子写法不同但捕捉同一收益来源，必须在结论里指出这一点，不得因为因子值相关性低就断言「是新因子」。
```

同时把第 12 条（因子提炼链路）末尾补上相关性环节 —— 注意任务 A 已经改过
第 12 条，**在 A 的版本上继续追加**，不要覆盖回旧文案：

```
…（A 层已加的回填说明）… 跑出第二个及以后的因子时，链路末端追加 `quant_factor_correlation` 判增量。
```

### 步骤 9：Trace 侧工具注册（`crates/code-tools/src/quant_tools.rs`）

在 `quant_get_factor_evaluation` 之后插入两个工具，照该文件既有
`mk(ToolSpec{...})` 形状：

```rust
        mk(
            ToolSpec {
                tool_name: "quant_factor_correlation",
                skill: "factor.correlation",
                description: "检验待检因子相对已有因子集是否有增量：逐调仓日截面相关性、IC 序列相关性、以及对对照因子回归后的残差 IC（含 Newey-West t 值）。残差 IC 不显著即无增量，此时裸 IC 好看也不得推荐。跑出第二个因子后必须调用。模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": ["object", "null"] },
                        "factor_key": { "type": ["string", "null"] },
                        "benchmark_keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "对照因子 key 列表，最多 20 个。省略则取全部启用的系统因子（超过 20 个会报错要求显式指定）。"
                        },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "rebalance": { "type": "string", "enum": ["weekly", "monthly"], "description": "默认 weekly" },
                        "neutralize": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["industry", "market_cap"] },
                            "description": "与 quant_evaluate_factor 同口径；待检与对照因子用同一口径中性化，否则相关性无意义。"
                        },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["start", "end"]
                }),
                high_cost: true,
                artifact_name: "factor_correlation",
            },
            |input| {
                let key = input["factor_key"].as_str().map(|s| s.to_string())
                    .unwrap_or_else(|| "表达式".to_string());
                let n = input["benchmark_keys"].as_array().map(|a| a.len()).unwrap_or(0);
                format!("检验因子 {} 相对 {} 个对照因子的增量", key, n)
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_get_factor_correlation",
                skill: "factor.correlation_get",
                description: "按 correlation_id 取单次因子相关性检验详情。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "correlation_id": { "type": ["integer", "string"] }
                    },
                    "required": ["correlation_id"]
                }),
                high_cost: false,
                artifact_name: "factor_correlation",
            },
            |input| {
                let id = input["correlation_id"].as_i64().map(|v| v.to_string())
                    .or_else(|| input["correlation_id"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "?".to_string());
                format!("查看因子相关性检验 {}", id)
            },
        ),
```

同时：
- 文件头工具数注释改为 24（A 层已改成 22，这里 +2）
- `test_high_cost_tools_delegate_confirmation_to_runtime_gate` 的数组加
  `"quant_factor_correlation"`
- `test_factor_evaluation_read_tools_are_registered_and_low_cost` 的
  只读数组加 `"quant_get_factor_correlation"`

### 步骤 10：测试 `tests/test_factor_correlation.py`

**统计逻辑必须用构造数据断言已知答案**，不能只断言「跑通不报错」。
参照 `tests/test_factor_evaluation.py` 的 seed 风格。必须覆盖：

1. `test_identical_factor_has_no_increment`
   待检因子 = 对照因子（同一 key 的值 × 2 + 常数，即完全线性相关）→
   残差应恒为 0（或全期被 skip），`verdict == "no_increment"`
2. `test_orthogonal_factor_keeps_increment`
   构造两个正交的因子（如一个用 close 动量、一个用独立随机数且与收益相关）→
   `pairs[0].pearson_mean` 接近 0，残差 IC 与裸 IC 接近
3. `test_high_correlation_pair_is_flagged`
   5 日动量 vs 10 日动量（真实高相关）→ `high_corr_ratio` 明显大于 0
4. `test_orthogonalize_returns_none_on_singular_design`
   直接单测 `_orthogonalize`：传两列完全共线的对照 → 返回 None
   （**不是**返回原值 —— 这是 §3.3 的核心约定）
5. `test_benchmark_limit_enforced`
   传 21 个 key → 报错含「最多」
6. `test_missing_benchmark_key_errors`
   传不存在的 key → 报错列出缺失的 key
7. `test_self_excluded_from_benchmarks`
   `factor_key` 同时出现在 `benchmark_keys` → 被剔除，结果 `note` 说明
8. `test_verdict_inconclusive_on_short_sample`
   区间过短导致 `n_periods < 6` → `verdict == "inconclusive"`

A2A 层测试加在 `tests/test_a2a.py` 或新建 `tests/test_a2a_correlation.py`：

9. `test_correlation_requires_confirmed`
   不带 `confirmed=true` → failed，错误含「高成本」
10. `test_correlation_get_rejects_other_users_row`
    用户 A 的结果，用户 B 取 → failed，错误含「不存在」

### 步骤 11：文档 `quant/docs/a2a-design.md`

新增 §8.13b（放在 §8.13a 之后），内容：payload 表、规则表
（授权 `can_client`、高成本闸门、对照上限 20、区间 10 年、互斥槽共用）、
artifact `factor_correlation` 形状、**判读口径**（残差 IC 不显著 = 无增量；
因子值低相关但 IC 高相关 = 同一收益来源）。

同时更新：
- 第 245 行免确认清单：加 `factor.correlation_get`（**不加**
  `factor.correlation`，它是高成本）
- 第 262 行附近的长任务映射表：加一行 `factor.correlation`
- 第 1437 行附近文件树：加 `factor_correlation.py` /
  `factor_correlation_get.py`
- 第 1470 行授权总表：`factor.correlation` 归 client 行

---

## 5. 验收标准

```bash
cd /Users/admin/projects/hank/trace/quant

# 1. 迁移链（0028 应是新 head）
uv run alembic -x db_url=sqlite+pysqlite:///$(mktemp -d)/t.db upgrade head

# 2. 本任务新测试
uv run pytest tests/test_factor_correlation.py -v

# 3. 受影响的既有测试
uv run pytest tests/test_a2a.py tests/test_factor_evaluation.py \
    tests/test_schema.py tests/test_factor_api.py -q

# 4. 全量回归
uv run pytest -q
```

```bash
cd /Users/admin/projects/hank/trace
cargo test -p code-tools quant
cargo build --workspace
```

**期望结果**：

- 步骤 1 输出 `Running upgrade 0027_factor_def_owner -> 0028_factor_correlation`
- 步骤 2 全绿，10 个用例
- 步骤 3 全绿
- 步骤 4：**A 层完成后的基线是 `828 + A 新增用例数`**（A 层交付时会给出实测值）。
  本任务完成后总数应为该基线 + 本任务新增用例数，失败数 0
- 步骤 5：`cargo test` 全绿

**人工核对项**（写进回报）：

- Agent Card skills 数量 = `len(SKILL_IDS)` = 24
- `quant_tools.rs` 的 `tool_name: "quant_` 计数（排除 tests 模块内的）= 24
- 用真实数据跑一次 5 日 vs 10 日动量，确认 `verdict == "no_increment"`；
  这是需求 §8 B 层验收的原话，自动化测试用的是构造数据，需人工验一次真实数据

---

## 6. 约定

- 中文注释，注释解释**为什么**而不是复述代码
- commit message 用中文，形如
  `feat(quant): 新增因子相关性与正交化检验 skill，残差 IC 判增量`
- 不新增依赖（numpy / pandas 已有）
- 保留工作区原有改动，只提交与本任务相关的文件
- 遇到与本文档冲突的既有实现，以**既有实现为准**并在回报里说明冲突点
- **不要**为了让相关性代码更好看去重构 `app/factors/evaluation.py`
