"""六个系统策略的完整 StrategySpec 种子及 legacy 参数适配器。

这些构造器只负责把迁移期 ``template + params`` 转成数据库规格。编译器从不读取
模板名，新增普通策略也不需要修改本文件。
"""
from __future__ import annotations

from copy import deepcopy
from typing import Any, Callable

from .spec import StrategySpec, parse_strategy_spec


def _field(name: str) -> dict[str, Any]:
    return {"op": "field", "name": name}


def _literal(value: bool | int | float) -> dict[str, Any]:
    return {"op": "literal", "value": value}


def _binary(op: str, left: dict, right: dict) -> dict[str, Any]:
    return {"op": op, "left": left, "right": right}


def _window(op: str, source: dict, window: int, shift: int) -> dict[str, Any]:
    return {"op": op, "input": source, "window": window, "shift": shift}


def _indicator(op: str, source: dict, window: int) -> dict[str, Any]:
    return {"op": op, "input": source, "window": window}


def _disabled_overlays() -> dict[str, Any]:
    return {
        "risk": {
            "enabled": False, "type": "fixed_pct", "value": 0.08,
            "atr_period": 14, "trailing": False,
        },
        "take_profit": {
            "enabled": False, "type": "fixed_pct", "value": 0.20,
            "atr_period": 14, "trailing": False,
        },
    }


def _apply_legacy_overlays(overlays: dict[str, Any], params: dict[str, Any]) -> None:
    mapping = {"risk_overlay": "risk", "take_profit": "take_profit"}
    for legacy, current in mapping.items():
        if legacy not in params:
            continue
        value = params[legacy]
        if not isinstance(value, dict):
            raise ValueError(f"{legacy} 必须是对象")
        unknown = set(value) - {"enabled", "type", "value", "atr_period"}
        if unknown:
            raise ValueError(f"{legacy} 包含未知字段: {sorted(unknown)}")
        overlays[current].update(value)


def _validate_params(params: dict[str, Any], allowed: set[str]) -> None:
    unknown = set(params) - allowed - {"risk_overlay", "take_profit"}
    if unknown:
        raise ValueError(f"策略参数包含未知字段: {sorted(unknown)}")


def _finalize_spec(spec: dict[str, Any]) -> StrategySpec:
    declared = {item["field"] for item in spec["data_requirements"]}
    required: set[str] = set()
    for overlay in spec["overlays"].values():
        if not overlay["enabled"]:
            continue
        required.add("close")
        if overlay["type"] == "atr_multiple":
            required.update({"high", "low"})
    for field in sorted(required - declared):
        spec["data_requirements"].append({
            "field": field,
            "availability": "daily_close",
            "required": True,
        })
    return parse_strategy_spec(spec)


def _base(
    *,
    kind: str,
    canonical_id: str,
    book: str,
    candidate_id: str,
    hypothesis: str,
    data_fields: list[str],
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": kind,
        "metadata": {
            "canonical_id": canonical_id,
            "sources": [{"book": book, "candidate_id": candidate_id}],
            "evidence_status": "unverified",
            "hypothesis": hypothesis,
        },
        "universe": {
            "pool_id": 2,
            "exclude_st": True,
            "min_listing_days": 60,
            "min_amount_avg20": 0.0,
        },
        "data_requirements": [
            {"field": field, "availability": "daily_close", "required": True}
            for field in data_fields
        ],
        "holding": {
            "allow_add": False,
            "allow_reduce": False,
            "cooldown_days": 0,
            "risk_reentry": "native_reset",
        },
        "overlays": _disabled_overlays(),
        "portfolio_constraints": {
            "long_only": True,
            "max_positions": 500,
            "max_single_weight": 1.0,
            "max_total_weight": 1.0,
        },
        "execution": {
            "signal_time": "close",
            "execution_time": "next_open",
            "buy_limit_policy": "reject",
            "sell_limit_policy": "retry",
            "suspension_policy": "reject_entry_retry_exit",
            "missing_bar_policy": "reject_entry_retry_exit",
            "cost_model": "a_share_daily_v1",
            "max_entry_premium": 0.0,
        },
        "validation": {
            "baseline_ids": ["buy_and_hold", "equal_weight"],
            "locked_oos": True,
            "rejection_criteria": [
                "no_net_oos_increment", "unstable_parameters", "capacity_failure",
            ],
            "parameter_scans": [],
        },
    }


