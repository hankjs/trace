"""策略统一风险/止盈覆盖层。

覆盖层只消费日频收盘确认条件，模拟成交仍由回测引擎安排在下一交易日开盘。
这里保存的是信号状态和退出原因，不包含任何真实交易能力。
"""
from __future__ import annotations

import math
from copy import deepcopy
from typing import Any

import pandas as pd

from ..indicators import atr

OVERLAY_KEYS = ("risk_overlay", "take_profit")
DEFAULT_SIMULATION_SLIPPAGE = 0.0001
EXIT_REASON_PRIORITY = {
    "risk_overlay": 0,
    "take_profit": 1,
    "native": 2,
    "reduce": 3,
    "rebalance": 4,
}

DEFAULT_OVERLAYS: dict[str, dict[str, Any]] = {
    "risk_overlay": {
        "enabled": False,
        "type": "fixed_pct",
        "value": 0.08,
        "atr_period": 14,
    },
    "take_profit": {
        "enabled": False,
        "type": "fixed_pct",
        "value": 0.20,
        "atr_period": 14,
    },
}

_VALUE_LIMITS = {
    "risk_overlay": {
        "fixed_pct": (0.001, 1.0),
        "atr_multiple": (0.1, 20.0),
    },
    "take_profit": {
        "fixed_pct": (0.001, 1.0),
        "atr_multiple": (0.1, 50.0),
    },
}


def validate_overlay(name: str, value: object | None) -> dict[str, Any]:
    """严格校验并补齐一个覆盖层配置。"""
    if name not in DEFAULT_OVERLAYS:
        raise ValueError(f"未知覆盖层: {name}")
    if value is None:
        return deepcopy(DEFAULT_OVERLAYS[name])
    if not isinstance(value, dict):
        raise ValueError(f"参数 {name} 必须是对象")
    allowed = {"enabled", "type", "value", "atr_period"}
    unknown = set(value) - allowed
    if unknown:
        raise ValueError(f"参数 {name} 包含未知字段: {', '.join(sorted(unknown))}")
    result = {**DEFAULT_OVERLAYS[name], **value}
    if not isinstance(result["enabled"], bool):
        raise ValueError(f"参数 {name}.enabled 必须是布尔值")
    if result["type"] not in {"fixed_pct", "atr_multiple"}:
        raise ValueError(f"参数 {name}.type 必须是 fixed_pct 或 atr_multiple")
    number = result["value"]
    if isinstance(number, bool) or not isinstance(number, (int, float)):
        raise ValueError(f"参数 {name}.value 必须是数字")
    number = float(number)
    minimum, maximum = _VALUE_LIMITS[name][result["type"]]
    if not math.isfinite(number) or not minimum <= number <= maximum:
        unit = "小数比例" if result["type"] == "fixed_pct" else "ATR 倍数"
        raise ValueError(
            f"参数 {name}.value ({unit}) 必须在 {minimum} 到 {maximum} 之间"
        )
    period = result["atr_period"]
    if isinstance(period, bool) or not isinstance(period, (int, float)):
        raise ValueError(f"参数 {name}.atr_period 必须是整数")
    period_number = float(period)
    if not period_number.is_integer() or not 2 <= period_number <= 250:
        raise ValueError(f"参数 {name}.atr_period 必须是 2 到 250 的整数")
    result["value"] = number
    result["atr_period"] = int(period_number)
    return result


def overlay_defaults() -> dict[str, dict[str, Any]]:
    return deepcopy(DEFAULT_OVERLAYS)


def reason(code: str, *, price_line: float | None = None) -> dict[str, Any]:
    labels = {
        "risk_overlay": "风险覆盖层",
        "take_profit": "止盈覆盖层",
        "native": "策略原生退出",
        "reduce": "减仓规则触发",
        "rebalance": "组合调仓或资格变化",
    }
    item: dict[str, Any] = {"code": code, "name": labels[code]}
    if price_line is not None and math.isfinite(price_line):
        item["price_line"] = round(float(price_line), 6)
    return item


