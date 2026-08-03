# 任务 C：横截面因子表达力（`cs_*` 算子与截面求值路径）

| 属性 | 内容 |
|------|------|
| 任务 ID | `TASK-FACTOR-C-CROSS-SECTION` |
| 上游需求 | `quant/docs/factor-research-requirements.md` §4.1（R1.1–R1.6）、§5 C 层 |
| 执行顺序 | 第 3 份（A → B → **C**）。改动面最大，必须最后做 |
| 工作目录 | `/Users/admin/projects/hank/trace/quant` |
| 语言约定 | 中文注释、中文 commit message（遵循 `trace/CLAUDE.md`） |

> **执行前置**：本文档假设 A、B 已合并。本任务**不需要新迁移**（不改 schema）。
> 若 A/B 未合并，`app/a2a/card.py` 与 `quant_tools.rs` 的工具数注释以当时实际值为准。

> **风险提示**：本任务碰的是组合策略在用的生产代码路径
> （`app/strategy/operators.py`、`app/strategy/spec.py`）。
> 这两个文件有**冻结基线测试** `tests/test_operator_baseline.py`
> （3800 行，逐字节断言算子形状快照、预设策略哈希、30 个算子的规范化 JSON
> 与求值结果）。§3.1 详细说明了哪些断言会红、为什么、怎么改。
> **不要**为了让基线变绿而改动既有算子的任何字段或求值逻辑。

---

## 1. 背景与目标

### 现状（已核对代码）

因子求值是**逐股时序**的：`app/factors/engine.py:30` 的
`evaluate_factor(expr, df)` 接收**单只股票**的日线 DataFrame，
`bars_fields()` 把每列包成 `pd.Series`。

而 `rank` / `top_n` 两个算子明确要求 DataFrame 输入
（`app/strategy/operators.py:209` 与 `:217`，否则抛「只能用于组合横截面」）。
因子链路因此永远拿不到横截面 —— 行业内排名、截面 z-score、对市值回归取残差
这类因子研究的基本操作，agent 一个都写不出来。

**关键发现（需求 §4.1 已指出，本次已核对确认）**：组合策略侧**已经有**完整的
横截面求值路径。`app/strategy/compiler.py:782` 的 `_portfolio_fields(index, pool_dfs)`
把 `{code: 日线帧}` 转成 `{字段: date×code DataFrame}`，
`evaluate_expression` 在该形状下正常工作，`rank` / `top_n` 正是为它设计的。

**所以本任务不是新建横截面引擎，而是让因子求值复用这条既有路径。**

### 目标

1. agent 能用 DSL 写出「行业内 20 日动量排名」并跑完 `factor.evaluate`，
   `ic_decay` 与分层结果非空
2. 新增三个截面算子 `cs_rank` / `cs_zscore` / `cs_demean`，均支持
   可选 `group_by`（本期只支持 `industry`）
3. 时序表达式里误用 `cs_*` **必须校验失败**，不能静默算错
4. 截面因子在 `factor.preview`（≤5 标的）上返回明确错误，而不是算出垃圾值

### 边界

- `group_by` 只支持 `industry`。`industry` **不进** `SUPPORTED_FIELDS`
  （它不是价量数据，是分组维度），由求值上下文注入
- 行业数据非 PIT（`quant_stock.industry` 是**当前**行业，akshare 覆盖写入），
  按它做历史分组是轻微前视。**这是中性化上线时就存在的既有问题，不是本任务引入**
  （需求 §6 R3）。本期沿用并在结果里声明
- 截面因子**不走** `FactorDaily` 回填：截面值依赖当期全池，逐股回填算不出来。
  评估时直接按调仓日截面现算（R1.5）

---

## 2. 涉及文件清单

**修改：**