def _ma_cross(params: dict[str, Any]) -> StrategySpec:
    _validate_params(params, {"fast", "slow"})
    p = {"fast": 5, "slow": 20, **params}
    spec = _base(
        kind="single", canonical_id="CAN-TRD-01",
        book="股市趋势技术分析", candidate_id="TREND-08",
        hypothesis="短期均价高于长期均价时，趋势延续概率可能高于简单持有基线。",
        data_fields=["close"],
    )
    fast = _indicator("ma", _field("close"), int(p["fast"]))
    slow = _indicator("ma", _field("close"), int(p["slow"]))
    spec.update({
        "entry": {"condition": _binary("gt", fast, slow), "reason_code": "fast_ma_above_slow"},
        "positioning": {"type": "binary", "target": 1.0},
        "native_exit": {
            "condition": _binary("lte", fast, slow),
            "reason_code": "fast_ma_not_above_slow",
        },
    })
    _apply_legacy_overlays(spec["overlays"], p)
    return _finalize_spec(spec)


def _breakout(params: dict[str, Any]) -> StrategySpec:
    _validate_params(params, {"entry", "exit", "max_entry_premium"})
    p = {"entry": 20, "exit": 10, "max_entry_premium": 0.0, **params}
    spec = _base(
        kind="single", canonical_id="CAN-TRD-02",
        book="股市趋势技术分析", candidate_id="TREND-03",
        hypothesis="收盘突破历史区间上沿后可能延续，跌破较短退出通道表示假设失效。",
        data_fields=["close", "high", "low"],
    )
    entry_line = _window("rolling_max", _field("high"), int(p["entry"]), 1)
    exit_line = _window("rolling_min", _field("low"), int(p["exit"]), 1)
    spec.update({
        "entry": {
            "condition": _binary("gt", _field("close"), entry_line),
            "reason_code": "close_above_prior_high",
        },
        "positioning": {"type": "binary", "target": 1.0},
        "native_exit": {
            "condition": _binary("lt", _field("close"), exit_line),
            "reason_code": "close_below_prior_low",
        },
    })
    spec["execution"]["max_entry_premium"] = float(p["max_entry_premium"])
    _apply_legacy_overlays(spec["overlays"], p)
    return _finalize_spec(spec)


def _mean_reversion(params: dict[str, Any]) -> StrategySpec:
    _validate_params(params, {"rsi_buy", "rsi_sell", "ma"})
    p = {"rsi_buy": 30, "rsi_sell": 55, "ma": 60, **params}
    spec = _base(
        kind="single", canonical_id="CAN-REV-06",
        book="量化交易从入门到精通", candidate_id="QTP-003",
        hypothesis="长期趋势向上时的短期超卖可能均值修复，修复完成或趋势失效时退出。",
        data_fields=["close"],
    )
    rsi14 = _indicator("rsi", _field("close"), 14)
    trend = _indicator("ma", _field("close"), int(p["ma"]))
    spec.update({
        "entry": {
            "condition": {"op": "all", "args": [
                _binary("lt", rsi14, _literal(float(p["rsi_buy"]))),
                _binary("gt", _field("close"), trend),
            ]},
            "reason_code": "uptrend_oversold",
        },
        "positioning": {"type": "binary", "target": 1.0},
        "native_exit": {
            "condition": {"op": "any", "args": [
                _binary("gt", rsi14, _literal(float(p["rsi_sell"]))),
                _binary("lt", _field("close"), trend),
            ]},
            "reason_code": "reversion_complete_or_trend_failed",
        },
    })
    _apply_legacy_overlays(spec["overlays"], p)
    return _finalize_spec(spec)


def _volume_breakout(params: dict[str, Any]) -> StrategySpec:
    _validate_params(
        params, {"window", "range_max", "vol_mult", "atr_mult", "max_entry_premium"},
    )
    p = {
        "window": 20, "range_max": 0.15, "vol_mult": 2.0, "atr_mult": 2.0,
        "max_entry_premium": 0.0, **params,
    }
    window = int(p["window"])
    spec = _base(
        kind="single", canonical_id="CAN-TRD-04",
        book="量化交易从入门到精通", candidate_id="QTP-002",
        hypothesis="价格和成交收缩后的放量向上突破可能形成趋势，平台下沿或 ATR 风险线失效。",
        data_fields=["close", "high", "low", "volume"],
    )
    high_line = _window("rolling_max", _field("high"), window, 1)
    low_line = _window("rolling_min", _field("low"), window, 1)
    vol5 = _window("rolling_mean", _field("volume"), 5, 1)
    voln = _window("rolling_mean", _field("volume"), window, 1)
    range_ratio = _binary(
        "divide", _binary("subtract", high_line, low_line), _field("close"),
    )
    spec.update({
        "entry": {
            "condition": {"op": "all", "args": [
                _binary("lte", range_ratio, _literal(float(p["range_max"]))),
                _binary("lt", vol5, voln),
                _binary(
                    "gt", _field("volume"),
                    _binary("multiply", _literal(float(p["vol_mult"])), voln),
                ),
                _binary("gt", _field("close"), high_line),
            ]},
            "reason_code": "contracted_volume_breakout",
        },
        "positioning": {"type": "binary", "target": 1.0},
        "native_exit": {
            "condition": _binary("lt", _field("close"), low_line),
            "reason_code": "close_below_platform_low",
        },
    })
    spec["overlays"]["risk"] = {
        "enabled": True,
        "type": "atr_multiple",
        "value": float(p["atr_mult"]),
        "atr_period": 14,
        "trailing": True,
    }
    spec["execution"]["max_entry_premium"] = float(p["max_entry_premium"])
    # legacy 的 risk_overlay 是叠加在模板内置 ATR 线之外的通用覆盖层。默认关闭
    # 不能反过来关闭模板原生 ATR 线；启用时首期规格以显式覆盖层为准。
    overlay_params = dict(p)
    if not (overlay_params.get("risk_overlay") or {}).get("enabled", False):
        overlay_params.pop("risk_overlay", None)
    _apply_legacy_overlays(spec["overlays"], overlay_params)
    return _finalize_spec(spec)


