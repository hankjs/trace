"""数据库策略规格、能力解析与规范化哈希。

StrategySpec 只描述日频研究和模拟目标，不包含可执行代码。表达式节点使用固定
操作符和精确字段集合，任何扩展都必须先进入组件注册表并经过代码评审。
"""
from __future__ import annotations

import hashlib
import json
import math
import re
from enum import StrEnum
from functools import reduce
from operator import mul
from typing import Any, Literal

from pydantic import (
    BaseModel, ConfigDict, Field, ValidationError, model_serializer, model_validator,
)

SCHEMA_VERSION = 1
MAX_AST_DEPTH = 12
MAX_AST_NODES = 256
MAX_WINDOW = 500
MAX_PARAMETER_VALUES = 20
MAX_PARAMETER_COMBINATIONS = 256

SUPPORTED_OPERATORS = frozenset({
    "field", "literal", "all", "any", "not",
    "gt", "gte", "lt", "lte", "cross_above", "cross_below",
    "add", "subtract", "multiply", "divide",
    "rolling_mean", "rolling_max", "rolling_min", "rolling_std",
    "rolling_rank", "zscore", "shift",
    "ma", "rsi", "atr", "momentum", "return", "volume_ratio",
    "rank", "top_n",
})

# 字段存在于当前日线、估值或点时财务目录时才可声明。编译时仍要求调用方提供
# 实际用到的列，避免把“目录支持”误解为某一批数据必然完整。
SUPPORTED_FIELDS = frozenset({
    "open", "high", "low", "close", "raw_close", "volume", "amount",
    "is_st", "pe_ttm", "pb", "ps_ttm", "market_cap", "roe",
    "revenue_growth", "profit_growth", "gross_margin", "debt_ratio",
    "cashflow_quality",
})

_OP_FIELDS: dict[str, frozenset[str]] = {
    "field": frozenset({"op", "name"}),
    "literal": frozenset({"op", "value"}),
    "all": frozenset({"op", "args"}),
    "any": frozenset({"op", "args"}),
    "not": frozenset({"op", "arg"}),
    **{op: frozenset({"op", "left", "right"}) for op in (
        "gt", "gte", "lt", "lte", "cross_above", "cross_below",
        "add", "subtract", "multiply", "divide",
    )},
    **{op: frozenset({"op", "input", "window", "shift"}) for op in (
        "rolling_mean", "rolling_max", "rolling_min", "rolling_std",
        "rolling_rank", "zscore", "volume_ratio",
    )},
    "shift": frozenset({"op", "input", "periods"}),
    **{op: frozenset({"op", "input", "window"}) for op in (
        "ma", "rsi", "momentum", "return",
    )},
    "atr": frozenset({"op", "high", "low", "close", "window"}),
    "rank": frozenset({"op", "input", "ascending"}),
    "top_n": frozenset({"op", "input", "n"}),
}


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class CapabilityStatus(StrEnum):
    SUPPORTED = "supported"
    MISSING_DATA = "missing_data"
    MISSING_ENGINE = "missing_engine"
    SUBJECTIVE_ONLY = "subjective_only"
    BOUNDARY_DENIED = "boundary_denied"


class CapabilityIssue(StrictModel):
    status: CapabilityStatus
    path: str
    code: str
    message: str


class CapabilityReport(StrictModel):
    status: CapabilityStatus
    issues: list[CapabilityIssue] = Field(default_factory=list)

    @property
    def supported(self) -> bool:
        return self.status == CapabilityStatus.SUPPORTED


class StrategyCapabilityError(ValueError):
    """运行入口无法满足规格能力时携带结构化报告。"""

    def __init__(self, report: CapabilityReport, message: str = "策略能力不足"):
        self.report = report
        super().__init__(message)