| 文件 | 改什么 |
|------|--------|
| `app/strategy/operators.py` | 新增 3 个 `cs_*` OperatorSpec 与求值函数；新增 `CROSS_SECTION_OPS` 常量 |
| `app/strategy/spec.py` | `Expression` 加 `group_by` 槽位；`validate_expression` 加 `mode` 参数区分时序/横截面 |
| `app/factors/engine.py` | 新增 `evaluate_factor_cross_section()` |
| `app/factors/evaluation.py` | 截面因子走新路径求值（R1.5） |
| `app/a2a/skills/factor_preview.py` | 截面因子明确拒绝（R1.6） |
| `app/a2a/skills/factor_validate.py` | 透传横截面判定结果 |
| `app/api/factors.py` | REST preview 同步拒绝；REST validate 透传 |
| `app/factors/backfill.py` | 截面因子跳过回填并明确报错 |
| `tests/test_operator_baseline.py` | 更新冻结快照（见 §3.1，**只允许新增条目**） |
| `tests/test_operator_registry.py` | 算子数 30 → 33 |
| `crates/code-tools/src/quant_tools.rs`（trace 根） | `quant_validate_factor` / `quant_preview_factor` / `quant_evaluate_factor` 描述补截面说明 |
| `server/skills/quant-research/SKILL.md`（trace 根） | 新增截面因子的使用约束 |
| `quant/docs/a2a-design.md` | catalog 的算子契约补 `cs_*`；§8.11/§8.13 补截面说明 |
| `app/catalog.py` | 若算子目录由此暴露给 agent，补 `cs_*` 条目（先确认该文件是否列举算子） |

**新增：**

| 文件 | 内容 |
|------|------|
| `tests/test_cross_section_factors.py` | 截面算子求值、校验、端到端评估的测试 |

**不许碰：**

- **既有 30 个算子的任何字段、求值逻辑、min_window 函数**。
  只允许在 `_OPERATORS` 列表**末尾追加**新条目
- `app/strategy/compiler.py`。`_portfolio_fields` 是生产代码，
  本任务**只新增调用方**，不改它的签名与语义（需求 §6 R1）
- `app/strategy/components.py` 的 `evaluate_expression` 分发逻辑
- `tests/test_operator_baseline.py` 里既有 30 个算子的
  `OP_FIELDS_SNAPSHOT` / `CORPUS` / `CORPUS_CANONICAL` / `CORPUS_EVAL`
  / `PRESET_HASHES` 条目 —— 只允许**新增** key，不允许改既有 key 的值
- A / B 层交付的代码
- 工作区里与本任务无关的既有改动一律保留

---

## 3. 关键设计决定（照做，不要自行改口径）

### 3.1 冻结基线测试怎么处理（**执行前必读**）

`tests/test_operator_baseline.py` 的文件头写着「勿手改」。本任务必须改它，
因为新增算子会让下面两个断言必然失败：

| 断言 | 位置 | 为什么会红 | 怎么改 |
|------|------|-----------|--------|
| `test_op_fields_snapshot` | `:3720` | `OP_FIELDS_SNAPSHOT` 是全量算子形状字典（当前 30 项），`assert current == SNAPSHOT` 是精确相等 | 在字典里**新增** 3 个 `cs_*` 条目。**既有 30 项一个字符都不能动** |
| `test_registry_has_31_ops` | `test_operator_registry.py:17` | 函数名写 31 但实际断言 `len(OPERATORS) == len(_OP_FIELDS)`，两边同源所以不会因新增而红 | 只需把函数名与 docstring 里的数字改成 33，断言逻辑不动 |

**不会红、也不该动的**：

- `PRESET_HASHES`（`:57`）—— 6 个预设策略的哈希。新增算子不改既有策略的
  规格，哈希必须保持不变。**如果这个测试红了，说明你改了不该改的东西，
  回退重做，不要更新哈希值**
- `CORPUS` / `CORPUS_CANONICAL` / `CORPUS_EVAL`（`:64` / `:160` / `:193`）——
  30 个算子的规范化 JSON 与求值结果。给 `cs_*` **新增** 3 个 corpus 条目
  （连带三处字典各加 3 个 key），既有条目不动
- `PORTFOLIO_OPS`（`:3679`）—— 当前 `{"rank", "top_n"}`。
  `cs_*` 也只能在横截面求值，**把 3 个新算子加进这个集合**，
  否则 `test_corpus_evaluation` 会用 Series 输入喂它们并失败

新增 corpus 条目时注意 `test_corpus_evaluation`（`:3773`）对 `PORTFOLIO_OPS`
里的算子有一段特殊的期望值构造（`row` / `initial` 硬编码在
`:3780-3787`，只区分 `rank` 与其它）。给 `cs_*` 加期望值时**不要**改那段
既有分支，在它之后加 `elif name.startswith("cs_")` 分支，
或把 `cs_*` 的期望值直接写进 `CORPUS_EVAL` 走通用路径 —— 后者更干净，优先选它。

### 3.2 `group_by` 是新槽位，不复用 `name`

