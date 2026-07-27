"""研究计划的纯计算层。

这里把六种模板适配为统一信息模型，但不改变模板的持仓算法。所有价格参考都
必须能由当日及此前数据客观算出；缺数据或依赖未来模拟成交价时明确记录计算
状态，不用空上下界伪装成价格区间。
"""
from __future__ import annotations

import hashlib
import json
import math
from copy import deepcopy
from datetime import date
from pathlib import Path
from typing import Any, Mapping

import pandas as pd

from ..indicators import atr, ma, rsi
from ..catalog import STRATEGY_TEMPLATES
from ..selection.pipeline import SCORE_WEIGHTS
from ..strategy.overlays import overlay_price_line
from ..strategy.strategies import REGISTRY

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
    """固化原始实例参数、完整模板参数和统一覆盖层配置。"""
    # 延迟导入避免 catalog/策略 API 初始化时形成模块环；验证与回测、信号共用
    # 同一入口，不能由计划层近似复刻一份规则。
    from ..backtest.engine import DEFAULT_COSTS, validate_strategy_params

    supplied = dict(strategy.params or {})
    effective = validate_strategy_params(strategy.template, supplied)
    risk = dict(effective["risk_overlay"])
    profit = dict(effective["take_profit"])
    return {
        "strategy_params": supplied,
        "effective_params": effective,
        "risk_overlay": risk,
        "take_profit": profit,
        "simulation_costs": deepcopy(DEFAULT_COSTS),
    }


def strategy_version(strategy: Any, params_snapshot: dict) -> str:
    """内容寻址版本：参数或实际算法源码变化都会得到新版本。"""
    implementation = hashlib.sha256()
    files = [
        Path(__file__),
        Path(__file__).parents[1] / "strategy" / "overlays.py",
        Path(REGISTRY[strategy.template].__file__),
    ]
    for path in files:
        implementation.update(path.read_bytes())
    payload = {
        "adapter_version": ADAPTER_VERSION,
        "implementation": implementation.hexdigest(),
        "strategy_id": strategy.id,
        "template": strategy.template,
        "params": params_snapshot,
    }
    canonical = json.dumps(payload, ensure_ascii=True, sort_keys=True,
                           separators=(",", ":"), default=str)
    return f"rp{ADAPTER_VERSION}-{hashlib.sha256(canonical.encode()).hexdigest()[:24]}"


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