class Expression(StrictModel):
    """受控表达式节点。

    字段在模型层列全，再由 ``op`` 校验精确允许集合。这样未知字段由 Pydantic
    拒绝，同时每种操作符都不能携带其它操作符的参数。
    """

    op: str
    name: str | None = None
    value: bool | int | float | None = None
    args: list[Expression] | None = None
    arg: Expression | None = None
    left: Expression | None = None
    right: Expression | None = None
    input: Expression | None = None
    high: Expression | None = None
    low: Expression | None = None
    close: Expression | None = None
    window: int | None = None
    shift: int | None = None
    periods: int | None = None
    ascending: bool | None = None
    n: int | None = None

    @model_validator(mode="before")
    @classmethod
    def validate_operator_shape(cls, value: Any) -> Any:
        if not isinstance(value, dict):
            raise ValueError("表达式节点必须是对象")
        op = value.get("op")
        if not isinstance(op, str):
            raise ValueError("表达式节点必须包含字符串 op")
        allowed = _OP_FIELDS.get(op)
        if allowed is None:
            raise ValueError(f"不支持的操作符: {op}")
        actual = frozenset(value)
        if actual != allowed:
            missing = sorted(allowed - actual)
            unknown = sorted(actual - allowed)
            details = []
            if missing:
                details.append(f"缺少字段 {missing}")
            if unknown:
                details.append(f"未知字段 {unknown}")
            raise ValueError(f"操作符 {op} 形状错误: {'; '.join(details)}")
        return value

    @model_validator(mode="after")
    def validate_values(self) -> Expression:
        if self.op == "field" and (not self.name or not re.fullmatch(r"[a-z][a-z0-9_]*", self.name)):
            raise ValueError("field.name 必须是 snake_case 字段名")
        if self.op == "literal":
            if self.value is None or isinstance(self.value, str):
                raise ValueError("literal.value 只允许有限数字或布尔值")
            if isinstance(self.value, float) and not math.isfinite(self.value):
                raise ValueError("literal.value 必须是有限数字")
        if self.op in {"all", "any"} and not self.args:
            raise ValueError(f"{self.op}.args 不能为空")
        if self.op in {
            "rolling_mean", "rolling_max", "rolling_min", "rolling_std",
            "rolling_rank", "zscore", "volume_ratio",
        }:
            if self.window is None or not 2 <= self.window <= MAX_WINDOW:
                raise ValueError(f"{self.op}.window 必须在 2 到 {MAX_WINDOW} 之间")
            if self.shift is None or not 0 <= self.shift <= MAX_WINDOW:
                raise ValueError(f"{self.op}.shift 必须在 0 到 {MAX_WINDOW} 之间")
        if self.op in {"ma", "rsi", "atr", "momentum", "return"}:
            if self.window is None or not 2 <= self.window <= MAX_WINDOW:
                raise ValueError(f"{self.op}.window 必须在 2 到 {MAX_WINDOW} 之间")
        if self.op == "shift" and (
            self.periods is None or not 1 <= self.periods <= MAX_WINDOW
        ):
            raise ValueError(f"shift.periods 必须在 1 到 {MAX_WINDOW} 之间")
        if self.op == "top_n" and (self.n is None or not 1 <= self.n <= 500):
            raise ValueError("top_n.n 必须在 1 到 500 之间")
        self._validate_types()
        return self

    def _validate_types(self) -> str:
        child_types: list[str]
        if self.op == "field":
            return "number"
        if self.op == "literal":
            return "bool" if isinstance(self.value, bool) else "number"
        if self.op in {"all", "any"}:
            child_types = [item._validate_types() for item in self.args or []]
            if any(kind != "bool" for kind in child_types):
                raise ValueError(f"{self.op} 只接受布尔表达式")
            return "bool"
        if self.op == "not":
            if self.arg is None or self.arg._validate_types() != "bool":
                raise ValueError("not.arg 必须是布尔表达式")
            return "bool"
        if self.op in {"gt", "gte", "lt", "lte", "cross_above", "cross_below"}:
            if self.left is None or self.right is None:
                raise ValueError(f"{self.op} 缺少比较参数")
            if self.left._validate_types() != "number" or self.right._validate_types() != "number":
                raise ValueError(f"{self.op} 只接受数值表达式")
            return "bool"
        if self.op in {"add", "subtract", "multiply", "divide"}:
            if self.left is None or self.right is None:
                raise ValueError(f"{self.op} 缺少运算参数")
            if self.left._validate_types() != "number" or self.right._validate_types() != "number":
                raise ValueError(f"{self.op} 只接受数值表达式")
            return "number"
        for child in (self.input, self.high, self.low, self.close):
            if child is not None and child._validate_types() != "number":
                raise ValueError(f"{self.op} 只接受数值表达式")
        return "bool" if self.op == "top_n" else "number"

    @model_serializer(mode="plain")
    def serialize_exact_shape(self) -> dict[str, Any]:
        """序列化时只保留该操作符的正式字段，不输出其它可选槽位的 null。"""
        result: dict[str, Any] = {"op": self.op}
        for key in _OP_FIELDS[self.op] - {"op"}:
            value = getattr(self, key)
            if isinstance(value, Expression):
                result[key] = value.serialize_exact_shape()
            elif isinstance(value, list):
                result[key] = [
                    item.serialize_exact_shape() if isinstance(item, Expression) else item
                    for item in value
                ]
            else:
                result[key] = value
        return result


