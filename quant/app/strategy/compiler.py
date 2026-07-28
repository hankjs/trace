"""StrategySpec 到日频目标仓位/权重的通用编译器。

这里只形成 T 日收盘后的目标。T+1 开盘、涨跌停、停牌、缺 bar、费用与滑点继续
交给现有回测撮合引擎，避免产生第二套成交模拟。
"""
from __future__ import annotations

from dataclasses import dataclass
from copy import deepcopy
from typing import Any

import pandas as pd

from .components import build_reason_tree, evaluate_expression, used_component_versions
from .overlays import apply_portfolio_overlays, apply_single_overlays
from .spec import (
    Expression,
    PortfolioPositioningSpec,
    StrategySpec,
    parse_strategy_spec,
)

COMPILER_VERSION = "strategy-compiler-v1"


@dataclass(frozen=True)
class StateTransition:
    date: pd.Timestamp
    previous_state: str
    state: str
    target: float
    reason: dict[str, Any]
    code: str | None = None

    def as_dict(self) -> dict[str, Any]:
        return {
            "date": str(self.date.date()),
            "code": self.code,
            "previous_state": self.previous_state,
            "state": self.state,
            "target": self.target,
            "reason": self.reason,
        }


@dataclass(frozen=True)
class SingleCompilation:
    positions: pd.Series
    transitions: list[StateTransition]
    reasons: dict[pd.Timestamp, list[dict[str, Any]]]
    component_versions: dict[str, str]
    state: dict[str, Any]

    def reason_events(self) -> list[dict[str, Any]]:
        return [
            {"date": str(day.date()), "reasons": items}
            for day, items in sorted(self.reasons.items())
        ]


@dataclass(frozen=True)
class PortfolioCompilation:
    weights: pd.DataFrame
    transitions: list[StateTransition]
    reasons: dict[tuple[pd.Timestamp, str], list[dict[str, Any]]]
    component_versions: dict[str, str]

    def reason_events(self) -> list[dict[str, Any]]:
        return [
            {"date": str(day.date()), "code": code, "reasons": items}
            for (day, code), items in sorted(self.reasons.items())
        ]


def compile_single(
    spec: StrategySpec | dict[str, Any],
    df: pd.DataFrame,
    *,
    slippage: float = 0.0,
    entry_tradable: pd.Series | None = None,
    entry_price_floor: pd.Series | None = None,
    entry_price_ceiling: pd.Series | None = None,
) -> SingleCompilation:
    """编译 single 规格，并复用既有覆盖层的 T+1 模拟入场语义。"""
    parsed = parse_strategy_spec(spec)
    native = _compile_single_native(parsed, df)
    generic_risk = parsed.overlays.risk.enabled and not parsed.overlays.risk.trailing
    generic_take = (
        parsed.overlays.take_profit.enabled and not parsed.overlays.take_profit.trailing
    )
    needs_execution_state = (
        generic_risk
        or generic_take
        or entry_tradable is not None
        or parsed.execution.max_entry_premium > 0
    )
    if not needs_execution_state:
        return native

    if parsed.execution.max_entry_premium > 0 and (
        entry_price_floor is None or entry_price_ceiling is None
    ):
        reference_expr = _entry_price_reference(parsed.entry.condition)
        if reference_expr is None:
            raise ValueError("max_entry_premium 需要 entry 中的 close 上穿参考线")
        fields = _single_fields(df)
        reference = evaluate_expression(reference_expr, fields)
        if not isinstance(reference, pd.Series):
            raise ValueError("入场参考线必须返回逐日序列")
        dated_reference = pd.Series(
            reference.to_numpy(dtype=float),
            index=pd.DatetimeIndex(df["date"]),
        )
        if entry_price_floor is None:
            entry_price_floor = dated_reference
        if entry_price_ceiling is None:
            entry_price_ceiling = (
                dated_reference * (1 + parsed.execution.max_entry_premium)
            )

    params = {
        "risk_overlay": _legacy_overlay_config(
            parsed.overlays.risk, enabled=generic_risk,
        ),
        "take_profit": _legacy_overlay_config(
            parsed.overlays.take_profit, enabled=generic_take,
        ),
    }
    overlay_state: dict[str, Any] = {}
    overlaid, overlay_reasons = apply_single_overlays(
        df,
        native.positions,
        params,
        slippage=slippage,
        entry_tradable=entry_tradable,
        entry_price_floor=entry_price_floor,
        entry_price_ceiling=entry_price_ceiling,
        state_out=overlay_state,
    )
    positions = pd.Series(overlaid.to_numpy(), index=df.index)
    positions = _coerce_position_dtype(positions.astype(float))
    reasons = _merge_single_reasons(
        parsed, deepcopy(native.reasons), overlay_reasons,
    )
    transitions = _single_transitions(
        df, positions, native.positions, reasons, parsed.entry.reason_code,
    )
    return SingleCompilation(
        positions=positions,
        transitions=transitions,
        reasons=reasons,
        component_versions=native.component_versions,
        state=overlay_state,
    )


