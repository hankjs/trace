"""研究计划的纯计算层。

这里把六种模板适配为统一信息模型，但不改变模板的持仓算法。所有价格参考都
必须能由当日及此前数据客观算出；缺数据或依赖未来模拟成交价时明确记录计算
状态，不用空上下界伪装成价格区间。
"""
from __future__ import annotations

import math
from copy import deepcopy
from datetime import date
from typing import Any, Mapping

import pandas as pd

from ..indicators import atr, ma, rsi
from ..catalog import STRATEGY_TEMPLATES
from ..selection.pipeline import SCORE_WEIGHTS
from ..strategy.compiler import (
    COMPILER_VERSION,
    SingleCompilation,
    compile_single,
    component_versions_for_spec,
)
from ..strategy.components import build_reason_tree, evaluate_expression
from ..strategy.overlays import overlay_price_line
from ..strategy.runtime import build_execution_snapshot, strategy_spec_for
from ..strategy.spec import Expression, PortfolioPositioningSpec, StrategySpec
from ..strategy.strategies import REGISTRY
from ..strategy.watch import assess_entry_watch

PRODUCT_BOUNDARY = (
    "本计划根据日频数据和策略规则生成，仅用于研究。真实买卖、价格、数量和风险决策"
    "由你在外部交易应用中确认。"
)
ADAPTER_VERSION = 1

STATUS_NAMES = {
    "current": "当前有效",
    "reevaluate": "需要重新评估",
    "invalid": "已失效",
    "exit_triggered": "已触发退出",
    "expired": "已过期",
}

PLAN_TYPE_NAMES = {
    "single": "单标的研究计划",
    "portfolio_rebalance": "组合调仓研究计划",
}

# 模板能力声明以 catalog 为唯一来源；策略列表和计划生成器不维护第二套词典。
CAPABILITIES: dict[str, dict[str, Any]] = {
    name: deepcopy(metadata["plan_capability"])
    for name, metadata in STRATEGY_TEMPLATES.items()
}


def _number(value: Any, digits: int = 4) -> float | None:
    if value is None or pd.isna(value):
        return None
    number = float(value)
    if not math.isfinite(number):
        return None
    return round(number, digits)


def _native_params(template: str, supplied: Mapping[str, Any] | None) -> dict:
    module = REGISTRY[template]
    defaults = dict(getattr(module, "DEFAULT_PARAMS", {}))
    values = supplied or {}
    return {key: values.get(key, default) for key, default in defaults.items()}


def evaluate_single_entry_condition(
    template: str,
    df: pd.DataFrame,
    effective_params: Mapping[str, Any],
    signal_type: str,
) -> dict:
    """按计划快照重算最新确认日的原生入场或观察条件。

    `satisfied=None` 表示历史窗口不足，不能把缺数据误判为条件失效。watch
    计划既接受已经满足原生入场条件，也接受模板自身的临近触发条件。
    """
    if template not in REGISTRY or getattr(REGISTRY[template], "KIND", None) != "single":
        raise ValueError(f"模板 {template} 不是单标的策略模板")
    if df.empty:
        return {"satisfied": None, "text": "没有可用于重算的已确认日线数据。"}

    params = _native_params(template, effective_params)
    close = df["close"].astype(float)
    entry_satisfied: bool

    if template == "ma_cross":
        fast = ma(close, int(params["fast"]))
        slow = ma(close, int(params["slow"]))
        fast_value, slow_value = fast.iat[-1], slow.iat[-1]
        if pd.isna(fast_value) or pd.isna(slow_value):
            return {"satisfied": None, "text": "快慢均线历史窗口不足，无法重算原生条件。"}
        entry_satisfied = bool(fast_value > slow_value)
        text = (
            f"短期均线 {float(fast_value):.4f} 不再高于长期均线"
            f" {float(slow_value):.4f}。"
        )
    elif template == "breakout":
        window = int(params["entry"])
        entry_line = df["high"].astype(float).shift(1).rolling(window).max().iat[-1]
        if pd.isna(entry_line):
            return {"satisfied": None, "text": f"不足 {window} 个历史交易日，无法重算突破条件。"}
        current_close = float(close.iat[-1])
        entry_satisfied = bool(current_close > entry_line)
        text = (
            f"收盘价 {current_close:.4f} 未突破前 {window} 个交易日高点"
            f" {float(entry_line):.4f}。"
        )
    elif template == "mean_reversion":
        rsi_value = rsi(close, 14).iat[-1]
        ma_window = int(params["ma"])
        ma_value = ma(close, ma_window).iat[-1]
        if pd.isna(rsi_value) or pd.isna(ma_value):
            return {"satisfied": None, "text": "RSI 或长期均线历史窗口不足，无法重算原生条件。"}
        current_close = float(close.iat[-1])
        entry_satisfied = bool(
            rsi_value < float(params["rsi_buy"]) and current_close > ma_value
        )
        text = (
            f"RSI 14 为 {float(rsi_value):.2f}、收盘价为 {current_close:.4f}，"
            f"不再同时满足 RSI 低于 {float(params['rsi_buy']):g} 且收盘价高于"
            f" {ma_window} 日均线 {float(ma_value):.4f}。"
        )
    elif template == "volume_breakout":
        window = int(params["window"])
        high_line = df["high"].astype(float).shift(1).rolling(window).max().iat[-1]
        low_line = df["low"].astype(float).shift(1).rolling(window).min().iat[-1]
        volume = df["volume"].astype(float)
        vol_ma5 = volume.shift(1).rolling(5).mean().iat[-1]
        vol_ma_window = volume.shift(1).rolling(window).mean().iat[-1]
        if any(pd.isna(value) for value in (high_line, low_line, vol_ma5, vol_ma_window)):
            return {"satisfied": None, "text": "平台或成交量历史窗口不足，无法重算放量突破条件。"}
        current_close = float(close.iat[-1])
        contracted = (float(high_line) - float(low_line)) / current_close <= float(
            params["range_max"]
        )
        shrink = float(vol_ma5) < float(vol_ma_window)
        burst = float(volume.iat[-1]) > float(params["vol_mult"]) * float(vol_ma_window)
        entry_satisfied = bool(
            contracted and shrink and burst and current_close > float(high_line)
        )
        text = "缩量平台、当日放量和收盘突破平台上沿的组合条件不再同时满足。"
    else:
        raise ValueError(f"模板 {template} 不是已支持的单标的研究计划")

    watch_satisfied = False
    if signal_type == "watch" and not entry_satisfied:
        watch_satisfied = REGISTRY[template].watch(df, params) is not None
    satisfied = entry_satisfied or watch_satisfied
    if signal_type == "watch" and not satisfied:
        text = f"{text.rstrip('。')}，且不再满足模板的临近观察条件。"
    return {"satisfied": satisfied, "text": text}