class SourceSpec(StrictModel):
    book: str = Field(min_length=1, max_length=200)
    candidate_id: str = Field(min_length=1, max_length=100)


class MetadataSpec(StrictModel):
    canonical_id: str = Field(min_length=1, max_length=100)
    sources: list[SourceSpec] = Field(min_length=1, max_length=20)
    evidence_status: Literal[
        "unverified", "design_complete", "backtested", "oos_passed", "rejected"
    ]
    hypothesis: str = Field(min_length=1, max_length=1000)


class UniverseSpec(StrictModel):
    pool_id: int = Field(ge=1)
    exclude_st: bool
    min_listing_days: int = Field(ge=0, le=3650)
    min_amount_avg20: float = Field(ge=0)


class DataRequirementSpec(StrictModel):
    field: str
    availability: Literal["daily_close", "daily_open", "point_in_time"]
    required: bool = True


class RuleSpec(StrictModel):
    condition: Expression
    reason_code: str = Field(pattern=r"^[a-z][a-z0-9_]{0,99}$")

    @model_validator(mode="after")
    def require_boolean(self) -> RuleSpec:
        if self.condition._validate_types() != "bool":
            raise ValueError("规则 condition 必须返回布尔值")
        return self


class SinglePositioningSpec(StrictModel):
    type: Literal["binary", "fixed"]
    target: float = Field(gt=0, le=1)


class SelectionSpec(StrictModel):
    type: Literal["top_n"]
    n: int = Field(ge=1, le=500)


class WeightingSpec(StrictModel):
    type: Literal["equal", "rank"]


class RebalanceSpec(StrictModel):
    frequency: Literal["fixed", "weekly", "monthly"]
    interval_days: int | None

    @model_validator(mode="after")
    def validate_frequency(self) -> RebalanceSpec:
        if self.frequency == "fixed":
            if self.interval_days is None or not 1 <= self.interval_days <= 250:
                raise ValueError("固定周期调仓必须提供 1 到 250 的 interval_days")
        elif self.interval_days is not None:
            raise ValueError("weekly/monthly 调仓的 interval_days 必须为 null")
        return self


class PortfolioPositioningSpec(StrictModel):
    type: Literal["portfolio"]
    score: Expression
    selection: SelectionSpec
    weighting: WeightingSpec
    rebalance: RebalanceSpec
    risk_filter: Expression | None

    @model_validator(mode="after")
    def validate_outputs(self) -> PortfolioPositioningSpec:
        if self.score._validate_types() != "number":
            raise ValueError("组合 score 必须返回数值")
        if self.risk_filter is not None and self.risk_filter._validate_types() != "bool":
            raise ValueError("组合 risk_filter 必须返回布尔值")
        return self