def _compile_single_native(
    spec: StrategySpec | dict[str, Any],
    df: pd.DataFrame,
) -> SingleCompilation:
    """编译原生状态；trailing 覆盖层属于预置策略内生风险线。"""
    parsed = parse_strategy_spec(spec)
    if parsed.kind != "single":
        raise ValueError("compile_single 只接受 single StrategySpec")
    _validate_bars(df)
    fields = _single_fields(df)
    _require_fields(parsed, fields)

    entry = _as_bool_series(
        evaluate_expression(parsed.entry.condition, fields), df.index,
    )
    assert parsed.native_exit is not None
    native_exit = _as_bool_series(
        evaluate_expression(parsed.native_exit.condition, fields), df.index,
    )
    dates = pd.DatetimeIndex(df["date"])
    close = fields["close"]
    target = float(parsed.positioning.target)  # validated as SinglePositioningSpec
    positions = pd.Series(0.0, index=df.index, dtype=float)
    transitions: list[StateTransition] = []
    reasons: dict[pd.Timestamp, list[dict[str, Any]]] = {}

    atr_periods = {
        overlay.atr_period
        for overlay in (parsed.overlays.risk, parsed.overlays.take_profit)
        if overlay.enabled and overlay.trailing and overlay.type == "atr_multiple"
    }
    atr_values = {
        period: evaluate_expression(_atr_expression(period), fields)
        for period in atr_periods
    }

    holding = False
    state = "flat"
    blocked_until_reset = False
    reset_bars = 0
    entry_price: float | None = None
    entry_atr: dict[int, float | None] = {}
    risk_line: float | None = None
    take_line: float | None = None

    def transition(
        position: int,
        new_state: str,
        new_target: float,
        reason: dict[str, Any],
    ) -> None:
        nonlocal state
        if new_state == state:
            return
        transitions.append(StateTransition(
            date=dates[position],
            previous_state=state,
            state=new_state,
            target=new_target,
            reason=reason,
        ))
        state = new_state

    for i, day in enumerate(dates):
        entry_now = bool(entry.iat[i])
        if holding:
            exit_items: list[dict[str, Any]] = []
            price = _finite_or_none(close.iat[i])
            risk = parsed.overlays.risk
            take = parsed.overlays.take_profit
            if price is not None:
                if risk.enabled and risk.trailing:
                    candidate = _overlay_line(
                        risk.type,
                        risk.value,
                        entry_price,
                        entry_atr.get(risk.atr_period),
                        _value_at(atr_values.get(risk.atr_period), i),
                        is_risk=True,
                    )
                    if candidate is not None:
                        risk_line = (
                            max(risk_line, candidate)
                            if risk.trailing and risk_line is not None else candidate
                        )
                    if risk_line is not None and price < risk_line:
                        exit_items.append({
                            "code": "native_exit",
                            "reason_code": "native_atr_risk_line",
                            "price_line": round(risk_line, 12),
                        })
                if take.enabled and take.trailing:
                    candidate = _overlay_line(
                        take.type,
                        take.value,
                        entry_price,
                        entry_atr.get(take.atr_period),
                        _value_at(atr_values.get(take.atr_period), i),
                        is_risk=False,
                    )
                    if candidate is not None:
                        take_line = (
                            min(take_line, candidate)
                            if take.trailing and take_line is not None else candidate
                        )
                    if take_line is not None and price > take_line:
                        exit_items.append({
                            "code": "take_profit",
                            "price_line": round(take_line, 12),
                        })
            if bool(native_exit.iat[i]):
                exit_items.append({
                    "code": "native_exit",
                    "reason_code": parsed.native_exit.reason_code,
                    "tree": build_reason_tree(parsed.native_exit.condition, fields, i),
                })
            if exit_items:
                # 同一日同时命中内生 ATR 线和原生条件时，legacy 模板只产生一次
                # native 退出；保留首个原因及其参考线，避免重复计数。
                deduplicated: dict[str, dict[str, Any]] = {}
                for item in exit_items:
                    deduplicated.setdefault(item["code"], item)
                exit_items = list(deduplicated.values())
                holding = False
                positions.iat[i] = 0.0
                reasons[day] = exit_items
                risk_exit = any(
                    item["code"] == "take_profit"
                    for item in exit_items
                )
                # trailing 规则是策略内生状态的一部分；退出后等待新的原生入场事件，
                # 不再叠加通用覆盖层的 native-reset 锁定。
                blocked_until_reset = False
                reset_bars = 0
                transition(i, "exit_pending", 0.0, {
                    "type": "exit", "all_reasons": exit_items,
                })
                entry_price = None
                entry_atr = {}
                risk_line = None
                take_line = None
                continue
            positions.iat[i] = target
            transition(i, "holding", target, {"type": "target_unchanged"})
            continue

        if blocked_until_reset:
            if not entry_now:
                reset_bars += 1
                if reset_bars > parsed.holding.cooldown_days:
                    blocked_until_reset = False
                    reset_bars = 0
            if blocked_until_reset:
                positions.iat[i] = 0.0
                transition(i, "cooldown", 0.0, {
                    "type": "risk_reentry_blocked",
                    "policy": parsed.holding.risk_reentry,
                })
                continue

        if entry_now:
            holding = True
            positions.iat[i] = target
            entry_price = _finite_or_none(close.iat[i])
            entry_atr = {
                period: _finite_or_none(value.iat[i])
                for period, value in atr_values.items()
            }
            risk = parsed.overlays.risk
            take = parsed.overlays.take_profit
            risk_line = (
                _overlay_line(
                    risk.type, risk.value, entry_price,
                    entry_atr.get(risk.atr_period), entry_atr.get(risk.atr_period),
                    is_risk=True,
                )
                if risk.enabled and risk.trailing else None
            )
            take_line = (
                _overlay_line(
                    take.type, take.value, entry_price,
                    entry_atr.get(take.atr_period), entry_atr.get(take.atr_period),
                    is_risk=False,
                )
                if take.enabled and take.trailing else None
            )
            entry_reason = {
                "type": "entry",
                "reason_code": parsed.entry.reason_code,
                "tree": build_reason_tree(parsed.entry.condition, fields, i),
            }
            reasons[day] = [entry_reason]
            transition(i, "entry_pending", target, entry_reason)
        else:
            positions.iat[i] = 0.0
            transition(i, "watch", 0.0, {
                "type": "watch", "reason_code": parsed.entry.reason_code,
            })

    return SingleCompilation(
        positions=_coerce_position_dtype(positions),
        transitions=transitions,
        reasons=reasons,
        component_versions=component_versions_for_spec(parsed),
        state={},
    )