`Expression`（`app/strategy/spec.py:80`）的槽位是「字段全列出，
再由 op 校验精确允许集合」。给它加：

```python
    group_by: str | None = None
```

`cs_*` 的 `fields` 声明为 `frozenset({"op", "input", "group_by"})` ——
注意 `validate_operator_shape`（`:106`）要求 `frozenset(value) == allowed`
**精确相等**，所以 `group_by` 是**必填键**，允许值为 `null`。
这与 `rolling_*` 的 `shift` 同款处理（它也是必填但可为 0）。

在 `validate_values`（`:128`）里加：

```python
        if self.op in CROSS_SECTION_OPS and self.group_by is not None:
            if self.group_by not in SUPPORTED_GROUP_BY:
                raise ValueError(
                    f"group_by 只支持 {sorted(SUPPORTED_GROUP_BY)}，收到 {self.group_by!r}"
                )
```

`SUPPORTED_GROUP_BY = frozenset({"industry"})` 定义在 `operators.py`
（与 `CROSS_SECTION_OPS` 放一起），`spec.py` import 它。

### 3.3 `industry` 不进 `SUPPORTED_FIELDS`，走求值上下文注入

`group_by="industry"` 时，求值需要每只股票的行业。**不要**把 `industry`
加进 `SUPPORTED_FIELDS`（R1.3）—— 它不是价量数据，加进去会让
`{"op":"field","name":"industry"}` 变成合法表达式，而行业是字符串，
整套数值算子对它无意义。

做法：`cs_*` 的求值函数从 `fields` 映射里取一个**保留键**
`__industry__`（双下划线包围，不可能与 snake_case 字段名冲突，
因为 `field.name` 被正则 `[a-z][a-z0-9_]*` 限制，写不出双下划线开头）：

```python
# 分组维度的保留注入键。不进 SUPPORTED_FIELDS:行业是字符串标签,不是价量
# 数据,暴露成 field 会让整套数值算子对它可用而语义无意义。field.name 的
# 正则不允许下划线开头,所以这个键不可能与用户表达式里的字段撞名。
INDUSTRY_FIELD_KEY = "__industry__"
```

求值时该键的值是 `pd.Series`（index = code，value = 行业名），
由调用方（`engine.py` / `evaluation.py`）注入。`group_by` 非空但
`fields` 里没有这个键 → 抛 `ValueError("截面分组需要行业数据，当前求值上下文未提供")`。

### 3.4 三个算子的语义（照此实现，不要自行调整）

全部要求 DataFrame 输入（`date × code`），沿 `axis=1` 计算，返回同形 DataFrame。
输入是 Series 或标量 → 抛
`ValueError(f"{op} 只能用于横截面，时序表达式请改用 rolling_rank / zscore")`
（错误文案要指路到时序等价算子，agent 才知道怎么改）。

| 算子 | 语义 | 实现要点 |
|------|------|---------|
| `cs_rank` | 截面分位，输出 (0, 1] | `df.rank(axis=1, pct=True, na_option="keep")`。**必须 `na_option="keep"`**：NaN 参与排名会把停牌股算进分位 |
| `cs_zscore` | 截面标准化 | `(df - mean) / std`，`ddof=0`（与 `ROLLING_STD_DDOF` 一致，`operators.py:21` 已定义该常量，复用它）。std == 0 的行输出 NaN，不输出 inf |
| `cs_demean` | 截面去均值 | `df - mean` |

`group_by` 非空时，上述计算在**组内**进行：按 `INDUSTRY_FIELD_KEY` 的
Series 把列分组，逐组算完再拼回原列顺序。

组内样本数的门槛：**组内有效样本 < 3 时该组输出 NaN**，不要用组均值兜底。
理由与 `_neutralize_cross_section`（`evaluation.py:310`）的
`n < 5 return values` 同源 —— 样本不足时算出的分位/z 值是噪声，
输出 NaN 让下游的覆盖率统计如实反映样本损失，比给个假值好。

`min_window`：三个算子都用既有的 `_max_children`（`operators.py:222`），
截面操作不消耗时间窗。

### 3.5 表达式模式判定：新增 `expression_mode()`，`validate_expression` 加 `mode` 参数

在 `spec.py` 新增：