class HoldingSpec(StrictModel):
    """持仓期间的行为规则。

    加仓/减仓语义(仅 single 策略):
    - entry 触发建仓 ``positioning.target``(单档基准);
    - 持仓期间(非 entry 当日)``add_rule`` 触发时目标仓位上调 ``step``(占总资金
      比例),上限 ``max_position``;``reduce_rule`` 触发时下调 ``step``,减到 0
      等同清仓,按原生离场的既有语义等待下一次入场事件;
    - 同一日多条件冲突的优先级:exit/overlay > reduce > add。
    """

    allow_add: bool
    allow_reduce: bool
    add_rule: RuleSpec | None = None
    reduce_rule: RuleSpec | None = None
    step: float = Field(default=0.5, gt=0, lt=1)
    max_position: float = Field(default=1.0, gt=0, le=1)
    cooldown_days: int = Field(ge=0, le=250)
    risk_reentry: Literal["native_reset"]

    @model_validator(mode="after")
    def validate_adjust_rules(self) -> HoldingSpec:
        if self.allow_add and self.add_rule is None:
            raise ValueError("allow_add 为 true 时必须提供 add_rule")
        if not self.allow_add and self.add_rule is not None:
            raise ValueError("allow_add 为 false 时 add_rule 必须为 null")
        if self.allow_reduce and self.reduce_rule is None:
            raise ValueError("allow_reduce 为 true 时必须提供 reduce_rule")
        if not self.allow_reduce and self.reduce_rule is not None:
            raise ValueError("allow_reduce 为 false 时 reduce_rule 必须为 null")
        return self

    @model_serializer(mode="plain")
    def serialize_compat(self) -> dict[str, Any]:
        """无加减档规则时保持首期序列化形状,旧规格的规范化哈希不变。"""
        result: dict[str, Any] = {
            "allow_add": self.allow_add,
            "allow_reduce": self.allow_reduce,
            "cooldown_days": self.cooldown_days,
            "risk_reentry": self.risk_reentry,
        }
        if self.add_rule is not None or self.reduce_rule is not None:
            result["add_rule"] = (
                self.add_rule.model_dump(mode="json")
                if self.add_rule is not None else None
            )
            result["reduce_rule"] = (
                self.reduce_rule.model_dump(mode="json")
                if self.reduce_rule is not None else None
            )
            result["step"] = self.step
            result["max_position"] = self.max_position
        return result


class OverlayRuleSpec(StrictModel):
    enabled: bool
    type: Literal["fixed_pct", "atr_multiple"]
    value: float = Field(gt=0)
    atr_period: int = Field(ge=2, le=250)
    trailing: bool

    @model_validator(mode="after")
    def validate_limit(self) -> OverlayRuleSpec:
        maximum = 1.0 if self.type == "fixed_pct" else 50.0
        if self.value > maximum:
            raise ValueError(f"{self.type} value 不能超过 {maximum}")
        return self


class OverlaysSpec(StrictModel):
    risk: OverlayRuleSpec
    take_profit: OverlayRuleSpec


class PortfolioConstraintsSpec(StrictModel):
    long_only: Literal[True]
    max_positions: int = Field(ge=1, le=500)
    max_single_weight: float = Field(gt=0, le=1)
    max_total_weight: float = Field(gt=0, le=1)


class ExecutionSpec(StrictModel):
    signal_time: Literal["close"]
    execution_time: Literal["next_open"]
    buy_limit_policy: Literal["reject"]
    sell_limit_policy: Literal["retry"]
    suspension_policy: Literal["reject_entry_retry_exit"]
    missing_bar_policy: Literal["reject_entry_retry_exit"]
    cost_model: Literal["a_share_daily_v1"]
    max_entry_premium: float = Field(ge=0, le=1)


class ParameterScanSpec(StrictModel):
    path: str = Field(pattern=r"^\$\.[a-zA-Z0-9_.]+$")
    values: list[bool | int | float] = Field(min_length=1, max_length=MAX_PARAMETER_VALUES)

    @model_validator(mode="after")
    def finite_values(self) -> ParameterScanSpec:
        for value in self.values:
            if isinstance(value, float) and not math.isfinite(value):
                raise ValueError("参数扫描值必须是有限数字")
        return self


# 结构化否决规则可用的指标。excess_annual_return_vs_best_baseline 依赖对照
# 基线已成功计算,缺少基线时该条规则如实记为不可评估(见 backtest/validation.py)。
REJECTION_RULE_METRICS = frozenset({
    "total_return", "annual_return", "max_drawdown", "sharpe", "win_rate",
    "trade_count", "round_trips", "excess_annual_return_vs_best_baseline",
})