def compile_portfolio(
    spec: StrategySpec | dict[str, Any],
    dates,
    pool_dfs: dict[str, pd.DataFrame],
    eligibility: pd.DataFrame | None = None,
    *,
    slippage: float = 0.0,
    entry_tradable: pd.DataFrame | None = None,
) -> PortfolioCompilation:
    """把 portfolio 规格编译为每日目标权重和逐标的变化原因。"""
    parsed = parse_strategy_spec(spec)
    if parsed.kind != "portfolio" or not isinstance(
        parsed.positioning, PortfolioPositioningSpec
    ):
        raise ValueError("compile_portfolio 只接受 portfolio StrategySpec")
    index = pd.DatetimeIndex(dates)
    if not index.is_monotonic_increasing or index.has_duplicates:
        raise ValueError("dates 必须严格递增且不重复")
    if not pool_dfs:
        raise ValueError("pool_dfs 不能为空")
    fields = _portfolio_fields(index, pool_dfs)
    _require_fields(parsed, fields)
    columns = list(pool_dfs)

    score = _as_frame(
        evaluate_expression(parsed.positioning.score, fields), index, columns,
    ).astype(float)
    entry = _as_bool_frame(
        evaluate_expression(parsed.entry.condition, fields), index, columns,
    )
    native_exit = (
        _as_bool_frame(
            evaluate_expression(parsed.native_exit.condition, fields), index, columns,
        )
        if parsed.native_exit is not None
        else pd.DataFrame(False, index=index, columns=columns)
    )
    risk_filter = (
        _as_bool_frame(
            evaluate_expression(parsed.positioning.risk_filter, fields), index, columns,
        )
        if parsed.positioning.risk_filter is not None
        else pd.DataFrame(False, index=index, columns=columns)
    )
    eligible = (
        eligibility.reindex(index=index, columns=columns).fillna(False).astype(bool)
        if eligibility is not None
        else pd.DataFrame(True, index=index, columns=columns)
    )
    eligible &= entry
    rebalance = _rebalance_mask(index, parsed.positioning.rebalance)
    base_weights = pd.DataFrame(0.0, index=index, columns=columns)
    current = pd.Series(0.0, index=columns)
    for i, day in enumerate(index):
        rebalance_today = bool(rebalance.iat[i])
        if rebalance_today:
            ranked = score.iloc[i].where(eligible.iloc[i]).dropna()
            top_n = min(
                parsed.positioning.selection.n,
                parsed.portfolio_constraints.max_positions,
            )
            selected = ranked.nlargest(top_n)
            current = _selected_weights(
                selected,
                columns,
                parsed.positioning.weighting.type,
                parsed.portfolio_constraints.max_single_weight,
                parsed.portfolio_constraints.max_total_weight,
            )
        current = current.where(eligible.iloc[i], 0.0)
        current = current.where(~native_exit.iloc[i], 0.0)
        output = current.where(~risk_filter.iloc[i], 0.0)
        base_weights.iloc[i] = output

    base_exit_reasons: dict[
        tuple[pd.Timestamp, str], list[dict[str, Any]]
    ] = {}
    previous_base = base_weights.shift().fillna(0.0)
    for i, day in enumerate(index):
        for code in columns:
            before = float(previous_base.at[day, code])
            target = float(base_weights.at[day, code])
            if target >= before - 1e-12:
                continue
            native_blocked = bool(
                target <= 1e-12
                and (risk_filter.at[day, code] or native_exit.at[day, code])
            )
            items: list[dict[str, Any]] = []
            if native_blocked:
                items.append({"code": "native", "name": "策略原生退出"})
            if bool(rebalance.iat[i]) or not native_blocked:
                items.append({"code": "rebalance", "name": "组合调仓或资格变化"})
            base_exit_reasons[(day, code)] = items

    overlay_params = {
        "risk_overlay": _legacy_overlay_config(
            parsed.overlays.risk, enabled=parsed.overlays.risk.enabled,
        ),
        "take_profit": _legacy_overlay_config(
            parsed.overlays.take_profit, enabled=parsed.overlays.take_profit.enabled,
        ),
    }
    weights, overlay_reasons, _ = apply_portfolio_overlays(
        base_weights,
        pool_dfs,
        overlay_params,
        rebalance,
        slippage=slippage,
        entry_tradable=entry_tradable,
        base_exit_reasons=base_exit_reasons,
    )

    reasons: dict[tuple[pd.Timestamp, str], list[dict[str, Any]]] = {}
    transitions: list[StateTransition] = []
    previous_output = pd.Series(0.0, index=columns)
    for i, day in enumerate(index):
        rebalance_today = bool(rebalance.iat[i])
        for code in columns:
            previous = float(previous_output[code])
            target = float(weights.at[day, code])
            if abs(previous - target) <= 1e-15:
                continue
            if previous == 0 and target > 0:
                state = "entry_pending"
                change = "entry"
            elif previous > 0 and target == 0:
                state = "exit_pending"
                change = "exit"
            elif target > previous:
                state = "increase_pending"
                change = "increase"
            else:
                state = "reduce_pending"
                change = "reduce"
            item: dict[str, Any] = {
                "type": change,
                "rebalance": rebalance_today,
                "eligible": bool(eligible.at[day, code]),
                "risk_blocked": bool(risk_filter.at[day, code]),
                "native_exit": bool(native_exit.at[day, code]),
                "score": _finite_or_none(score.at[day, code]),
            }
            if target < previous:
                exit_items = _compiler_portfolio_exit_items(
                    parsed, overlay_reasons.get((day, code), []),
                )
                item["all_reasons"] = exit_items or [{"code": "rebalance"}]
                item["overlay_blocked"] = any(
                    reason["code"] in {"risk_overlay", "take_profit"}
                    for reason in item["all_reasons"]
                )
            else:
                item["overlay_blocked"] = False
            if target > 0:
                item["score_tree"] = build_reason_tree(
                    parsed.positioning.score, fields, i, column=code,
                )
            if native_exit.at[day, code] and parsed.native_exit is not None:
                item["exit_tree"] = build_reason_tree(
                    parsed.native_exit.condition, fields, i, column=code,
                )
            reasons[(day, code)] = [item]
            transitions.append(StateTransition(
                date=day,
                code=code,
                previous_state="holding" if previous > 0 else "flat",
                state=state,
                target=target,
                reason=item,
            ))
        previous_output = weights.loc[day].copy()

    return PortfolioCompilation(
        weights=weights,
        transitions=transitions,
        reasons=reasons,
        component_versions=component_versions_for_spec(parsed),
    )