def build_single_snapshot(strategy: Any, df: pd.DataFrame, *, side: str,
                          data_date: date, next_execution_date: date | None,
                          entry_price: float | None = None,
                          exit_hits: list[dict] | None = None,
                          overlay_state_rules: list[dict] | None = None) -> dict:
    if strategy.template not in CAPABILITIES or strategy.kind != "single":
        raise ValueError(f"模板 {strategy.template} 不是单标的研究计划模板")
    if df.empty or df["date"].iat[-1] != data_date:
        raise ValueError("行情未更新到计划数据基准日")

    params_snapshot = parameter_snapshot(strategy)
    params = _native_params(strategy.template, params_snapshot["effective_params"])
    observation, native_risk, native_exit, atr_value = _single_adapter(
        strategy.template, df, params, data_date, next_execution_date)
    overlay_risk, take_profit = _overlay_rules(
        params_snapshot, entry_price=entry_price, df=df, data_date=data_date)
    # 状态机保存的是模拟入场时 ATR/价格口径下的真实覆盖线，优先于计划层按
    # 最新 ATR 能做出的静态估算。固定比例覆盖层也走同一路径，保持单一口径。
    for state_rule in overlay_state_rules or []:
        if not isinstance(state_rule, dict):
            continue
        source = state_rule.get("source")
        target = (
            overlay_risk[0] if source == "risk_overlay"
            else take_profit if source == "take_profit" else None
        )
        if target is None:
            continue
        target.pop("reference_line", None)
        for key in (
            "calculation_status", "reference_line", "simulated_entry_price",
            "data_date", "explanation",
        ):
            if key in state_rule:
                target[key] = deepcopy(state_rule[key])
    exit_hits = exit_hits or []
    # 覆盖层状态机已把实际命中线固化在退出原因中。退出计划必须复用这条线，
    # 不能一边显示“已触发”，一边仍称“等待模拟入场价后计算”。
    for hit in exit_hits:
        line = hit.get("price_line") if isinstance(hit, dict) else None
        if not isinstance(line, (int, float)) or not math.isfinite(float(line)):
            continue
        if hit.get("code") == "risk_overlay":
            overlay_risk[0].update({
                "calculation_status": "calculated",
                "reference_line": _number(line, 6),
            })
        elif hit.get("code") == "take_profit":
            take_profit.update({
                "calculation_status": "calculated",
                "reference_line": _number(line, 6),
            })
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
    }
    return {
        "params_snapshot": params_snapshot,
        "strategy_version": strategy_version(strategy, params_snapshot),
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
                             ) -> tuple[dict, list[dict]]:
    if strategy.template not in CAPABILITIES or strategy.kind != "portfolio":
        raise ValueError(f"模板 {strategy.template} 不是组合研究计划模板")
    params_snapshot = parameter_snapshot(strategy)
    params = _native_params(strategy.template, params_snapshot["effective_params"])
    previous_weights = previous_weights or {}
    scores = scores or {}
    eligible = eligible or {}
    risk_lines = risk_lines or {}
    overlay_snapshots = overlay_snapshots or {}
    exit_reasons = exit_reasons or {}
    score_details = score_details or {}
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
        reasons: list[dict] = []
        if not is_eligible:
            reasons.append({"code": "ineligible", "text": "不满足基准日股票池资格。"})
        elif any(
            item.get("code") == "native"
            for item in exit_reasons.get(code, [])
            if isinstance(item, dict)
        ):
            reasons.append({"code": "ma20_risk_filter",
                            "text": "收盘价跌破20日均线，目标权重按模板风险过滤清零。"})
        elif change == "added":
            reasons.append({"code": "entered_top_n", "text": "基准日评分进入 Top N。"})
        elif change == "removed":
            reasons.append({"code": "left_top_n", "text": "基准日评分或资格不再满足持有范围。"})
        elif change == "retained":
            reasons.append({"code": "retained_top_n", "text": "仍在目标排名范围内，权重保持。"})
        else:
            reasons.append({"code": "weight_changed", "text": "目标数量或资格变化导致权重调整。"})
        native_risk_snapshot = (
            {"source": "native_risk", "name": "20日均线资格过滤",
             "reference_line": risk_lines.get(code),
             "data_date": str(data_date)}
            if strategy.template == "momentum_rotation" else None)
        overlay_snapshot = overlay_snapshots.get(code) or {}
        snapshot_rules = ([native_risk_snapshot] if native_risk_snapshot else [])
        snapshot_rules += list(overlay_snapshot.get("rules") or [])
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
    frequency = "每周" if strategy.template == "momentum_rotation" else "每月"
    observation = _base_observation(
        "portfolio_rebalance", data_date,
        f"{frequency}按评分排名生成目标权重，不生成单股进场价格区间。", None)
    observation.update({
        "calculation_status": "insufficient_data" if insufficient else "calculated",
        "pool_name": pool_name, "top_n": int(params["top_n"]),
    })
    overlay_risk, take_profit = _overlay_rules(
        params_snapshot, entry_price=None, df=None, data_date=data_date)
    native_exit = ([{
        "source": "native", "name": "20日均线资格过滤",
        "condition": "收盘价跌破20日均线时目标权重清零",
        "data_date": str(data_date),
    }] if strategy.template == "momentum_rotation" else [{
        "source": "native", "name": "月度排名变化",
        "condition": "月度调仓时不再位于 Top N 或失去股票池资格",
        "data_date": str(data_date),
    }])
    snapshot = {
        "params_snapshot": params_snapshot,
        "strategy_version": strategy_version(strategy, params_snapshot),
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
        "risk_rules": ([{"source": "native", "name": "模板原生风险",
                          "condition": native_exit[0]["condition"],
                          "data_date": str(data_date)}] + overlay_risk),
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