```python
def expression_mode(expr: Expression) -> str:
    """判定表达式是「时序」还是「横截面」。

    含任何 CROSS_SECTION_OPS 或 rank / top_n 即为横截面 —— 这些算子沿 code
    轴计算,必须拿到 date×code 帧。时序上下文里出现它们会静默算错(Series
    上 rank(axis=1) 无意义),所以必须在校验期拦下而不是等求值抛异常。
    """
    for node in _walk_expression(expr):
        if node.op in CROSS_SECTION_OPS or node.op in {"rank", "top_n"}:
            return "cross_section"
    return "time_series"
```

`validate_expression` 加参数：

```python
def validate_expression(
    value: Any,
    *,
    require_type: str = "number",
    available_fields: set[str] | frozenset[str] | None = None,
    mode: str | None = None,   # None = 不限制;"time_series" / "cross_section"
) -> ExpressionValidationResult:
```

`mode` 非 None 且与 `expression_mode(expr)` 不符 → 加一条 issue：

```python
            add(
                CapabilityStatus.MISSING_ENGINE,
                "$",
                "expression_mode_mismatch",
                f"该上下文只接受{'时序' if mode == 'time_series' else '横截面'}"
                f"表达式，但表达式是{'横截面' if actual == 'cross_section' else '时序'}的",
            )
```

`ExpressionValidationResult`（`spec.py:531`）加一个字段
`mode: str | None`，成功时填 `expression_mode(expr)` —— agent 需要知道
自己写的是哪种，才能选对后续 skill。失败时填 None。

**`mode` 默认 None（不限制）**：这样所有既有调用方
（`factor_save_draft.py:51`、`api/factors.py:252`、
`evaluation.py:635` 等）行为不变，只有显式传 `mode` 的新调用方才受约束。

### 3.6 各调用方传什么 mode

| 调用方 | mode | 理由 |
|--------|------|------|
| `factor.validate` / REST validate | `None` | 校验是纯粹的「这个表达式合法吗」，两种都该放过，把 `mode` 回给 agent |
| `factor.preview` / REST preview | `"time_series"` | ≤5 标的算不出有意义的截面（R1.6） |
| `factor.save_draft` | `None` | 草稿两种都能存 |
| `factor_backfill` | `"time_series"` | 逐股回填算不出截面值 |
| `factor.evaluate`（`expression` 路径） | `None` | 两种都支持，按判定分流求值路径（R1.5） |
| `factor.evaluate`（`factor_key` 路径） | 见 §3.7 | |

### 3.7 `factor.evaluate` 的两条求值路径

`evaluate_factor_efficacy`（`evaluation.py:589`）现在的分流是
`factor_key` 读库 / `expression` 现算（`:688-696`）。改成三分支：

```python
        if factor_key is not None:
            # 截面因子不落 FactorDaily(截面值依赖当期全池,逐股回填算不出来),
            # 所以按 key 评估时先看它的表达式是哪种模式,截面的走现算路径。
            if def_mode == "cross_section":
                factor_values = _load_cross_section_factor_values(...)
            else:
                factor_values = _load_saved_factor_values(...)
        elif expression_mode(parse_expression(expression)) == "cross_section":
            factor_values = _load_cross_section_factor_values(...)
        else:
            factor_values = _load_expression_factor_values(...)
```

`factor_key` 路径需要先取出该因子的 expression 来判定模式 ——
从 `load_all_defs`（`app/factors/defs.py:106`）拿快照，
不要新写一次 SQL。

新增 `_load_cross_section_factor_values(db, expression, codes, dates, ...)`：

- 只在**调仓日**求值（不是每个交易日）—— 截面因子只在调仓点用得到，
  逐日算全池是 N 倍浪费
- 每个调仓日：取该日全池的字段帧 → 调
  `evaluate_factor_cross_section` → 取该日那一行 → 落进
  `{(code, date): value}`，形状与另两个 loader 完全一致，
  下游逐期循环（`:712` 起）**一行都不用改**
- 注入 `INDUSTRY_FIELD_KEY`（从 `_stocks_for_codes` 已有的 stocks dict 取）
- 取消检查点：每个调仓日调 `_check_cancel`

### 3.8 内存：按调仓日惰性求值，不建全区间大帧

需求 §6 R4 点出全市场 date×code 帧的内存风险
（5000 只 × 2400 日 × N 字段）。§3.7 的「只在调仓日求值」已经是应对：
每次只构造**该调仓日所需回看窗口**的帧（`min_bars` 决定窗长），
用完即弃，峰值内存是 `5000 × min_bars × N`，与全区间无关。