def component_versions_for_spec(
    spec: StrategySpec | dict[str, Any],
) -> dict[str, str]:
    parsed = parse_strategy_spec(spec)
    versions: dict[str, str] = {}
    expressions = [parsed.entry.condition]
    if parsed.native_exit is not None:
        expressions.append(parsed.native_exit.condition)
    if isinstance(parsed.positioning, PortfolioPositioningSpec):
        expressions.append(parsed.positioning.score)
        if parsed.positioning.risk_filter is not None:
            expressions.append(parsed.positioning.risk_filter)
    for expr in expressions:
        versions.update(used_component_versions(expr))
    return dict(sorted(versions.items()))


def _legacy_overlay_config(overlay, *, enabled: bool) -> dict[str, Any]:
    return {
        "enabled": enabled,
        "type": overlay.type,
        "value": overlay.value,
        "atr_period": overlay.atr_period,
    }


def _entry_price_reference(expr: Expression) -> Expression | None:
    if (
        expr.op in {"gt", "gte", "cross_above"}
        and expr.left is not None
        and expr.left.op == "field"
        and expr.left.name == "close"
    ):
        return expr.right
    for child in (
        expr.arg, expr.left, expr.right, expr.input, expr.high, expr.low, expr.close,
    ):
        if child is not None:
            found = _entry_price_reference(child)
            if found is not None:
                return found
    for child in expr.args or []:
        found = _entry_price_reference(child)
        if found is not None:
            return found
    return None