def sort_exit_reasons(reasons: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """去重并按风险、止盈、原生、调仓的固定优先级排序。"""
    unique: dict[str, dict[str, Any]] = {}
    for item in reasons:
        code = item.get("code")
        if code in EXIT_REASON_PRIORITY and code not in unique:
            unique[code] = item
    return sorted(unique.values(), key=lambda item: EXIT_REASON_PRIORITY[item["code"]])


def overlay_price_line(
    config: dict[str, Any],
    *,
    entry_price: float,
    entry_atr: float | None,
    is_risk: bool,
) -> float | None:
    """按模拟入场价计算固定比例或 ATR 覆盖线。"""
    if not config["enabled"]:
        return None
    distance: float
    if config["type"] == "fixed_pct":
        distance = entry_price * float(config["value"])
    else:
        if entry_atr is None or not math.isfinite(entry_atr):
            return None
        distance = entry_atr * float(config["value"])
    return entry_price - distance if is_risk else entry_price + distance


def single_entry_price_ceiling(
    strategy_module,
    df: pd.DataFrame,
    params: dict[str, Any],
) -> pd.Series | None:
    """把模板观察线和显式溢价参数转换为 T+1 开盘有效上界。"""
    premium = params.get("max_entry_premium", 0)
    line_factory = getattr(strategy_module, "entry_observation_line", None)
    if (
        not isinstance(premium, (int, float))
        or isinstance(premium, bool)
        or float(premium) <= 0
        or line_factory is None
    ):
        return None
    line = line_factory(df, params)
    return pd.Series(
        line.to_numpy(dtype=float) * (1 + float(premium)),
        index=pd.DatetimeIndex(df["date"]),
        dtype=float,
    )


def single_entry_price_floor(
    strategy_module,
    df: pd.DataFrame,
    params: dict[str, Any],
) -> pd.Series | None:
    """显式配置观察区间时，以模板观察线作为 T+1 开盘有效下界。"""
    premium = params.get("max_entry_premium", 0)
    line_factory = getattr(strategy_module, "entry_observation_line", None)
    if (
        not isinstance(premium, (int, float))
        or isinstance(premium, bool)
        or float(premium) <= 0
        or line_factory is None
    ):
        return None
    line = line_factory(df, params)
    return pd.Series(
        line.to_numpy(dtype=float),
        index=pd.DatetimeIndex(df["date"]),
        dtype=float,
    )


def apply_single_overlays(
    df: pd.DataFrame,
    native_positions: pd.Series,
    params: dict[str, Any],
    *,
    slippage: float = 0.0,
    entry_tradable: pd.Series | None = None,
    entry_price_floor: pd.Series | None = None,
    entry_price_ceiling: pd.Series | None = None,
    state_out: dict[str, Any] | None = None,
) -> tuple[pd.Series, dict[pd.Timestamp, list[dict[str, Any]]]]:
    """把覆盖层应用到单标的原生目标仓位。

    原生 0->1 是 T 日信号，模拟入场价取 T+1 开盘并加入买入滑点；覆盖条件从
    成交日收盘开始检查。覆盖退出后，必须观察到原生入场条件失效（原生仓位为
    0），之后再次形成 0->1 上升沿才可重新进入。

    原生仓位可以是 (0,1] 的中间档位(加仓/减仓规则):档位变化在信号日直接跟随
    原生序列,覆盖层风险线始终锚定最初的模拟入场价,不随加仓移动。

    返回覆盖后的每日目标仓位，以及以信号日为键的全部退出原因。
    """
    if len(df) != len(native_positions):
        raise ValueError("行情与原生仓位长度不一致")
    risk = validate_overlay("risk_overlay", params.get("risk_overlay"))
    take = validate_overlay("take_profit", params.get("take_profit"))
    native = pd.Series(
        native_positions.to_numpy(dtype=float),
        index=pd.DatetimeIndex(df["date"]), dtype=float,
    )
    result = pd.Series(0.0, index=native.index, dtype=float)
    tradable = (
        entry_tradable.reindex(native.index, fill_value=False).astype(bool)
        if entry_tradable is not None
        else pd.Series(True, index=native.index)
    )
    ceiling = (
        entry_price_ceiling.reindex(native.index)
        if entry_price_ceiling is not None else None
    )
    floor = (
        entry_price_floor.reindex(native.index)
        if entry_price_floor is not None else None
    )
    atr_periods = {
        config["atr_period"]
        for config in (risk, take)
        if config["enabled"] and config["type"] == "atr_multiple"
    }
    atr_values = {
        period: atr(df["high"], df["low"], df["close"], period)
        for period in atr_periods
    }

    holding = False
    level = 0.0  # 当前目标档位(跟随原生序列的中间档位)
    entry_price: float | None = None
    entry_atr_by_period: dict[int, float | None] = {}
    pending_entry = False
    pending_level = 0.0
    blocked_until_reset = False
    exit_reasons: dict[pd.Timestamp, list[dict[str, Any]]] = {}
    latest_state: dict[str, Any] = {}

    for i, day in enumerate(native.index):
        native_now = float(native.iat[i])
        native_prev = float(native.iat[i - 1]) if i else 0.0
        entry_blocked_today = False

        # 上一收盘日形成的进入信号，在今日开盘模拟成交。
        if pending_entry:
            open_price = float(df["open"].iat[i])
            ceiling_value = ceiling.iat[i - 1] if ceiling is not None and i else None
            floor_value = floor.iat[i - 1] if floor is not None and i else None
            within_range = (
                (ceiling is None or (
                    pd.notna(ceiling_value) and open_price <= float(ceiling_value)
                ))
                and (floor is None or (
                    pd.notna(floor_value) and open_price >= float(floor_value)
                ))
            )
            if (tradable.iat[i] and math.isfinite(open_price)
                    and open_price > 0 and within_range):
                holding = True
                level = pending_level
                entry_price = open_price * (1 + slippage)
                entry_atr_by_period = {}
                for period, values in atr_values.items():
                    value = values.iat[i - 1] if i else math.nan
                    entry_atr_by_period[period] = (
                        float(value) if pd.notna(value) and float(value) > 0 else None
                    )
            else:
                # 入场信号不顺延；原生条件必须先失效后再形成新上升沿。
                blocked_until_reset = native_now > 0
                entry_blocked_today = True
            pending_entry = False

        if blocked_until_reset and native_now <= 0:
            blocked_until_reset = False

        reasons: list[dict[str, Any]] = []
        day_state: dict[str, Any] = (
            {"entry_blocked": True,
             "data_date": str(pd.Timestamp(day).date())}
            if entry_blocked_today else {}
        )
        if holding and entry_price is not None:
            close = float(df["close"].iat[i])
            risk_line = overlay_price_line(
                risk,
                entry_price=entry_price,
                entry_atr=entry_atr_by_period.get(risk["atr_period"]),
                is_risk=True,
            )
            take_line = overlay_price_line(
                take,
                entry_price=entry_price,
                entry_atr=entry_atr_by_period.get(take["atr_period"]),
                is_risk=False,
            )
            rules = []
            for source, config, line, name in (
                ("risk_overlay", risk, risk_line, "风险覆盖层"),
                ("take_profit", take, take_line, "止盈覆盖层"),
            ):
                if not config["enabled"]:
                    continue
                item = {
                    "source": source, "name": name,
                    "data_date": str(pd.Timestamp(day).date()),
                    "calculation_status": (
                        "calculated" if line is not None else "insufficient_data"
                    ),
                }
                if line is not None:
                    item["reference_line"] = round(float(line), 6)
                rules.append(item)
            day_state = {
                "simulated_entry_price": round(float(entry_price), 6),
                "data_date": str(pd.Timestamp(day).date()),
                "rules": rules,
            }
            if risk_line is not None and close <= risk_line:
                reasons.append(reason("risk_overlay", price_line=risk_line))
            if take_line is not None and close >= take_line:
                reasons.append(reason("take_profit", price_line=take_line))
            if native_now <= 0:
                reasons.append(reason("native"))
            if reasons:
                ordered = sort_exit_reasons(reasons)
                exit_reasons[day] = ordered
                holding = False
                level = 0.0
                entry_price = None
                # 覆盖层退出要求重新武装；纯原生退出按模板自己的新上升沿即可。
                if any(item["code"] in {"risk_overlay", "take_profit"}
                       for item in ordered):
                    blocked_until_reset = native_now > 0
            elif abs(native_now - level) > 1e-9:
                # 加减仓信号:目标档位跟随原生序列,覆盖层风险线不随加仓移动。
                level = native_now
        elif holding and native_now <= 0:
            exit_reasons[day] = [reason("native")]
            holding = False
            level = 0.0

        # 仅接受原生上升沿(0 -> 正档位)。退出信号当天不能同时预约再次进入。
        if (not holding and not reasons and not blocked_until_reset
                and native_now > 0 and native_prev <= 0):
            pending_entry = True
            pending_level = native_now

        # 目标仓位在信号日即变化，回测执行层统一 shift 到下一开盘。
        result.iat[i] = level if holding else (pending_level if pending_entry else 0.0)
        latest_state = day_state

    if state_out is not None:
        state_out.clear()
        state_out.update(latest_state)
    return result, exit_reasons


def apply_portfolio_overlays(
    weights: pd.DataFrame,
    pool_dfs: dict[str, pd.DataFrame],
    params: dict[str, Any],
    rebalance_mask: pd.Series,
    *,
    slippage: float = 0.0,
    entry_tradable: pd.DataFrame | None = None,
    base_exit_reasons: dict[
        tuple[pd.Timestamp, str], list[dict[str, Any]]
    ] | None = None,
) -> tuple[
    pd.DataFrame,
    dict[tuple[pd.Timestamp, str], list[dict[str, Any]]],
    dict[str, dict[str, Any]],
]:
    """逐持仓应用组合覆盖层，退出后锁定到下一次计划调仓。

    调仓日可以重新获得资格，但当日收盘生成的新权重仍在 T+1 开盘成交。
    """
    risk = validate_overlay("risk_overlay", params.get("risk_overlay"))
    take = validate_overlay("take_profit", params.get("take_profit"))
    base_exit_reasons = base_exit_reasons or {}
    if not risk["enabled"] and not take["enabled"]:
        return weights.copy(), dict(base_exit_reasons), {}

    result = weights.copy().astype(float)
    tradable = (
        entry_tradable.reindex(index=result.index, columns=result.columns)
        .fillna(False).astype(bool)
        if entry_tradable is not None
        else pd.DataFrame(True, index=result.index, columns=result.columns)
    )
    rebalance = rebalance_mask.reindex(result.index, fill_value=False).astype(bool)
    atr_periods = {
        config["atr_period"]
        for config in (risk, take)
        if config["enabled"] and config["type"] == "atr_multiple"
    }
    atr_by_code = {
        code: {
            period: pd.Series(
                atr(frame["high"], frame["low"], frame["close"], period).to_numpy(),
                index=pd.DatetimeIndex(frame["date"]),
            ).reindex(result.index)
            for period in atr_periods
        }
        for code, frame in pool_dfs.items()
    }
    opens = pd.DataFrame({
        code: frame.set_index("date")["open"] for code, frame in pool_dfs.items()
    }).reindex(result.index)
    closes = pd.DataFrame({
        code: frame.set_index("date")["close"] for code, frame in pool_dfs.items()
    }).reindex(result.index)
    entries: dict[str, tuple[float, dict[int, float | None]] | None] = {
        code: None for code in result.columns
    }
    locked = {code: False for code in result.columns}
    reasons_out: dict[tuple[pd.Timestamp, str], list[dict[str, Any]]] = {}

    for i, day in enumerate(result.index):
        if rebalance.iat[i]:
            for code in locked:
                locked[code] = False
        for code in result.columns:
            desired = float(weights.at[day, code])
            previous = float(result.iat[i - 1, result.columns.get_loc(code)]) if i else 0.0
            if locked[code]:
                result.at[day, code] = 0.0
                continue

            # 昨日收盘首次获得正权重，今日开盘形成模拟入场价。
            if entries[code] is None and i and previous > 0:
                open_price = opens.at[day, code]
                if (tradable.at[day, code] and pd.notna(open_price)
                        and float(open_price) > 0):
                    entry_atrs: dict[int, float | None] = {}
                    for period in atr_periods:
                        value = atr_by_code[code][period].iat[i - 1]
                        entry_atrs[period] = (
                            float(value) if pd.notna(value) and float(value) > 0 else None
                        )
                    entries[code] = (float(open_price) * (1 + slippage), entry_atrs)
                else:
                    result.at[day, code] = 0.0
                    locked[code] = True
                    continue

            entry = entries[code]
            reasons: list[dict[str, Any]] = []
            if entry is not None and previous > 0:
                close = closes.at[day, code]
                if pd.notna(close):
                    entry_price, entry_atrs = entry
                    risk_line = overlay_price_line(
                        risk, entry_price=entry_price,
                        entry_atr=entry_atrs.get(risk["atr_period"]), is_risk=True,
                    )
                    take_line = overlay_price_line(
                        take, entry_price=entry_price,
                        entry_atr=entry_atrs.get(take["atr_period"]), is_risk=False,
                    )
                    if risk_line is not None and float(close) <= risk_line:
                        reasons.append(reason("risk_overlay", price_line=risk_line))
                    if take_line is not None and float(close) >= take_line:
                        reasons.append(reason("take_profit", price_line=take_line))
                if desired <= 0:
                    reasons.extend(base_exit_reasons.get(
                        (day, code), [reason("rebalance")],
                    ))
            if reasons:
                ordered = sort_exit_reasons(reasons)
                reasons_out[(day, code)] = ordered
                result.at[day, code] = 0.0
                entries[code] = None
                if any(item["code"] in {"risk_overlay", "take_profit"}
                       for item in ordered):
                    locked[code] = True
            elif desired <= 0:
                entries[code] = None
    snapshots: dict[str, dict[str, Any]] = {}
    if len(result):
        data_date = str(pd.Timestamp(result.index[-1]).date())
        for code in result.columns:
            if float(result[code].iat[-1]) <= 0:
                continue
            entry = entries[code]
            rules = []
            for source, config, is_risk, name in (
                ("risk_overlay", risk, True, "风险覆盖层"),
                ("take_profit", take, False, "止盈覆盖层"),
            ):
                if not config["enabled"]:
                    continue
                entry_price = entry[0] if entry is not None else None
                entry_atr = (
                    entry[1].get(config["atr_period"])
                    if entry is not None else None
                )
                line = (
                    overlay_price_line(
                        config, entry_price=entry_price,
                        entry_atr=entry_atr, is_risk=is_risk,
                    )
                    if entry_price is not None else None
                )
                if entry_price is None:
                    status = "pending_simulated_entry"
                    explanation = "等待下一模拟成交日形成逐股模拟入场价后计算。"
                elif line is None:
                    status = "insufficient_data"
                    explanation = "模拟入场时 ATR 数据不足，暂不能计算参考线。"
                else:
                    status = "calculated"
                    explanation = (
                        "基于逐股模拟入场价和固定比例计算。"
                        if config["type"] == "fixed_pct"
                        else "基于逐股模拟入场价和入场时 ATR 计算。"
                    )
                item = {
                    "source": source,
                    "name": name,
                    "type": config["type"],
                    "value": config["value"],
                    "atr_period": config["atr_period"],
                    "data_date": data_date,
                    "calculation_status": status,
                    "explanation": explanation,
                }
                if entry_price is not None:
                    item["simulated_entry_price"] = round(float(entry_price), 6)
                if line is not None:
                    item["reference_line"] = round(float(line), 6)
                rules.append(item)
            if rules:
                snapshots[code] = {"data_date": data_date, "rules": rules}
    return result, reasons_out, snapshots


def portfolio_base_exit_reasons(
    template: str,
    weights: pd.DataFrame,
    pool_dfs: dict[str, pd.DataFrame],
    rebalance_mask: pd.Series,
) -> dict[tuple[pd.Timestamp, str], list[dict[str, Any]]]:
    """区分组合模板原生风险退出与计划调仓/资格变化。"""
    previous = weights.shift().fillna(0.0)
    rebalance = rebalance_mask.reindex(weights.index, fill_value=False).astype(bool)
    native_blocked = pd.DataFrame(
        False, index=weights.index, columns=weights.columns,
    )
    if template == "momentum_rotation":
        closes = pd.DataFrame({
            code: frame.set_index("date")["close"]
            for code, frame in pool_dfs.items()
        }).reindex(index=weights.index, columns=weights.columns)
        native_blocked = closes.lt(closes.rolling(20).mean()).fillna(False)

    result: dict[tuple[pd.Timestamp, str], list[dict[str, Any]]] = {}
    for day in weights.index:
        for code in weights.columns:
            before = float(previous.at[day, code])
            target = float(weights.at[day, code])
            if target >= before - 1e-12:
                continue
            is_native = (
                template == "momentum_rotation"
                and target <= 1e-12
                and bool(native_blocked.at[day, code])
            )
            reasons = [reason("native")] if is_native else []
            if bool(rebalance.at[day]) or not is_native:
                reasons.append(reason("rebalance"))
            result[(day, code)] = sort_exit_reasons(reasons)
    return result


__all__ = [
    "DEFAULT_OVERLAYS",
    "DEFAULT_SIMULATION_SLIPPAGE",
    "EXIT_REASON_PRIORITY",
    "OVERLAY_KEYS",
    "apply_portfolio_overlays",
    "apply_single_overlays",
    "overlay_defaults",
    "overlay_price_line",
    "portfolio_base_exit_reasons",
    "reason",
    "single_entry_price_ceiling",
    "single_entry_price_floor",
    "sort_exit_reasons",
    "validate_overlay",
]