class RejectionRuleSpec(StrictModel):
    """结构化否决规则:回测指标命中比较式即否决。

    op 刻意只取 SUPPORTED_OPERATORS 子集:能力扫描把规格里任何 ``op`` 键都按
    表达式操作符校验,引入 "eq" 之类新词会被误报为 unknown_operator。
    """

    metric: str
    op: Literal["lt", "lte", "gt", "gte"]
    threshold: float
    segment: Literal["full", "in_sample", "oos"] = "full"
    description: str = Field(default="", max_length=200)

    @model_validator(mode="after")
    def validate_rule(self) -> RejectionRuleSpec:
        if self.metric not in REJECTION_RULE_METRICS:
            raise ValueError(
                f"否决规则不支持指标 {self.metric},可选: "
                f"{', '.join(sorted(REJECTION_RULE_METRICS))}"
            )
        if not math.isfinite(self.threshold):
            raise ValueError("否决规则 threshold 必须是有限数字")
        return self


class ValidationSpec(StrictModel):
    baseline_ids: list[str] = Field(min_length=1, max_length=20)
    locked_oos: bool
    rejection_criteria: list[str] = Field(min_length=1, max_length=20)
    parameter_scans: list[ParameterScanSpec] = Field(max_length=20)
    # 结构化否决规则;旧字符串条件保持原样,由 backtest/validation.py 的
    # 兼容映射解释,两者并存时逐条评估。
    rejection_rules: list[RejectionRuleSpec] = Field(
        default_factory=list, max_length=20,
    )

    @model_validator(mode="after")
    def limit_scan_combinations(self) -> ValidationSpec:
        combinations = reduce(mul, (len(item.values) for item in self.parameter_scans), 1)
        if combinations > MAX_PARAMETER_COMBINATIONS:
            raise ValueError(
                f"参数扫描组合数 {combinations} 超过 {MAX_PARAMETER_COMBINATIONS}"
            )
        return self

    @model_serializer(mode="plain")
    def serialize_compat(self) -> dict[str, Any]:
        """无结构化否决规则时保持首期序列化形状,六个预设的规范化哈希不变。"""
        result: dict[str, Any] = {
            "baseline_ids": list(self.baseline_ids),
            "locked_oos": self.locked_oos,
            "parameter_scans": [
                item.model_dump(mode="json") for item in self.parameter_scans
            ],
            "rejection_criteria": list(self.rejection_criteria),
        }
        if self.rejection_rules:
            result["rejection_rules"] = [
                item.model_dump(mode="json") for item in self.rejection_rules
            ]
        return result


class StrategySpec(StrictModel):
    schema_version: Literal[SCHEMA_VERSION]
    kind: Literal["single", "portfolio"]
    metadata: MetadataSpec
    universe: UniverseSpec
    data_requirements: list[DataRequirementSpec] = Field(min_length=1, max_length=100)
    entry: RuleSpec
    positioning: SinglePositioningSpec | PortfolioPositioningSpec
    holding: HoldingSpec
    native_exit: RuleSpec | None
    overlays: OverlaysSpec
    portfolio_constraints: PortfolioConstraintsSpec
    execution: ExecutionSpec
    validation: ValidationSpec

    @model_validator(mode="after")
    def validate_complete_strategy(self) -> StrategySpec:
        if self.kind == "single":
            if not isinstance(self.positioning, SinglePositioningSpec):
                raise ValueError("single 策略必须使用 binary/fixed positioning")
            if self.native_exit is None:
                raise ValueError("single 策略必须包含原生离场")
        elif not isinstance(self.positioning, PortfolioPositioningSpec):
            raise ValueError("portfolio 策略必须使用 portfolio positioning")
        if self.kind == "portfolio" and (
            self.holding.allow_add or self.holding.allow_reduce
        ):
            raise ValueError("组合策略暂不支持加仓/减仓(allow_add/allow_reduce)")

        expressions = list(_iter_spec_expressions(self))
        node_count = sum(_expression_stats(expr)[0] for expr in expressions)
        max_depth = max((_expression_stats(expr)[1] for expr in expressions), default=0)
        if node_count > MAX_AST_NODES:
            raise ValueError(f"AST 节点数 {node_count} 超过 {MAX_AST_NODES}")
        if max_depth > MAX_AST_DEPTH:
            raise ValueError(f"AST 深度 {max_depth} 超过 {MAX_AST_DEPTH}")

        used_fields = {
            node.name
            for expr in expressions
            for node in _walk_expression(expr)
            if node.op == "field" and node.name is not None
        }
        declared = {item.field for item in self.data_requirements if item.required}
        missing = sorted(used_fields - declared)
        if missing:
            raise ValueError(f"data_requirements 未声明字段: {missing}")
        overlay_fields: set[str] = set()
        for overlay in (self.overlays.risk, self.overlays.take_profit):
            if overlay.enabled:
                overlay_fields.add("close")
                if overlay.type == "atr_multiple":
                    overlay_fields.update({"high", "low"})
        missing_overlay = sorted(overlay_fields - declared)
        if missing_overlay:
            raise ValueError(f"data_requirements 未声明覆盖层字段: {missing_overlay}")
        return self