def parameter_snapshot(strategy: Any) -> dict:
    """固化完整规格和通用编译元数据。

    ``strategy_params`` / ``effective_params`` 仅为旧客户端兼容字段；新执行路径
    只读取 ``strategy_spec``，不会再按模板参数重建规则。
    """
    from ..backtest.engine import DEFAULT_COSTS

    spec = strategy_spec_for(strategy)
    spec_snapshot = spec.model_dump(mode="json")
    components = component_versions_for_spec(spec)
    risk = spec.overlays.risk.model_dump(mode="json")
    profit = spec.overlays.take_profit.model_dump(mode="json")
    return {
        "strategy_params": dict(strategy.params or {}),
        "effective_params": deepcopy(spec_snapshot),
        "strategy_spec": deepcopy(spec_snapshot),
        "strategy_spec_hash": strategy_version(strategy, {}),
        "compiler_version": COMPILER_VERSION,
        "component_versions": components,
        "risk_overlay": risk,
        "take_profit": profit,
        "simulation_costs": deepcopy(DEFAULT_COSTS),
    }


def strategy_version(strategy: Any, params_snapshot: dict) -> str:
    """计划版本就是完整 StrategySpec 的规范化 SHA-256。"""
    from ..strategy.spec import strategy_spec_hash

    return strategy_spec_hash(strategy_spec_for(strategy))


def _base_observation(kind: str, data_date: date, explanation: str,
                      valid_until: date | None) -> dict:
    return {
        "kind": kind,
        "name": {
            "none": "条件观察",
            "line": "进场观察线",
            "range": "进场观察区间",
            "portfolio_rebalance": "调仓目标权重",
        }[kind],
        "data_date": str(data_date),
        "explanation": explanation,
        "valid_until": str(valid_until) if valid_until else None,
        "reevaluate_when": [
            "出现新的已确认收盘数据",
            "下一模拟成交日行情不可成交或明显偏离当前条件",
        ],
    }


def _premium(params: Mapping[str, Any]) -> float | None:
    for key in ("max_entry_premium", "max_entry_premium_pct", "entry_premium_pct"):
        value = params.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0:
            return float(value)
    return None


def _line_or_range(line: float | None, params: Mapping[str, Any], *,
                   data_date: date, explanation: str,
                   valid_until: date | None, basis: dict) -> dict:
    premium = _premium(params)
    if line is None:
        result = _base_observation("line", data_date, explanation, valid_until)
        result.update({"calculation_status": "insufficient_data", "basis": basis})
        return result
    kind = "range" if premium is not None else "line"
    result = _base_observation(kind, data_date, explanation, valid_until)
    result.update({"calculation_status": "calculated", "basis": basis,
                   "line": _number(line)})
    if premium is not None:
        # 突破类策略只有“高于观察线最多容忍多少溢价”这一客观参数；下界就是
        # 观察线，不对其下方虚构对称区间。
        result.update({"lower": _number(line),
                       "upper": _number(line * (1 + premium)),
                       "tolerance": {"value": premium, "unit": "ratio"}})
    return result


def _overlay_rules(params_snapshot: dict, *, entry_price: float | None,
                   df: pd.DataFrame | None, data_date: date) -> tuple[list[dict], dict]:
    def atr_value(config: dict) -> float | None:
        if df is None or df.empty or config.get("type") != "atr_multiple":
            return None
        values = atr(df["high"], df["low"], df["close"],
                     int(config.get("atr_period", 14)))
        return _number(values.iat[-1])

    risk = params_snapshot["risk_overlay"]
    risk_rule = {
        "source": "overlay", "name": "用户启用的风险覆盖层",
        "enabled": bool(risk.get("enabled", False)), "config": risk,
        "data_date": str(data_date),
    }
    if risk_rule["enabled"]:
        kind = risk.get("type") or risk.get("kind")
        risk_rule["type"] = kind
        if entry_price is None:
            risk_rule["calculation_status"] = "pending_simulated_entry"
        else:
            line = overlay_price_line(
                risk, entry_price=entry_price, entry_atr=atr_value(risk),
                is_risk=True)
            if line is None:
                risk_rule["calculation_status"] = "insufficient_data"
            else:
                risk_rule.update({"calculation_status": "calculated",
                                  "reference_line": _number(line)})
    else:
        risk_rule["calculation_status"] = "disabled"

    profit = params_snapshot["take_profit"]
    take_profit = {
        "source": "overlay", "name": "止盈覆盖层",
        "enabled": bool(profit.get("enabled", False)), "config": profit,
        "data_date": str(data_date),
    }
    if not take_profit["enabled"]:
        take_profit.update({
            "calculation_status": "disabled",
            "explanation": "未设置止盈，按风险规则或策略原生条件退出。",
        })
    else:
        kind = profit.get("type") or profit.get("kind")
        take_profit["type"] = kind
        if entry_price is None:
            take_profit["calculation_status"] = "pending_simulated_entry"
        else:
            line = overlay_price_line(
                profit, entry_price=entry_price, entry_atr=atr_value(profit),
                is_risk=False)
            if line is None:
                take_profit["calculation_status"] = "insufficient_data"
            else:
                take_profit.update({"calculation_status": "calculated",
                                    "reference_line": _number(line)})
    return [risk_rule], take_profit