`evaluate_factor_cross_section(expr, pool_dfs)` 的签名接收
`{code: 日线帧}`（与 `compile_portfolio` 的 `pool_dfs` 同形），
内部调 `_portfolio_fields`。**必须复用它，不要复制第二套帧构造实现**（R1.1）。
由于它是 `compiler.py` 的模块私有函数，在 `compiler.py` 的 `__all__` 里
补上 `_portfolio_fields`（只改 `__all__`，不改函数体），
并在那里加一行注释说明为什么导出私有名。

---

## 4. 实现步骤

### 步骤 1：`app/strategy/operators.py` 加三个算子

在文件顶部常量区（`ROLLING_STD_DDOF` 附近）加：

```python
# 横截面算子:沿 code 轴计算,只能作用于 date×code 帧。与 rank / top_n 同类,
# 但后两者是组合选股用的(输出排名/布尔),cs_* 是因子加工用的(输出标准化值)。
CROSS_SECTION_OPS = frozenset({"cs_rank", "cs_zscore", "cs_demean"})
# 截面分组维度。只支持行业:市值是连续变量,分组要先分箱,箱界是另一个
# 自由度(需求明确不做无界搜索),本期不引入。
SUPPORTED_GROUP_BY = frozenset({"industry"})
INDUSTRY_FIELD_KEY = "__industry__"
# 组内有效样本少于此数则该组输出 NaN:样本不足时的分位/z 值是噪声,
# 与 _neutralize_cross_section 的 n<5 降级同源。
MIN_GROUP_SIZE = 3
```

求值函数（放在 `_eval_top_n` 之后）：

```python
def _cross_section_frame(expr: "Expression", fields, recurse) -> pd.DataFrame:
    """取出 cs_* 的输入并确保是横截面帧。"""
    assert expr.input is not None
    value = recurse(expr.input, fields)
    if not isinstance(value, pd.DataFrame):
        raise ValueError(
            f"{expr.op} 只能用于横截面(date×code 帧);时序表达式请改用 "
            "rolling_rank(窗口内分位)或 zscore(窗口内标准化)"
        )
    return value


def _apply_cross_section(
    frame: pd.DataFrame, kind: str, groups: pd.Series | None,
) -> pd.DataFrame:
    """逐行(逐日)做截面变换;groups 非空时在组内做。"""
    # 实现要点见任务文档 §3.4
```

三个 `OperatorSpec` **追加到 `_OPERATORS` 列表末尾**（`:343` 的 `]` 之前）：

```python
    *[
        OperatorSpec(
            op=op,
            fields=frozenset({"op", "input", "group_by"}),
            arg_types={"input": "number"},
            result_type="number",
            evaluate=_make_cross_section_eval(op),
            min_window=_max_children,
        )
        for op in ("cs_rank", "cs_zscore", "cs_demean")
    ],
```

（`_make_cross_section_eval(op)` 返回闭包，或者写 3 个具名函数 ——
照该文件既有风格，`ma`/`rsi`/`momentum` 那组用的是列表推导 + 共享
evaluate（`:317-323`），可以照抄那个模式。）

`__all__` 补 `CROSS_SECTION_OPS`、`SUPPORTED_GROUP_BY`、
`INDUSTRY_FIELD_KEY`、`MIN_GROUP_SIZE`。

### 步骤 2：`app/strategy/spec.py`

1. `Expression` 加 `group_by: str | None = None` 槽位（放在 `n` 之后）
2. `validate_values` 加 §3.2 的 `group_by` 取值校验
3. 新增 §3.5 的 `expression_mode()`
4. `ExpressionValidationResult` 加 `mode: str | None`
5. `validate_expression` 加 `mode` 参数与不匹配的 issue
6. 成功分支（`:809-820`）里填 `mode=expression_mode(expr)`；
   失败分支（`:825`）填 `mode=None`
7. `__all__` 补 `expression_mode`

**注意**：`_scan_raw_capabilities`（`:836`）会扫原始 JSON 的所有 key。
`group_by` 不在 `forbidden_keys` 里，安全。但确认一下
`"industry"` 作为**值**不会被 `denied_patterns` 的正则命中 —— 逐条看过，
不会（那些是 eval/import/SQL/URL 模式）。

### 步骤 3：`app/factors/engine.py` 新增截面求值