class StrategyValidationResult(StrictModel):
    valid: bool
    spec: StrategySpec | None
    spec_hash: str | None
    canonical_json: str | None
    capability: CapabilityReport


def _iter_spec_expressions(spec: StrategySpec):
    yield spec.entry.condition
    if spec.native_exit is not None:
        yield spec.native_exit.condition
    if spec.holding.add_rule is not None:
        yield spec.holding.add_rule.condition
    if spec.holding.reduce_rule is not None:
        yield spec.holding.reduce_rule.condition
    if isinstance(spec.positioning, PortfolioPositioningSpec):
        yield spec.positioning.score
        if spec.positioning.risk_filter is not None:
            yield spec.positioning.risk_filter


def _walk_expression(expr: Expression):
    yield expr
    for child in (expr.arg, expr.left, expr.right, expr.input, expr.high, expr.low, expr.close):
        if child is not None:
            yield from _walk_expression(child)
    for child in expr.args or []:
        yield from _walk_expression(child)


def _expression_stats(expr: Expression) -> tuple[int, int]:
    children = [
        child for child in (
            expr.arg, expr.left, expr.right, expr.input, expr.high, expr.low, expr.close,
        ) if child is not None
    ] + list(expr.args or [])
    if not children:
        return 1, 1
    stats = [_expression_stats(child) for child in children]
    return 1 + sum(item[0] for item in stats), 1 + max(item[1] for item in stats)