def _momentum_rotation(params: dict[str, Any]) -> StrategySpec:
    _validate_params(params, {"top_n", "w_mom20", "w_mom60"})
    p = {"top_n": 10, "w_mom20": 0.6, "w_mom60": 0.4, **params}
    spec = _base(
        kind="portfolio", canonical_id="CAN-TRD-05",
        book="股票大作手回忆录", candidate_id="LIV-04",
        hypothesis="横截面中短期动量较强的股票可能延续，每周轮动并用短均线控制趋势失效。",
        data_fields=["close"],
    )
    score = _binary("add",
        _binary("multiply", _literal(float(p["w_mom20"])),
                _indicator("momentum", _field("close"), 20)),
        _binary("multiply", _literal(float(p["w_mom60"])),
                _indicator("momentum", _field("close"), 60)),
    )
    spec.update({
        "entry": {"condition": _literal(True), "reason_code": "eligible_for_ranking"},
        "positioning": {
            "type": "portfolio",
            "score": score,
            "selection": {"type": "top_n", "n": int(p["top_n"])},
            "weighting": {"type": "equal"},
            "rebalance": {"frequency": "weekly", "interval_days": None},
            "risk_filter": _binary(
                "lt", _field("close"), _indicator("ma", _field("close"), 20),
            ),
        },
        "native_exit": None,
    })
    spec["portfolio_constraints"]["max_positions"] = int(p["top_n"])
    _apply_legacy_overlays(spec["overlays"], p)
    return _finalize_spec(spec)


def _multifactor_hold(params: dict[str, Any]) -> StrategySpec:
    _validate_params(params, {"top_n"})
    p = {"top_n": 20, **params}
    spec = _base(
        kind="portfolio", canonical_id="CAN-PORT-04",
        book="打开量化投资的黑箱", candidate_id="BLACKBOX-ALPHA-01",
        hypothesis="中短期动量与均线斜率的组合排序可能比单因子等权基线更稳定。",
        data_fields=["close"],
    )
    ma20 = _indicator("ma", _field("close"), 20)
    score = _binary("add",
        _binary("add",
            _binary("multiply", _literal(0.5),
                    _indicator("momentum", _field("close"), 20)),
            _binary("multiply", _literal(0.3),
                    _indicator("momentum", _field("close"), 60)),
        ),
        _binary("multiply", _literal(0.2), _indicator("return", ma20, 5)),
    )
    spec.update({
        "entry": {"condition": _literal(True), "reason_code": "eligible_for_ranking"},
        "positioning": {
            "type": "portfolio",
            "score": score,
            "selection": {"type": "top_n", "n": int(p["top_n"])},
            "weighting": {"type": "equal"},
            "rebalance": {"frequency": "monthly", "interval_days": None},
            "risk_filter": None,
        },
        "native_exit": None,
    })
    spec["portfolio_constraints"]["max_positions"] = int(p["top_n"])
    _apply_legacy_overlays(spec["overlays"], p)
    return _finalize_spec(spec)


_BUILDERS: dict[str, Callable[[dict[str, Any]], StrategySpec]] = {
    "ma_cross": _ma_cross,
    "breakout": _breakout,
    "mean_reversion": _mean_reversion,
    "volume_breakout": _volume_breakout,
    "momentum_rotation": _momentum_rotation,
    "multifactor_hold": _multifactor_hold,
}

SYSTEM_STRATEGY_SPECS: dict[str, dict[str, Any]] = {
    name: builder({}).model_dump(mode="json")
    for name, builder in _BUILDERS.items()
}


def get_preset_spec(
    template: str,
    params: dict[str, Any] | None = None,
) -> StrategySpec:
    """把六个 legacy 模板及参数转换成完整规格，返回独立对象。"""
    builder = _BUILDERS.get(template)
    if builder is None:
        raise ValueError(f"未知系统策略模板: {template}")
    return builder(deepcopy(params or {}))


__all__ = ["SYSTEM_STRATEGY_SPECS", "get_preset_spec"]