def _single_adapter(template: str, df: pd.DataFrame, params: dict,
                    data_date: date, valid_until: date | None
                    ) -> tuple[dict, list[dict], list[dict], float | None]:
    close = df["close"]
    if template == "ma_cross":
        fast = ma(close, int(params["fast"]))
        slow = ma(close, int(params["slow"]))
        fast_value, slow_value = _number(fast.iat[-1]), _number(slow.iat[-1])
        observation = _base_observation(
            "none", data_date,
            "按快慢均线关系判断，不生成进场价格区间。", valid_until)
        observation.update({
            "calculation_status": "calculated" if fast_value is not None and slow_value is not None
            else "insufficient_data",
            "conditions": [
                {"name": "短期均线", "value": fast_value, "unit": "price",
                 "window": int(params["fast"])},
                {"name": "长期均线", "value": slow_value, "unit": "price",
                 "window": int(params["slow"])},
                {"name": "均线距离", "value": (
                    _number(fast_value / slow_value - 1, 6)
                    if fast_value is not None and slow_value not in (None, 0) else None),
                 "unit": "ratio"},
            ],
        })
        native = [{
            "source": "native", "name": "策略退出条件",
            "condition": "短期均线下穿长期均线",
            "calculation_status": observation["calculation_status"],
            "current_values": {"fast_ma": fast_value, "slow_ma": slow_value},
            "data_date": str(data_date),
            "explanation": "均线会随新收盘数据变化，不预先换算未来退出价格。",
        }]
        risk = [{"source": "native", "name": "模板原生风险",
                 "condition": "快线不再高于慢线，趋势研究假设失效",
                 "reference_line": None, "data_date": str(data_date)}]
        return observation, risk, native, None

    if template == "breakout":
        entry_n, exit_n = int(params["entry"]), int(params["exit"])
        entry_series = REGISTRY[template].entry_observation_line(df, params)
        exit_series = df["low"].shift(1).rolling(exit_n).min()
        entry_line, exit_line = _number(entry_series.iat[-1]), _number(exit_series.iat[-1])
        observation = _line_or_range(
            entry_line, params, data_date=data_date, valid_until=valid_until,
            explanation=f"过去 {entry_n} 个交易日最高价形成客观观察线。",
            basis={"type": "prior_high", "window": entry_n, "unit": "trading_day"})
        native = [{"source": "native", "name": "策略退出条件",
                   "condition": f"收盘价跌破过去 {exit_n} 个交易日最低价",
                   "reference_line": exit_line,
                   "calculation_status": "calculated" if exit_line is not None else "insufficient_data",
                   "data_date": str(data_date)}]
        risk = [{"source": "native", "name": "模板原生风险",
                 "condition": native[0]["condition"], "reference_line": exit_line,
                 "data_date": str(data_date)}]
        return observation, risk, native, None

    if template == "mean_reversion":
        rsi14 = rsi(close, 14)
        trend = ma(close, int(params["ma"]))
        rsi_value, ma_value = _number(rsi14.iat[-1]), _number(trend.iat[-1])
        observation = _base_observation(
            "none", data_date,
            "按 RSI 与长期均线的组合关系判断，不把指标组合伪造成固定价格区间。",
            valid_until)
        observation.update({
            "calculation_status": "calculated" if rsi_value is not None and ma_value is not None
            else "insufficient_data",
            "conditions": [
                {"name": "RSI 14", "value": rsi_value,
                 "threshold": params["rsi_buy"], "operator": "below"},
                {"name": "长期均线", "value": ma_value,
                 "window": int(params["ma"]), "close": _number(close.iat[-1]),
                 "operator": "close_above"},
            ],
        })
        native = [
            {"source": "native", "name": "RSI 修复退出",
             "condition": f"RSI 14 高于 {params['rsi_sell']}",
             "current_value": rsi_value, "reference_line": None,
             "data_date": str(data_date)},
            {"source": "native", "name": "长期趋势退出",
             "condition": f"收盘价跌破 {int(params['ma'])} 日均线",
             "reference_line": ma_value, "data_date": str(data_date)},
        ]
        risk = [{"source": "native", "name": "模板原生风险",
                 "condition": native[1]["condition"], "reference_line": ma_value,
                 "data_date": str(data_date)}]
        return observation, risk, native, None

    if template == "volume_breakout":
        window = int(params["window"])
        high_series = REGISTRY[template].entry_observation_line(df, params)
        low_series = df["low"].shift(1).rolling(window).min()
        atr_series = atr(df["high"], df["low"], close, 14)
        high_line, low_line = _number(high_series.iat[-1]), _number(low_series.iat[-1])
        atr_value = _number(atr_series.iat[-1])
        observation = _line_or_range(
            high_line, params, data_date=data_date, valid_until=valid_until,
            explanation=f"过去 {window} 个交易日平台上沿形成客观观察线，并结合量能条件确认。",
            basis={"type": "platform_high", "window": window,
                   "range_max": params["range_max"], "volume_multiple": params["vol_mult"]})
        # 该模板以原始入场信号收盘价为基准，并随持有期间 ATR 只收紧不放宽。
        # 直接读取模板计算结果，避免持仓计划误用当日收盘价近似。
        atr_risk = _number(
            REGISTRY[template].native_risk_line(df, params).iat[-1]
        )
        native = [
            {"source": "native", "name": "平台下沿退出",
             "condition": "收盘价跌破整理平台下沿", "reference_line": low_line,
             "data_date": str(data_date)},
            {"source": "native", "name": "模板 ATR 风险退出",
             "condition": f"收盘价跌破入场信号价下方 {params['atr_mult']} 倍 ATR",
             "reference_line": atr_risk, "atr14": atr_value,
             "basis": "native_strategy_state", "data_date": str(data_date)},
        ]
        risk = [{"source": "native", "name": item["name"],
                 "condition": item["condition"], "reference_line": item["reference_line"],
                 "data_date": str(data_date)} for item in native]
        return observation, risk, native, atr_value

    raise ValueError(f"模板 {template} 不是已支持的单标的研究计划")