def _compiler_portfolio_exit_items(
    spec: StrategySpec,
    items: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    converted: list[dict[str, Any]] = []
    for item in items:
        code = item.get("code")
        if code == "native":
            converted.append({
                "code": "native_exit",
                "reason_code": (
                    spec.native_exit.reason_code
                    if spec.native_exit is not None else "portfolio_risk_filter"
                ),
            })
        elif code in {"risk_overlay", "take_profit", "rebalance"}:
            converted.append({
                key: value for key, value in item.items()
                if key in {"code", "price_line"}
            })
    return converted


def _merge_single_reasons(
    spec: StrategySpec,
    reasons: dict[pd.Timestamp, list[dict[str, Any]]],
    overlay_reasons: dict[pd.Timestamp, list[dict[str, Any]]],
) -> dict[pd.Timestamp, list[dict[str, Any]]]:
    priority = {"risk_overlay": 0, "take_profit": 1, "native_exit": 2}
    for day, items in overlay_reasons.items():
        existing = reasons.get(day, [])
        native_item = next(
            (item for item in existing if item.get("code") == "native_exit"),
            {
                "code": "native_exit",
                "reason_code": (
                    spec.native_exit.reason_code if spec.native_exit is not None
                    else "native_exit"
                ),
            },
        )
        converted: list[dict[str, Any]] = []
        for item in items:
            if item.get("code") == "native":
                converted.append(native_item)
            elif item.get("code") in {"risk_overlay", "take_profit"}:
                converted.append({
                    key: value for key, value in item.items()
                    if key in {"code", "price_line"}
                })
        by_code = {
            item["code"]: item for item in converted
            if item.get("code") in priority
        }
        reasons[day] = sorted(by_code.values(), key=lambda item: priority[item["code"]])
    return reasons


def _single_transitions(
    df: pd.DataFrame,
    positions: pd.Series,
    native_positions: pd.Series,
    reasons: dict[pd.Timestamp, list[dict[str, Any]]],
    entry_reason_code: str,
) -> list[StateTransition]:
    dates = pd.DatetimeIndex(df["date"])
    transitions: list[StateTransition] = []
    state = "flat"
    previous = 0.0
    blocked = False
    for i, day in enumerate(dates):
        target = float(positions.iat[i])
        day_reasons = reasons.get(day, [])
        if previous <= 0 < target:
            new_state = "entry_pending"
            reason = next(
                (item for item in day_reasons if item.get("type") == "entry"),
                {"type": "entry", "reason_code": entry_reason_code},
            )
        elif previous > 0 and target <= 0:
            new_state = "exit_pending"
            reason = {"type": "exit", "all_reasons": day_reasons}
            blocked = any(
                item.get("code") in {"risk_overlay", "take_profit"}
                for item in day_reasons
            )
        elif target > 0:
            new_state = "holding"
            reason = {"type": "target_unchanged"}
        else:
            if blocked and float(native_positions.iat[i]) > 0:
                new_state = "cooldown"
                reason = {"type": "risk_reentry_blocked", "policy": "native_reset"}
            else:
                if float(native_positions.iat[i]) <= 0:
                    blocked = False
                new_state = "watch"
                reason = {"type": "watch", "reason_code": entry_reason_code}
        if new_state != state:
            transitions.append(StateTransition(
                date=day,
                previous_state=state,
                state=new_state,
                target=target,
                reason=reason,
            ))
            state = new_state
        previous = target
    return transitions


def _single_fields(df: pd.DataFrame) -> dict[str, pd.Series]:
    return {
        column: pd.Series(df[column].to_numpy(), index=df.index)
        for column in df.columns
        if column != "date"
    }


def _portfolio_fields(
    index: pd.DatetimeIndex,
    pool_dfs: dict[str, pd.DataFrame],
) -> dict[str, pd.DataFrame]:
    common = set.intersection(*(set(frame.columns) for frame in pool_dfs.values()))
    common.discard("date")
    return {
        field: pd.DataFrame({
            code: pd.Series(
                frame[field].to_numpy(),
                index=pd.DatetimeIndex(frame["date"]),
            )
            for code, frame in pool_dfs.items()
        }).reindex(index)
        for field in common
    }


def _require_fields(spec: StrategySpec, fields: dict[str, Any]) -> None:
    missing = sorted({
        item.field for item in spec.data_requirements
        if item.required and item.field not in fields
    })
    if missing:
        raise ValueError(f"输入数据缺少 StrategySpec 必需字段: {missing}")


def _validate_bars(df: pd.DataFrame) -> None:
    if "date" not in df:
        raise ValueError("行情必须包含 date")
    dates = pd.DatetimeIndex(df["date"])
    if not dates.is_monotonic_increasing or dates.has_duplicates:
        raise ValueError("行情日期必须严格递增且不重复")


def _as_bool_series(value: Any, index: pd.Index) -> pd.Series:
    if isinstance(value, pd.Series):
        return value.reindex(index).fillna(False).astype(bool)
    if isinstance(value, bool):
        return pd.Series(value, index=index, dtype=bool)
    raise ValueError("single 规则必须返回 Series 或布尔字面量")


def _as_frame(
    value: Any,
    index: pd.DatetimeIndex,
    columns: list[str],
) -> pd.DataFrame:
    if isinstance(value, pd.DataFrame):
        return value.reindex(index=index, columns=columns)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return pd.DataFrame(float(value), index=index, columns=columns)
    raise ValueError("组合数值表达式必须返回 DataFrame 或数值字面量")


def _as_bool_frame(
    value: Any,
    index: pd.DatetimeIndex,
    columns: list[str],
) -> pd.DataFrame:
    if isinstance(value, pd.DataFrame):
        return value.reindex(index=index, columns=columns).fillna(False).astype(bool)
    if isinstance(value, bool):
        return pd.DataFrame(value, index=index, columns=columns, dtype=bool)
    raise ValueError("组合规则必须返回 DataFrame 或布尔字面量")


def _rebalance_mask(index: pd.DatetimeIndex, spec) -> pd.Series:
    if spec.frequency == "fixed":
        assert spec.interval_days is not None
        return pd.Series(
            [position % spec.interval_days == 0 for position in range(len(index))],
            index=index,
        )
    if spec.frequency == "weekly":
        iso = index.isocalendar()
        group = pd.Series(
            iso["year"].to_numpy() * 100 + iso["week"].to_numpy(), index=index,
        )
    else:
        group = pd.Series(index.year * 100 + index.month, index=index)
    return group.ne(group.shift())


def _selected_weights(
    selected: pd.Series,
    columns: list[str],
    weighting: str,
    max_single: float,
    max_total: float,
) -> pd.Series:
    result = pd.Series(0.0, index=columns)
    if selected.empty:
        return result
    if weighting == "equal":
        raw = pd.Series(1.0 / len(selected), index=selected.index)
    else:
        rank_points = pd.Series(
            range(len(selected), 0, -1), index=selected.index, dtype=float,
        )
        raw = rank_points / rank_points.sum()
    raw = raw.clip(upper=max_single)
    if raw.sum() > max_total:
        raw *= max_total / raw.sum()
    result.loc[raw.index] = raw
    return result


def _atr_expression(period: int) -> Expression:
    return Expression.model_validate({
        "op": "atr",
        "high": {"op": "field", "name": "high"},
        "low": {"op": "field", "name": "low"},
        "close": {"op": "field", "name": "close"},
        "window": period,
    })


def _overlay_line(
    overlay_type: str,
    value: float,
    entry_price: float | None,
    entry_atr: float | None,
    current_atr: float | None,
    *,
    is_risk: bool,
) -> float | None:
    if entry_price is None:
        return None
    if overlay_type == "fixed_pct":
        distance = entry_price * value
    else:
        atr_value = current_atr if current_atr is not None else entry_atr
        if atr_value is None:
            return None
        distance = atr_value * value
    return entry_price - distance if is_risk else entry_price + distance


def _value_at(value: Any, position: int) -> Any:
    return value.iat[position] if isinstance(value, pd.Series) else None


def _matrix_value(
    value: Any,
    day: pd.Timestamp,
    code: str,
) -> float | None:
    if not isinstance(value, pd.DataFrame):
        return None
    return _finite_or_none(value.at[day, code])


def _finite_or_none(value: Any) -> float | None:
    if value is None or pd.isna(value):
        return None
    result = float(value)
    return result if pd.notna(result) else None


def _coerce_position_dtype(value: pd.Series) -> pd.Series:
    if set(value.unique()).issubset({0.0, 1.0}):
        return value.astype(int)
    return value


__all__ = [
    "COMPILER_VERSION", "PortfolioCompilation", "SingleCompilation", "StateTransition",
    "compile_portfolio", "compile_single", "component_versions_for_spec",
]