```python
def evaluate_factor_cross_section(
    expr: Expression | dict[str, Any] | str,
    pool_dfs: dict[str, pd.DataFrame],
    *,
    industries: dict[str, str] | None = None,
) -> pd.DataFrame:
    """在横截面上求值因子,返回 date×code 帧。

    复用组合策略侧既有的帧构造路径(compiler._portfolio_fields),不另建一套
    横截面引擎 —— 两套实现会在字段对齐与 reindex 语义上悄悄分叉。
    """
    from ..strategy.compiler import _portfolio_fields

    parsed = parse_expression(expr)
    if not pool_dfs:
        raise ValueError("pool_dfs 不能为空")
    # 统一日期轴:取全池日期并集并排序,与 compile_portfolio 的 index 同口径
    index = ...
    fields = _portfolio_fields(index, pool_dfs)
    if industries is not None:
        fields[INDUSTRY_FIELD_KEY] = pd.Series(industries)
    result = evaluate_expression(parsed, fields)
    if not isinstance(result, pd.DataFrame):
        raise ValueError("横截面求值必须返回 date×code 帧;该表达式可能是时序的")
    return result
```

`__all__` 补新函数。

### 步骤 4：`app/factors/evaluation.py` 接入截面路径

按 §3.7 改分流，新增 `_load_cross_section_factor_values`。
`__all__` 若 B 层已补过私有名，保持；本任务新增的 loader 是私有的，
不需要导出（只在本模块用）。

结果 JSON 里加一段声明（放在 `neutralization` 旁边）：

```python
            "cross_section": {
                "is_cross_section": is_cross_section,
                "group_by": group_by_used,
                "note": (
                    "截面因子按调仓日全池现算,不经因子日值表;"
                    "行业分组用当前行业(非 PIT),历史分组存在轻微前视"
                ) if is_cross_section else None,
            },
```

**行业非 PIT 的声明必须落进结果**（需求 §6 R3 要求「本期沿用并在结论中声明」）。

### 步骤 5：preview / backfill / validate 各处传 mode

- `app/a2a/skills/factor_preview.py`：`validate_expression(..., mode="time_series")`。
  校验失败时错误文案要明确指路：
  「截面因子无法在 ≤5 标的上抽查（截面样本不足），请改用 factor.evaluate」（R1.6）
- `app/api/factors.py` 的 `preview_factor`（`:302`）：同上
- `app/factors/backfill.py`：`_evaluate_day` 之前判定，截面因子
  抛 `ValueError("截面因子不支持回填:截面值依赖当期全池,请直接用 factor.evaluate")`
- `app/a2a/skills/factor_validate.py` 与 REST validate：
  `mode=None`，把 `result.mode` 透传给调用方

### 步骤 6：`app/catalog.py` 的算子目录必须区分因子算子与策略算子

**已核对**：`app/catalog.py:657` 的 `_strategy_authoring_catalog()` 的
`operators` 列表是从 `OPERATORS` 字典**全量派生**的
（`for op, spec in sorted(OPERATORS.items())`）。所以新增 `cs_*` 后，
它们会**自动出现在 `strategy_authoring` 契约里** —— 而那份契约是
「给 Agent 的 StrategySpec 编写契约」（该函数 docstring 原话）。

这是个真问题：agent 会以为可以在 StrategySpec 里用 `cs_rank`。
本任务**不让组合策略用 `cs_*`**（那会牵动策略哈希与回测语义，超出范围），
所以必须在目录层把它们标出来。做法 —— 给每个算子条目加一个 `context` 键：

```python
        "operators": [
            {
                "op": op,
                "required_keys": sorted(spec.fields),
                "argument_types": deepcopy(spec.arg_types),
                "result_type": spec.result_type,
                # 标明适用上下文:cs_* 是因子加工算子,只能用在 factor.evaluate
                # 的表达式里。目录是全量派生的,不标注 agent 会拿它写 StrategySpec。
                "context": (
                    "factor_only" if op in CROSS_SECTION_OPS else "strategy_and_factor"
                ),
                "version": spec.version,
            }
            for op, spec in sorted(OPERATORS.items())
        ],
```

并在返回的 dict 里加一条说明：

```python
        "operator_context_note": (
            "context=factor_only 的算子(cs_rank / cs_zscore / cs_demean)只能用于"
            "因子表达式,不能出现在 StrategySpec 里"
        ),
```

`tests/test_a2a.py:224` 的
`test_catalog_get_strategy_authoring_contract_is_directly_validatable`
断言了 `operators["gt"]["required_keys"]` 等具体键，**新增 `context` 键
不会让它红**（它只按 key 取值，不做精确 dict 相等）。给该测试**追加**两条断言：