def _single_fields(df: pd.DataFrame) -> dict[str, pd.Series]:
    return {
        column: pd.Series(df[column].to_numpy(), index=df.index)
        for column in df.columns
        if column != "date"
    }


def _reason_tree(expr: Expression, df: pd.DataFrame) -> dict[str, Any]:
    return build_reason_tree(expr, _single_fields(df), len(df) - 1)


def _tree_has_unavailable_value(tree: Mapping[str, Any]) -> bool:
    indicator_ops = {
        "rolling_mean", "rolling_max", "rolling_min", "rolling_std",
        "rolling_rank", "zscore", "volume_ratio",
        "ma", "rsi", "atr", "momentum", "return", "shift",
    }
    if tree.get("op") in indicator_ops and tree.get("value") is None:
        return True
    return any(
        _tree_has_unavailable_value(child)
        for child in tree.get("children", [])
        if isinstance(child, Mapping)
    )


def _entry_reference(
    expr: Expression,
    tree: Mapping[str, Any],
) -> tuple[float | None, dict[str, Any] | None]:
    """从通用比较树中寻找“收盘价 vs 客观数值表达式”的观察线。"""
    children = tree.get("children") or []
    if expr.op in {"gt", "gte", "cross_above"} and len(children) == 2:
        pairs = ((expr.left, expr.right, 1), (expr.right, expr.left, 0))
        for close_expr, reference_expr, tree_index in pairs:
            if (
                close_expr is not None
                and close_expr.op == "field"
                and close_expr.name == "close"
                and reference_expr is not None
                and reference_expr.op != "literal"
                and _contains_shifted_channel(reference_expr)
            ):
                value = children[tree_index].get("value")
                if isinstance(value, (int, float)) and math.isfinite(float(value)):
                    return float(value), reference_expr.model_dump(mode="json")
    expression_children = [
        child for child in (
            expr.arg, expr.left, expr.right, expr.input,
            expr.high, expr.low, expr.close,
        ) if child is not None
    ] + list(expr.args or [])
    for child_expr, child_tree in zip(expression_children, children):
        result = _entry_reference(child_expr, child_tree)
        if result[0] is not None:
            return result
    return None, None


def _contains_shifted_channel(expr: Expression) -> bool:
    if expr.op in {"rolling_max", "rolling_min"} and (expr.shift or 0) > 0:
        return True
    children = [
        child for child in (
            expr.arg, expr.left, expr.right, expr.input,
            expr.high, expr.low, expr.close,
        ) if child is not None
    ] + list(expr.args or [])
    return any(_contains_shifted_channel(child) for child in children)


def evaluate_single_spec_condition(
    spec: StrategySpec | dict[str, Any],
    df: pd.DataFrame,
    signal_type: str,
) -> dict[str, Any]:
    """按完整规格重算最新确认日的入场条件，不读取模板名。"""
    from ..strategy.spec import parse_strategy_spec

    parsed = parse_strategy_spec(spec)
    if parsed.kind != "single":
        raise ValueError("只有 single StrategySpec 可以重算单标的条件")
    if df.empty:
        return {
            "satisfied": None,
            "text": "没有可用于重算的已确认日线数据。",
            "reason_tree": None,
        }
    compile_single(parsed, df)
    tree = _reason_tree(parsed.entry.condition, df)
    if _tree_has_unavailable_value(tree):
        return {
            "satisfied": None,
            "text": "规格所需指标或历史窗口不足，无法重算入场条件。",
            "reason_tree": tree,
        }
    # 计划尚未在真实世界自动执行；重评关注的是最新收盘下入场表达式是否仍
    # 成立，不能把编译器历史模拟中已经进入持仓误当成当前条件仍成立。
    # watch 计划额外接受「临近触发」:入场条件不再成立时,用通用临近判定
    # (strategy/watch.py)重算,距离超出观察容差则正常判失效。
    satisfied = bool(tree.get("value"))
    watch_assessment: dict[str, Any] | None = None
    if signal_type == "watch" and not satisfied:
        watch_assessment = assess_entry_watch(parsed.entry.condition, df)
        satisfied = watch_assessment["near"]
    if signal_type == "watch" and not satisfied:
        text = (
            f"入场规则 {parsed.entry.reason_code} 当前不再成立，且与触发条件的"
            "距离已超出临近观察容差。"
        )
    else:
        text = (
            f"入场规则 {parsed.entry.reason_code} 当前不再成立。"
            if not satisfied else f"入场规则 {parsed.entry.reason_code} 仍可计算。"
        )
    result: dict[str, Any] = {
        "satisfied": satisfied,
        "text": text,
        "reason_tree": tree,
    }
    if watch_assessment is not None:
        result["watch"] = watch_assessment
    return result