def canonical_spec_json(spec: StrategySpec | dict[str, Any]) -> str:
    parsed = spec if isinstance(spec, StrategySpec) else StrategySpec.model_validate(spec)
    return json.dumps(
        parsed.model_dump(mode="json"),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


def strategy_spec_hash(spec: StrategySpec | dict[str, Any]) -> str:
    return hashlib.sha256(canonical_spec_json(spec).encode("utf-8")).hexdigest()


def parse_strategy_spec(value: StrategySpec | dict[str, Any] | str) -> StrategySpec:
    if isinstance(value, StrategySpec):
        return value
    if isinstance(value, str):
        value = json.loads(value)
    return StrategySpec.model_validate(value)


def resolve_capabilities(
    value: Any,
    *,
    available_fields: set[str] | frozenset[str] | None = None,
) -> CapabilityReport:
    """对未解析 JSON 返回带精确路径的能力报告。"""
    issues: list[CapabilityIssue] = []

    def add(status: CapabilityStatus, path: str, code: str, message: str) -> None:
        item = CapabilityIssue(status=status, path=path, code=code, message=message)
        if item not in issues:
            issues.append(item)

    _scan_raw_capabilities(value, "$", add)
    _scan_holding_rules(value, add)
    _scan_overlay_requirements(value, add)
    try:
        spec = parse_strategy_spec(value)
    except (ValidationError, ValueError, TypeError, json.JSONDecodeError) as exc:
        if isinstance(exc, ValidationError):
            for error in exc.errors(include_url=False):
                path = "$" + "".join(
                    f"[{part}]" if isinstance(part, int) else f".{part}"
                    for part in error["loc"]
                )
                add(
                    CapabilityStatus.MISSING_ENGINE,
                    path,
                    "invalid_spec",
                    error["msg"],
                )
        elif not issues:
            add(CapabilityStatus.MISSING_ENGINE, "$", "invalid_spec", str(exc))
    else:
        for index, requirement in enumerate(spec.data_requirements):
            if requirement.field not in SUPPORTED_FIELDS:
                add(
                    CapabilityStatus.MISSING_DATA,
                    f"$.data_requirements[{index}].field",
                    "unknown_field",
                    f"当前数据目录不支持字段 {requirement.field}",
                )
            elif available_fields is not None and requirement.required and (
                requirement.field not in available_fields
            ):
                add(
                    CapabilityStatus.MISSING_DATA,
                    f"$.data_requirements[{index}].field",
                    "field_not_available",
                    f"本次数据快照缺少字段 {requirement.field}",
                )

    if not issues:
        return CapabilityReport(status=CapabilityStatus.SUPPORTED, issues=[])
    priority = {
        CapabilityStatus.BOUNDARY_DENIED: 0,
        CapabilityStatus.SUBJECTIVE_ONLY: 1,
        CapabilityStatus.MISSING_DATA: 2,
        CapabilityStatus.MISSING_ENGINE: 3,
        CapabilityStatus.SUPPORTED: 4,
    }
    status = min((item.status for item in issues), key=priority.__getitem__)
    return CapabilityReport(status=status, issues=issues)


def validate_strategy_spec(
    value: Any,
    *,
    available_fields: set[str] | frozenset[str] | None = None,
) -> StrategyValidationResult:
    capability = resolve_capabilities(value, available_fields=available_fields)
    if not capability.supported:
        return StrategyValidationResult(
            valid=False,
            spec=None,
            spec_hash=None,
            canonical_json=None,
            capability=capability,
        )
    spec = parse_strategy_spec(value)
    canonical = canonical_spec_json(spec)
    return StrategyValidationResult(
        valid=True,
        spec=spec,
        spec_hash=hashlib.sha256(canonical.encode("utf-8")).hexdigest(),
        canonical_json=canonical,
        capability=capability,
    )


def _scan_raw_capabilities(value: Any, path: str, add) -> None:
    forbidden_keys = {
        "code", "expression", "python", "sql", "javascript", "js", "shell",
        "command", "network", "url", "import", "module", "eval", "exec",
        "dynamic_import", "http", "request", "fetch",
        "broker", "order_submission", "auto_trade",
    }
    subjective_ops = {"subjective", "manual_judgment", "chart_pattern_manual"}
    boundary_ops = {"broker_order", "submit_order", "intraday", "high_frequency"}
    denied_patterns = (
        r"\beval\s*\(", r"\bexec\s*\(", r"\bimport\s+[a-zA-Z_]",
        r"\bfrom\s+[a-zA-Z_.]+\s+import\b", r"\b__import__\s*\(",
        r"\b(select\b.+\bfrom|insert\s+into|update\b.+\bset|delete\s+from|drop\s+table|alter\s+table)\b",
        r"<script\b", r"\bfunction\s*\(", r"=>", r"https?://",
        r"\bos\.system\s*\(", r"\bsubprocess\b", r"\blambda\b", r"\$\(",
    )
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = f"{path}.{key}"
            lowered = str(key).lower()
            if lowered in forbidden_keys:
                add(
                    CapabilityStatus.BOUNDARY_DENIED,
                    child_path,
                    "arbitrary_code_denied",
                    f"字段 {key} 可能引入任意代码、外部调用或真实交易能力，已拒绝",
                )
            if key == "op" and isinstance(child, str):
                if child in subjective_ops:
                    add(
                        CapabilityStatus.SUBJECTIVE_ONLY, child_path,
                        "subjective_operator", f"操作符 {child} 不能形成一致机器定义",
                    )
                elif child in boundary_ops:
                    add(
                        CapabilityStatus.BOUNDARY_DENIED, child_path,
                        "product_boundary", f"操作符 {child} 超出日频研究边界",
                    )
                elif child not in SUPPORTED_OPERATORS:
                    add(
                        CapabilityStatus.MISSING_ENGINE, child_path,
                        "unknown_operator", f"当前编译器不支持操作符 {child}",
                    )
            if key == "name" and value.get("op") == "field" and isinstance(child, str):
                if child not in SUPPORTED_FIELDS:
                    add(
                        CapabilityStatus.MISSING_DATA, child_path,
                        "unknown_field", f"当前数据目录不支持字段 {child}",
                    )
            if key == "signal_time" and child != "close":
                add(
                    CapabilityStatus.BOUNDARY_DENIED, child_path,
                    "product_boundary", "当前产品只允许日频收盘形成信号",
                )
            if key == "execution_time" and child != "next_open":
                add(
                    CapabilityStatus.BOUNDARY_DENIED, child_path,
                    "product_boundary", "当前产品只允许 T+1 开盘模拟成交",
                )
            if key == "availability" and child == "subjective":
                add(
                    CapabilityStatus.SUBJECTIVE_ONLY, child_path,
                    "subjective_data", "主观输入不能形成一致机器定义",
                )
            _scan_raw_capabilities(child, child_path, add)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _scan_raw_capabilities(child, f"{path}[{index}]", add)
    elif isinstance(value, str):
        for pattern in denied_patterns:
            if re.search(pattern, value, flags=re.IGNORECASE | re.DOTALL):
                add(
                    CapabilityStatus.BOUNDARY_DENIED,
                    path,
                    "arbitrary_code_denied",
                    "检测到任意代码、SQL、脚本、shell 或网络表达式",
                )
                break


def _scan_holding_rules(value: Any, add) -> None:
    """加仓/减仓规则的结构性能力检查:精确到 holding 下的具体字段。"""
    if not isinstance(value, dict):
        return
    holding = value.get("holding")
    if not isinstance(holding, dict):
        return
    for flag, rule_key in (("allow_add", "add_rule"), ("allow_reduce", "reduce_rule")):
        if holding.get(flag) is True and holding.get(rule_key) is None:
            add(
                CapabilityStatus.MISSING_ENGINE,
                f"$.holding.{rule_key}",
                "holding_rule_missing",
                f"{flag} 为 true 时必须提供 {rule_key}",
            )
    if value.get("kind") == "portfolio":
        for flag in ("allow_add", "allow_reduce"):
            if holding.get(flag) is True:
                add(
                    CapabilityStatus.MISSING_ENGINE,
                    f"$.holding.{flag}",
                    "holding_adjust_portfolio",
                    "组合策略暂不支持加仓/减仓",
                )


def _scan_overlay_requirements(value: Any, add) -> None:
    if not isinstance(value, dict):
        return
    requirements = value.get("data_requirements")
    overlays = value.get("overlays")
    if not isinstance(requirements, list) or not isinstance(overlays, dict):
        return
    declared = {
        item.get("field") for item in requirements
        if isinstance(item, dict) and item.get("required", True) is True
    }
    for name in ("risk", "take_profit"):
        overlay = overlays.get(name)
        if not isinstance(overlay, dict) or overlay.get("enabled") is not True:
            continue
        required = {"close"}
        if overlay.get("type") == "atr_multiple":
            required.update({"high", "low"})
        for field in sorted(required - declared):
            add(
                CapabilityStatus.MISSING_DATA,
                f"$.overlays.{name}",
                "overlay_field_not_declared",
                f"覆盖层 {name} 需要在 data_requirements 声明字段 {field}",
            )


__all__ = [
    "CapabilityIssue", "CapabilityReport", "CapabilityStatus", "DataRequirementSpec",
    "ExecutionSpec", "Expression", "HoldingSpec", "MAX_AST_DEPTH", "MAX_AST_NODES",
    "MAX_PARAMETER_COMBINATIONS", "MAX_WINDOW", "MetadataSpec", "OverlaysSpec",
    "PortfolioConstraintsSpec", "PortfolioPositioningSpec", "REJECTION_RULE_METRICS",
    "RejectionRuleSpec", "RuleSpec", "SCHEMA_VERSION",
    "SUPPORTED_FIELDS", "SUPPORTED_OPERATORS", "SinglePositioningSpec", "StrategySpec",
    "StrategyCapabilityError", "StrategyValidationResult", "UniverseSpec",
    "ValidationSpec", "canonical_spec_json",
    "parse_strategy_spec", "resolve_capabilities", "strategy_spec_hash",
    "validate_strategy_spec",
]