```python
    assert operators["cs_rank"]["context"] == "factor_only"
    assert operators["gt"]["context"] == "strategy_and_factor"
```

**另外**：`strategy.validate` 必须拒绝含 `cs_*` 的 StrategySpec。
检查 `spec.py:881` 附近的能力扫描（`if child not in SUPPORTED_FIELDS`
那段）与 `StrategySpec` 的表达式校验路径，
确认 `cs_*` 出现在策略规格里时会失败。若默认放过，
在 StrategySpec 的表达式校验处传 `mode="time_series"`
（策略侧 `rank`/`top_n` 是合法的横截面算子，所以**不能**简单传 time_series）——
改为显式检查 `CROSS_SECTION_OPS`：

```python
# 组合策略的横截面语义由 rank / top_n 承担;cs_* 是因子加工算子,进入策略
# 规格会改变既有回测语义,本期明确不支持。
```

补一条测试 `test_strategy_spec_rejects_cs_ops`。

### 步骤 7：测试 `tests/test_cross_section_factors.py`

必须覆盖（构造数据、断言已知答案，不能只断言不报错）：

1. `test_cs_rank_outputs_pct_within_zero_one`
   3 只股票、明确大小关系 → 分位为 1/3、2/3、1
2. `test_cs_rank_keeps_nan_out_of_ranking`
   一只停牌（NaN）→ 它输出 NaN，其余两只按 2 个样本排名（0.5、1.0）
3. `test_cs_zscore_zero_variance_row_is_nan`
   一行全相等 → 该行输出 NaN 而非 inf
4. `test_cs_demean_sums_to_zero`
   去均值后每行和 ≈ 0
5. `test_group_by_industry_computes_within_group`
   两个行业各 3 只 → 组内分位独立（每组都出现 1/3、2/3、1）
6. `test_small_group_outputs_nan`
   某行业只有 2 只（< `MIN_GROUP_SIZE`）→ 该组输出 NaN
7. `test_cs_op_on_series_raises_with_hint`
   Series 输入 → 报错含「只能用于横截面」且含「rolling_rank」
8. `test_group_by_without_industry_context_raises`
   `group_by="industry"` 但未注入 → 报错含「未提供」
9. `test_time_series_mode_rejects_cs_op`
   `validate_expression(cs_rank表达式, mode="time_series")` → `valid is False`，
   issue code == `expression_mode_mismatch`
10. `test_cross_section_mode_rejects_pure_time_series`
    反向：纯时序表达式 + `mode="cross_section"` → 失败
11. `test_expression_mode_detects_rank_and_top_n`
    `rank` / `top_n` 也判为 cross_section
12. `test_invalid_group_by_rejected`
    `group_by="market_cap"` → 解析期报错
13. `test_industry_not_in_supported_fields`
    `{"op":"field","name":"industry"}` 在 `available_fields` 校验下失败
    （R1.3 的守卫）
14. `test_preview_rejects_cross_section_factor`
    A2A preview skill 层：截面因子 → failed，文案含「抽查」
15. `test_backfill_rejects_cross_section_factor`
    → 报错含「不支持回填」
16. `test_evaluate_industry_momentum_rank_end_to_end`
    **G1 的验收**：seed 多行业多标的日线，
    用「行业内 20 日动量 `cs_rank`」跑 `evaluate_factor_efficacy`，
    断言 `ic_decay` 非空、`layers` 非空、
    `result["cross_section"]["is_cross_section"] is True`

### 步骤 8：更新冻结基线（照 §3.1，逐项核对）

1. `tests/test_operator_baseline.py`：
   - `OP_FIELDS_SNAPSHOT` 加 3 项
   - `CORPUS` / `CORPUS_CANONICAL` / `CORPUS_EVAL` 各加 3 项
   - `PORTFOLIO_OPS` 加 3 个算子名
2. `tests/test_operator_registry.py:17`：函数名与 docstring 数字 30 → 33
   （**注意**：该函数当前名为 `test_registry_has_31_ops` 但实际算子数是 30，
   名字本来就与实际不符。改名为 `test_registry_has_33_ops` 并把 docstring
   写清楚，顺手修掉这个名不副实）
3. 跑 `uv run pytest tests/test_operator_baseline.py tests/test_operator_registry.py -q`
   —— **`test_preset_hashes_unchanged` 必须绿**。它红了就是改错了

### 步骤 9：Trace 侧与文档