def _apply_compiler_overlay_state(
    overlay_risk: list[dict],
    take_profit: dict,
    rules: list[dict] | None,
) -> None:
    for state_rule in rules or []:
        if not isinstance(state_rule, dict):
            continue
        source = state_rule.get("source") or state_rule.get("code")
        target = (
            overlay_risk[0] if source == "risk_overlay"
            else take_profit if source == "take_profit" else None
        )
        if target is None:
            continue
        line = state_rule.get("price_line", state_rule.get("reference_line"))
        if isinstance(line, (int, float)) and math.isfinite(float(line)):
            target.update({
                "calculation_status": "calculated",
                "reference_line": _number(line, 6),
            })
        for key in ("simulated_entry_price", "data_date", "explanation"):
            if key in state_rule:
                target[key] = deepcopy(state_rule[key])


def build_single_snapshot(strategy: Any, df: pd.DataFrame, *, side: str,
                          data_date: date, next_execution_date: date | None,
                          entry_price: float | None = None,
                          exit_hits: list[dict] | None = None,
                          overlay_state_rules: list[dict] | None = None,
                          compilation: SingleCompilation | None = None) -> dict:
    """生成单标的研究计划快照;compilation=None 时由本函数自行编译。"""
    spec = strategy_spec_for(strategy)
    if spec.kind != "single":
        raise ValueError("组合策略不能生成单标的研究计划")
    if df.empty or df["date"].iat[-1] != data_date:
        raise ValueError("行情未更新到计划数据基准日")

    compilation = compilation if compilation is not None else compile_single(spec, df)
    execution = build_execution_snapshot(
        strategy,
        compiler_version=COMPILER_VERSION,
        component_versions=compilation.component_versions,
        spec_override=spec,
    )
    params_snapshot = parameter_snapshot(strategy)
    entry_tree = _reason_tree(spec.entry.condition, df)
    entry_line, entry_basis = _entry_reference(spec.entry.condition, entry_tree)
    if entry_basis is None:
        observation = _base_observation(
            "none", data_date,
            "按 StrategySpec 入场表达式判断，不生成固定价格区间。",
            next_execution_date,
        )
        observation.update({
            "calculation_status": (
                "insufficient_data" if _tree_has_unavailable_value(entry_tree)
                else "calculated"
            ),
        })
    else:
        observation = _line_or_range(
            entry_line,
            {"max_entry_premium": spec.execution.max_entry_premium},
            data_date=data_date,
            explanation="StrategySpec 入场比较表达式形成客观观察线。",
            valid_until=next_execution_date,
            basis={"type": "strategy_spec_expression", "expression": entry_basis},
        )
    observation.update({
        "reason_code": spec.entry.reason_code,
        "reason_tree": entry_tree,
        "compiler_version": execution.compiler_version,
        "component_versions": deepcopy(execution.component_versions),
    })

    assert spec.native_exit is not None
    native_tree = _reason_tree(spec.native_exit.condition, df)
    native_exit = [{
        "source": "native",
        "name": "StrategySpec 原生退出条件",
        "reason_code": spec.native_exit.reason_code,
        "condition": spec.native_exit.condition.model_dump(mode="json"),
        "reason_tree": native_tree,
        "triggered": bool(native_tree.get("value")),
        "data_date": str(data_date),
    }]
    native_risk = [{
        "source": "native",
        "name": "StrategySpec 原生风险条件",
        "reason_code": spec.native_exit.reason_code,
        "condition": spec.native_exit.condition.model_dump(mode="json"),
        "reason_tree": native_tree,
        "data_date": str(data_date),
    }]
    overlay_risk, take_profit = _overlay_rules(
        params_snapshot, entry_price=entry_price, df=df, data_date=data_date)
    compiler_reasons = compilation.reasons.get(pd.Timestamp(data_date), [])
    all_overlay_rules = list(overlay_state_rules or []) + list(compiler_reasons)
    exit_hits = list(exit_hits or [])
    if not exit_hits:
        exit_hits = [
            deepcopy(item) for item in compiler_reasons
            if isinstance(item, dict)
            and item.get("code") in {"risk_overlay", "take_profit", "native_exit"}
        ]
    _apply_compiler_overlay_state(
        overlay_risk, take_profit, all_overlay_rules + exit_hits)

    insufficient = observation.get("calculation_status") == "insufficient_data"
    status = "exit_triggered" if side == "sell" else (
        "reevaluate" if insufficient or next_execution_date is None else "current")
    reason = {
        "code": (
            "exit_condition_met" if status == "exit_triggered"
            else "insufficient_data" if insufficient
            else "next_execution_date_unknown" if next_execution_date is None
            else "fresh_daily_signal"
        ),
        "text": (
            "当日收盘已满足退出条件，最早按下一交易日开盘进行模拟退出。"
            if status == "exit_triggered"
            else "指标或历史窗口不足，需要补齐数据后重新评估。"
            if insufficient
            else "无法确定下一交易日，需要重新评估。"
            if next_execution_date is None
            else "使用当日已确认收盘数据生成，有效至下一交易日。"
        ),
        "compiler_reasons": deepcopy(compiler_reasons),
    }
    return {
        "params_snapshot": params_snapshot,
        "strategy_version": execution.spec_hash,
        "strategy_spec_snapshot": deepcopy(execution.spec_snapshot),
        "strategy_spec_hash": execution.spec_hash,
        "compiler_version": execution.compiler_version,
        "component_versions": deepcopy(execution.component_versions),
        "plan_type": "single", "data_date": data_date,
        "next_execution_date": next_execution_date,
        "valid_until": next_execution_date, "signal_type": side,
        "status": status, "status_reason": reason,
        "entry_observation": observation,
        "risk_rules": native_risk + overlay_risk,
        "take_profit": take_profit, "native_exit": native_exit,
        "exit_hits": exit_hits, "portfolio_summary": None,
    }


def portfolio_scores(template: str, pool_dfs: Mapping[str, pd.DataFrame],
                     data_date: date, params: Mapping[str, Any]) -> dict[str, float | None]:
    """按组合模板同一公式计算基准日分数，供逐股原因快照使用。"""
    details = portfolio_score_details(template, pool_dfs, data_date, params)
    return {code: item.get("total") for code, item in details.items()}


def portfolio_score_details(
    template: str,
    pool_dfs: Mapping[str, pd.DataFrame],
    data_date: date,
    params: Mapping[str, Any],
) -> dict[str, dict]:
    """保存组合评分的因子值、权重和贡献，避免只留下不可解释的总分。"""
    details: dict[str, dict] = {}
    for code, frame in pool_dfs.items():
        history = frame.loc[frame["date"] <= data_date].sort_values("date")
        if history.empty:
            details[code] = {"calculation_status": "insufficient_data", "total": None}
            continue
        close = history["close"].astype(float)
        if template not in {"momentum_rotation", "multifactor_hold"}:
            raise ValueError(f"模板 {template} 不是组合研究计划模板")
        if len(close) < 61:
            details[code] = {"calculation_status": "insufficient_data", "total": None}
            continue
        ma20 = close.rolling(20).mean()
        values = {
            "mom20": float(close.iat[-1] / close.iat[-21] - 1),
            "mom60": float(close.iat[-1] / close.iat[-61] - 1),
        }
        weights = (
            {"mom20": float(params["w_mom20"]),
             "mom60": float(params["w_mom60"])}
            if template == "momentum_rotation" else dict(SCORE_WEIGHTS)
        )
        if template == "multifactor_hold":
            values["ma20_slope"] = float(ma20.iat[-1] / ma20.iat[-6] - 1)
        names = {
            "mom20": "20日动量", "mom60": "60日动量",
            "ma20_slope": "20日均线斜率",
        }
        factors = {
            key: {
                "name": names[key],
                "value": _number(value, 8),
                "weight": _number(float(weights[key]), 8),
                "contribution": _number(value * float(weights[key]), 8),
            }
            for key, value in values.items()
        }
        details[code] = {
            "calculation_status": "calculated",
            "factors": factors,
            "total": _number(sum(
                float(item["contribution"]) for item in factors.values()
            ), 8),
            "data_date": str(data_date),
        }
    return details


def portfolio_change_type(previous: float, target: float) -> str:
    epsilon = 1e-10
    if previous <= epsilon and target > epsilon:
        return "added"
    if previous > epsilon and target <= epsilon:
        return "removed"
    if target > previous + epsilon:
        return "increased"
    if target < previous - epsilon:
        return "reduced"
    return "retained"


def portfolio_spec_score_details(
    spec: StrategySpec | dict[str, Any],
    dates: pd.DatetimeIndex,
    pool_dfs: Mapping[str, pd.DataFrame],
    data_date: date,
) -> dict[str, dict[str, Any]]:
    """从通用 score 表达式生成逐股数值和贡献树。"""
    from ..strategy.spec import parse_strategy_spec

    parsed = parse_strategy_spec(spec)
    if not isinstance(parsed.positioning, PortfolioPositioningSpec):
        raise ValueError("只有 portfolio StrategySpec 包含横截面评分")
    common = set.intersection(*(set(frame.columns) for frame in pool_dfs.values()))
    common.discard("date")
    fields = {
        field: pd.DataFrame({
            code: pd.Series(
                frame[field].to_numpy(),
                index=pd.DatetimeIndex(frame["date"]),
            )
            for code, frame in pool_dfs.items()
        }).reindex(dates)
        for field in common
    }
    scores = evaluate_expression(parsed.positioning.score, fields)
    if not isinstance(scores, pd.DataFrame):
        scores = pd.DataFrame(
            float(scores), index=dates, columns=list(pool_dfs),
        )
    risk_values = (
        evaluate_expression(parsed.positioning.risk_filter, fields)
        if parsed.positioning.risk_filter is not None else None
    )
    position = len(dates) - 1
    result: dict[str, dict[str, Any]] = {}
    for code in pool_dfs:
        value = scores.at[pd.Timestamp(data_date), code]
        total = _number(value, 8)
        item: dict[str, Any] = {
            "calculation_status": (
                "calculated" if total is not None else "insufficient_data"
            ),
            "total": total,
            "reason_tree": build_reason_tree(
                parsed.positioning.score, fields, position, column=code,
            ),
            "data_date": str(data_date),
        }
        if parsed.positioning.risk_filter is not None:
            item["risk_filter_tree"] = build_reason_tree(
                parsed.positioning.risk_filter, fields, position, column=code,
            )
            if isinstance(risk_values, pd.DataFrame):
                item["risk_blocked"] = bool(
                    risk_values.at[pd.Timestamp(data_date), code]
                )
        result[code] = item
    return result