**`crates/code-tools/src/quant_tools.rs`**（不新增工具，只改描述）：
- `quant_validate_factor`：description 补「返回 `mode` 字段标明表达式是时序
  还是横截面」；input_schema 不变
- `quant_preview_factor`：description 补「只支持时序因子；横截面因子
  （含 `cs_rank` / `cs_zscore` / `cs_demean` / `rank` / `top_n`）会被拒绝，
  请直接用 `quant_evaluate_factor`」
- `quant_evaluate_factor`：description 补「支持横截面因子：`cs_rank` 等算子
  可写出行业内排名、截面标准化类因子，按调仓日全池现算」

**`server/skills/quant-research/SKILL.md`** 新增一条约束（编号接 B 层之后）：

```
23. **横截面因子有独立链路**：`cs_rank` / `cs_zscore` / `cs_demean`（可带 `group_by:"industry"`）写出的是横截面因子，`quant_validate_factor` 的 `mode` 字段会标明。横截面因子**不能**走 `quant_preview_factor`（≤5 标的没有截面），也不能回填，必须直接 `quant_evaluate_factor`。用了 `group_by:"industry"` 时，结论必须声明「行业分组使用当前行业而非历史行业，存在轻微前视」—— 这个声明在 evaluate 结果的 `cross_section.note` 里，照抄即可，不要省略。
```

**`quant/docs/a2a-design.md`**：
- catalog 的算子契约段补 3 个 `cs_*`（形状 `{op, input, group_by}`）
- §8.11（preview）补「只接受时序表达式」
- §8.13（evaluate）补截面路径与非 PIT 行业声明

---

## 5. 验收标准

```bash
cd /Users/admin/projects/hank/trace/quant

# 1. 本任务新测试
uv run pytest tests/test_cross_section_factors.py -v

# 2. 冻结基线与算子注册表（最关键的一步）
uv run pytest tests/test_operator_baseline.py tests/test_operator_registry.py -q

# 3. 组合策略回归（需求 §6 R1 点名要求全绿）
uv run pytest tests/test_strategy_spec_regression.py \
    tests/test_portfolio_strategies.py tests/test_expression_api.py \
    tests/test_rolling_ops.py -q

# 4. 因子链路
uv run pytest tests/test_factor_api.py tests/test_factor_backfill.py \
    tests/test_factor_evaluation.py tests/test_factor_correlation.py \
    tests/test_factor_seed_parity.py tests/test_a2a.py -q

# 5. 全量回归
uv run pytest -q
```

```bash
cd /Users/admin/projects/hank/trace
cargo test -p code-tools quant
cargo build --workspace
```

**期望结果**：

- 步骤 1 全绿，16 个用例
- 步骤 2 全绿。**`test_preset_hashes_unchanged` 绿是硬性门槛** ——
  它红意味着改动波及了既有策略规格，必须回退重做而不是更新哈希
- 步骤 3 全绿，一个不许红（这是需求 §6 R1 的验收原话）
- 步骤 4 全绿
- 步骤 5：总数 = B 层完成后的基线 + 本任务新增用例数，失败 0
- Rust 侧全绿（本任务不新增工具，工具数不变）

**人工核对项**（写进回报）：

- `uv run python -c "from app.strategy.operators import OPERATORS; print(len(OPERATORS))"`
  输出 33
- 用真实数据跑一次「行业内 20 日动量 `cs_rank`」的 `factor.evaluate`，
  确认 `ic_decay` 非空且 `cross_section.note` 里有非 PIT 声明
  （需求 §8 C 层验收原话）
- 内存实测（需求 §6 R4）：全市场截面评估跑一次，
  记录峰值内存量级，写进回报。若明显超出可接受范围，
  **不要自行改方案**，在回报里说明并等下一轮指示

---

## 6. 约定

- 中文注释，注释解释**为什么**而不是复述代码
- commit message 用中文，形如
  `feat(quant): 因子求值支持横截面，新增 cs_rank/cs_zscore/cs_demean 算子`
- 不新增依赖
- 保留工作区原有改动，只提交与本任务相关的文件
- 遇到与本文档冲突的既有实现，以**既有实现为准**并在回报里说明冲突点
- **本任务最容易出的错是「为了让基线测试变绿而改既有算子」。**
  基线红了先判断是哪一类：新增算子导致的形状快照不匹配 → 加条目；
  预设策略哈希变了 → 你改错了，回退