def is_portfolio_rebalance_day(
    spec: StrategySpec | dict[str, Any],
    dates: pd.DatetimeIndex,
) -> bool:
    """按通用调仓规格判断最后一个交易日是否为计划调仓日。"""
    from ..strategy.spec import parse_strategy_spec

    parsed = parse_strategy_spec(spec)
    if not isinstance(parsed.positioning, PortfolioPositioningSpec):
        raise ValueError("只有 portfolio StrategySpec 包含调仓频率")
    rebalance = parsed.positioning.rebalance
    if rebalance.frequency == "fixed":
        assert rebalance.interval_days is not None
        return (len(dates) - 1) % rebalance.interval_days == 0
    current = dates[-1]
    if len(dates) == 1:
        return True
    previous = dates[-2]
    if rebalance.frequency == "weekly":
        return current.isocalendar()[:2] != previous.isocalendar()[:2]
    return (current.year, current.month) != (previous.year, previous.month)


def build_portfolio_snapshot(strategy: Any, *, data_date: date,
                             next_execution_date: date | None,
                             pool_name: str, target_weights: Mapping[str, float],
                             previous_weights: Mapping[str, float] | None = None,
                             scores: Mapping[str, float | None] | None = None,
                             eligible: Mapping[str, bool] | None = None,
                             risk_lines: Mapping[str, float | None] | None = None,
                             overlay_snapshots: Mapping[str, dict] | None = None,
                             exit_reasons: Mapping[str, list[dict]] | None = None,
                             score_details: Mapping[str, dict] | None = None,
                             compiler_reasons: Mapping[str, list[dict]] | None = None,
                             component_versions: Mapping[str, str] | None = None,
                             ) -> tuple[dict, list[dict]]:
    spec = strategy_spec_for(strategy)
    if not isinstance(spec.positioning, PortfolioPositioningSpec):
        raise ValueError("单标的策略不能生成组合研究计划")
    versions = dict(component_versions or component_versions_for_spec(spec))
    execution = build_execution_snapshot(
        strategy,
        compiler_version=COMPILER_VERSION,
        component_versions=versions,
        spec_override=spec,
    )
    params_snapshot = parameter_snapshot(strategy)
    previous_weights = previous_weights or {}
    scores = scores or {}
    eligible = eligible or {}
    risk_lines = risk_lines or {}
    overlay_snapshots = overlay_snapshots or {}
    exit_reasons = exit_reasons or {}
    score_details = score_details or {}
    compiler_reasons = compiler_reasons or {}
    all_codes = sorted(set(previous_weights) | set(target_weights))
    ranked = sorted(
        ((code, value) for code, value in scores.items() if value is not None),
        key=lambda item: (-float(item[1]), item[0]))
    ranks = {code: index + 1 for index, (code, _) in enumerate(ranked)}
    items = []
    for code in all_codes:
        previous = float(previous_weights.get(code, 0.0))
        target = float(target_weights.get(code, 0.0))
        is_eligible = bool(eligible.get(code, True))
        change = portfolio_change_type(previous, target)
        raw_reasons = list(compiler_reasons.get(code) or exit_reasons.get(code) or [])
        reasons: list[dict] = []
        if not is_eligible:
            reason_code = "ineligible"
            reason_text = "不满足基准日股票池或 StrategySpec 入场资格。"
        elif any(
            isinstance(item, dict) and item.get("risk_blocked")
            for item in raw_reasons
        ):
            reason_code = "risk_filter_triggered"
            reason_text = "StrategySpec 风险过滤条件成立，目标权重清零。"
        elif any(
            isinstance(item, dict) and item.get("native_exit")
            for item in raw_reasons
        ):
            reason_code = "native_exit_triggered"
            reason_text = "StrategySpec 原生退出条件成立，目标权重清零。"
        elif any(
            isinstance(item, dict) and item.get("overlay_blocked")
            for item in raw_reasons
        ):
            reason_code = "overlay_triggered"
            reason_text = "StrategySpec 覆盖层条件成立，目标权重清零。"
        elif change == "added":
            reason_code = "entered_selection"
            reason_text = "基准日评分进入 StrategySpec 目标选择范围。"
        elif change == "removed":
            reason_code = "left_selection"
            reason_text = "基准日评分或资格不再满足 StrategySpec 持有范围。"
        elif change == "retained":
            reason_code = "retained_selection"
            reason_text = "仍在 StrategySpec 目标选择范围内，权重保持。"
        else:
            reason_code = "weight_changed"
            reason_text = "StrategySpec 评分、约束或资格变化导致权重调整。"
        reasons.append({
            "code": reason_code,
            "text": reason_text,
            "compiler_reasons": deepcopy(raw_reasons),
        })
        detail = score_details.get(code) or {}
        risk_tree = detail.get("risk_filter_tree")
        native_risk_snapshot = None
        if spec.positioning.risk_filter is not None:
            native_risk_snapshot = {
                "source": "native_risk",
                "name": "StrategySpec 风险过滤",
                "condition": spec.positioning.risk_filter.model_dump(mode="json"),
                "reason_tree": deepcopy(risk_tree),
                "triggered": bool(detail.get("risk_blocked", False)),
                "reference_line": risk_lines.get(code),
                "data_date": str(data_date),
            }
        overlay_snapshot = overlay_snapshots.get(code) or {}
        snapshot_rules = ([native_risk_snapshot] if native_risk_snapshot else [])
        snapshot_rules += list(overlay_snapshot.get("rules") or [])
        for raw in raw_reasons:
            if not isinstance(raw, dict):
                continue
            for hit in raw.get("all_reasons") or []:
                if not isinstance(hit, dict) or hit.get("code") not in {
                    "risk_overlay", "take_profit"
                }:
                    continue
                snapshot_rules.append({
                    "source": hit["code"],
                    "name": (
                        "风险覆盖层" if hit["code"] == "risk_overlay"
                        else "止盈覆盖层"
                    ),
                    "reference_line": hit.get("price_line"),
                    "data_date": str(data_date),
                    "calculation_status": "calculated",
                })
        risk_snapshot = None
        if snapshot_rules:
            risk_snapshot = {
                "data_date": str(data_date),
                "rules": snapshot_rules,
            }
            # 保留原生风险的顶层字段，兼容只认识单条风险线的历史客户端。
            primary = native_risk_snapshot or snapshot_rules[0]
            risk_snapshot.update({
                "name": primary["name"],
                "reference_line": primary.get("reference_line"),
            })
        items.append({
            "code": code, "previous_weight": previous, "target_weight": target,
            "change_type": change, "score": scores.get(code),
            "score_details": score_details.get(code),
            "rank": ranks.get(code), "eligible": is_eligible,
            "reasons": reasons,
            "risk_snapshot": risk_snapshot,
        })

    active = [item for item in items if item["target_weight"] > 0]
    total_weight = sum(item["target_weight"] for item in active)
    insufficient = not scores or all(value is None for value in scores.values())
    status = "reevaluate" if insufficient or next_execution_date is None else "current"
    frequency_names = {"fixed": "固定周期", "weekly": "每周", "monthly": "每月"}
    frequency = frequency_names[spec.positioning.rebalance.frequency]
    observation = _base_observation(
        "portfolio_rebalance", data_date,
        f"{frequency}按 StrategySpec 评分表达式生成目标权重，不生成单股进场价格区间。",
        None)
    observation.update({
        "calculation_status": "insufficient_data" if insufficient else "calculated",
        "pool_name": pool_name,
        "top_n": spec.positioning.selection.n,
        "score_expression": spec.positioning.score.model_dump(mode="json"),
        "compiler_version": execution.compiler_version,
        "component_versions": deepcopy(execution.component_versions),
    })
    overlay_risk, take_profit = _overlay_rules(
        params_snapshot, entry_price=None, df=None, data_date=data_date)
    native_exit = []
    if spec.native_exit is not None:
        native_exit.append({
            "source": "native",
            "name": "StrategySpec 原生退出条件",
            "reason_code": spec.native_exit.reason_code,
            "condition": spec.native_exit.condition.model_dump(mode="json"),
            "data_date": str(data_date),
        })
    if spec.positioning.risk_filter is not None:
        native_exit.append({
            "source": "native",
            "name": "StrategySpec 风险过滤条件",
            "condition": spec.positioning.risk_filter.model_dump(mode="json"),
            "data_date": str(data_date),
        })
    native_exit.append({
        "source": "native",
        "name": "StrategySpec 组合选择条件",
        "condition": {
            "selection": spec.positioning.selection.model_dump(mode="json"),
            "rebalance": spec.positioning.rebalance.model_dump(mode="json"),
        },
        "data_date": str(data_date),
    })
    snapshot = {
        "params_snapshot": params_snapshot,
        "strategy_version": execution.spec_hash,
        "strategy_spec_snapshot": deepcopy(execution.spec_snapshot),
        "strategy_spec_hash": execution.spec_hash,
        "compiler_version": execution.compiler_version,
        "component_versions": deepcopy(execution.component_versions),
        "plan_type": "portfolio_rebalance", "data_date": data_date,
        "next_execution_date": next_execution_date, "valid_until": None,
        "signal_type": "rebalance", "status": status,
        "status_reason": {
            "code": "insufficient_data" if insufficient else (
                "next_execution_date_unknown" if next_execution_date is None else "rebalance_ready"),
            "text": "评分历史数据不足，需要重新评估。" if insufficient else (
                "无法确定下一交易日，需要重新评估。" if next_execution_date is None
                else "基准日目标权重已生成，等待下一交易日开盘进行模拟调仓。"),
        },
        "entry_observation": observation,
        "risk_rules": ([{
            "source": "native",
            "name": "StrategySpec 原生风险规则",
            "condition": item["condition"],
            "data_date": str(data_date),
        } for item in native_exit] + overlay_risk),
        "take_profit": take_profit, "native_exit": native_exit,
        "exit_hits": [],
        "portfolio_summary": {
            "pool_name": pool_name, "frequency": frequency,
            "target_count": len(active), "target_weight": round(total_weight, 8),
            "cash_weight": round(max(0.0, 1.0 - total_weight), 8),
            "changes": {
                key: sum(item["change_type"] == key for item in items)
                for key in ("added", "retained", "increased", "reduced", "removed")
            },
        },
    }
    return snapshot, items
