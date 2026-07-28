"""策略规格 validation 段的执行侧:基线对比、锁定样本外(OOS)与自动否决。

设计口径(docs/strategy-research-plan.md 只列出阶段目标,未定细节处按本模块
常量固化,改动需评审):

- **OOS 切分**:回测窗口净值序列的最后 ``OOS_FRACTION``(20%)个交易日为锁定
  样本外,其余为样本内;OOS 段不足 ``MIN_OOS_BARS`` 根 bar 时如实标记不可用,
  相关否决条件记为 unevaluated 而不是当作通过;
- **基线**:``buy_and_hold``(区间首个有效收盘归一、等权买入持有)与
  ``equal_weight``(每日等权再平衡)为内置基线,直接由回测已加载的价格矩阵
  计算。目前不存在"同区间同基线结果"的复用基建,而这两个基线的计算只是
  价格矩阵的归一化/平均,相对回测本身成本可忽略,故不做缓存复用;
  未知 ``baseline_id`` 记为 unavailable issue,不静默跳过;
- **否决**:旧字符串条件(``no_net_oos_increment`` 等)按本模块映射为结构化
  判定;新规格可直接写 ``validation.rejection_rules``(metric/op/threshold/
  segment)。任何条件缺少所需数据(无 OOS 段、基线不可用、无扫描结果)时记为
  unevaluated,既不算通过也不算否决;verdict: 有命中 -> "rejected",
  无命中但有未评估 -> "incomplete",全部通过 -> "passed"。
"""
from __future__ import annotations

import math
from datetime import date
from typing import Any

import numpy as np
import pandas as pd

from ..strategy.spec import StrategySpec

OOS_FRACTION = 0.2  # 锁定样本外占回测窗口交易日数的比例(文档未定,定为最后 20%)
MIN_OOS_BARS = 5    # OOS 段少于此长度的指标不可靠,如实标记不可用

BASELINE_NAMES = {
    "buy_and_hold": "买入持有(等权)",
    "equal_weight": "每日等权再平衡",
}

# 基线对比并排展示的核心指标;delta = 策略 - 基线(越高越好)
_BASELINE_DELTA_METRICS = ("total_return", "annual_return", "sharpe")

# 字符串否决条件的兼容映射说明(判定实现见 _eval_legacy_criterion):
# - no_net_oos_increment: OOS 段年化不超过全部可用基线的 OOS 年化最大值 -> 否决
# - unstable_parameters: 按声明扫描结果中优于当前参数的组合占比 > 50% -> 否决
# - capacity_failure: 全区间 round_trips 为 0,策略无法形成有效持仓 -> 否决
_LEGACY_CRITERIA = (
    "no_net_oos_increment", "unstable_parameters", "capacity_failure",
)


def _close_matrix(
    frames: dict[str, pd.DataFrame], start: date, end: date,
) -> pd.DataFrame:
    """与引擎同口径的收盘价矩阵:行 = [start, end] 内交易日并集,列 = 标的。"""
    idx = pd.DatetimeIndex(
        sorted({d for df in frames.values() for d in df["date"] if start <= d <= end})
    )
    if not len(idx):
        return pd.DataFrame()
    data = {
        code: df.set_index("date")["close"]
        for code, df in frames.items() if len(df)
    }
    return pd.DataFrame(data).reindex(idx)


def baseline_equity(
    baseline_id: str,
    frames: dict[str, pd.DataFrame],
    start: date,
    end: date,
) -> pd.Series | None:
    """内置基线的净值曲线;未知基线或数据不足返回 None。"""
    if baseline_id not in BASELINE_NAMES:
        return None
    close = _close_matrix(frames, start, end)
    if close.empty or not len(close.columns):
        return None
    if baseline_id == "buy_and_hold":
        # 每只标的以窗口内首个有效收盘归一,等权平均;窗口内新上市的标的自其
        # 首个有效值起计入(skipna 平均,与引擎等权合成口径一致)。
        first = close.apply(lambda s: s.dropna().iloc[0] if s.notna().any() else np.nan)
        normed = close / first
        equity = normed.ffill().mean(axis=1, skipna=True)
    else:
        # 每日等权再平衡:组合日收益 = 当日有数据标的的等权平均收益
        returns = close.pct_change().mean(axis=1, skipna=True).fillna(0.0)
        equity = (1.0 + returns).cumprod()
    equity = equity.dropna()
    return equity if len(equity) >= 3 else None


def segment_metrics(eq: pd.Series) -> dict[str, Any]:
    """净值子序列的核心指标(段首归一为 1.0)。序列过短时年化/夏普为 None。"""
    from .engine import _equity_statistics, _sharpe_ratio

    rebased = eq / eq.iloc[0]
    total, annual, max_dd, rets = _equity_statistics(rebased)
    return {
        "total_return": round(total, 4),
        "annual_return": None if annual is None else round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": _sharpe_ratio(rets),
    }


def split_oos(eq: pd.Series) -> tuple[pd.Series, pd.Series]:
    """(样本内, 样本外) 两段;OOS 为最后 OOS_FRACTION 比例的交易日。"""
    oos_n = int(len(eq) * OOS_FRACTION)
    return eq.iloc[: len(eq) - oos_n], eq.iloc[len(eq) - oos_n:]


def build_oos_report(locked: bool, eq: pd.Series) -> dict[str, Any]:
    """locked_oos=true 时报告样本内/锁定样本外两段各自的核心指标。"""
    if not locked:
        return {"enabled": False}
    in_sample, oos = split_oos(eq)
    if len(in_sample) < 2 or len(oos) < MIN_OOS_BARS:
        return {
            "enabled": True,
            "available": False,
            "fraction": OOS_FRACTION,
            "message": f"样本外段不足 {MIN_OOS_BARS} 根 bar,无法评估",
        }
    return {
        "enabled": True,
        "available": True,
        "fraction": OOS_FRACTION,
        "oos_start": str(pd.Timestamp(oos.index[0]).date()),
        "in_sample_bars": len(in_sample),
        "oos_bars": len(oos),
        "in_sample": segment_metrics(in_sample),
        "oos": segment_metrics(oos),
    }


def _baseline_results(
    spec: StrategySpec,
    frames: dict[str, pd.DataFrame],
    start: date,
    end: date,
) -> list[dict[str, Any]]:
    """逐个计算声明的基线(去重)。ok 条目带 equity 与分段指标,供否决判定复用。"""
    results: list[dict[str, Any]] = []
    seen: set[str] = set()
    for baseline_id in spec.validation.baseline_ids:
        if baseline_id in seen:
            continue  # 同一基线重复声明只算一次
        seen.add(baseline_id)
        name = BASELINE_NAMES.get(baseline_id, baseline_id)
        equity = baseline_equity(baseline_id, frames, start, end)
        if equity is None:
            results.append({
                "baseline_id": baseline_id,
                "name": name,
                "status": "unavailable",
                "message": (
                    f"未知基线 {baseline_id},内置可选: {', '.join(sorted(BASELINE_NAMES))}"
                    if baseline_id not in BASELINE_NAMES
                    else "回测区间内价格数据不足,无法构造基线净值"
                ),
            })
            continue
        in_sample, oos = split_oos(equity)
        results.append({
            "baseline_id": baseline_id,
            "name": name,
            "status": "ok",
            "metrics": segment_metrics(equity),
            "in_sample_metrics": (
                segment_metrics(in_sample) if len(in_sample) >= 2 else None
            ),
            "oos_metrics": (
                segment_metrics(oos) if len(oos) >= MIN_OOS_BARS else None
            ),
            "equity": equity,
        })
    return results


def _diff(actual: Any, base: Any) -> float | None:
    if isinstance(actual, (int, float)) and isinstance(base, (int, float)):
        return round(float(actual) - float(base), 4)
    return None


def _compare(actual: float, op: str, threshold: float) -> bool:
    return {
        "lt": actual < threshold,
        "lte": actual <= threshold,
        "gt": actual > threshold,
        "gte": actual >= threshold,
    }[op]


def evaluate_declared_sweep(
    spec: StrategySpec, sweep_rows: list[dict[str, Any]],
) -> dict[str, Any]:
    """按声明扫描的稳定性评估(unstable_parameters 的判定依据)。

    在扫描结果中找与当前规格在各扫描路径上取值全等的组合作为"当前参数";
    优于当前参数的组合占比超过 50% 视为参数不稳定。当前参数不在候选值中
    或全部组合无法年化时如实返回 unevaluated。
    """
    raw = spec.model_dump(mode="json")
    scans = spec.validation.parameter_scans
    current_params = {scan.path: _get_spec_path(raw, scan.path) for scan in scans}
    current_value: float | None = None
    for row in sweep_rows:
        params = row.get("params", {})
        if all(path in params and params[path] == value
               for path, value in current_params.items()):
            value = row.get("metrics", {}).get("annual_return_median")
            current_value = value if isinstance(value, (int, float)) else None
            break
    values = [
        row.get("metrics", {}).get("annual_return_median") for row in sweep_rows
    ]
    values = [float(v) for v in values if isinstance(v, (int, float))]
    if current_value is None or not values:
        return {
            "status": "unevaluated",
            "reason": "扫描结果未包含当前参数组合,或全部组合区间过短无法年化",
            "current_params": current_params,
        }
    better = sum(1 for value in values if value > current_value)
    share = round(better / len(values), 4)
    return {
        "status": "evaluated",
        "current": round(float(current_value), 4),
        "median": round(float(np.median(values)), 4),
        "better_share": share,
        "unstable": share > 0.5,
        "current_params": current_params,
    }


def _get_spec_path(raw: dict, path: str) -> Any:
    target: Any = raw
    for part in path[2:].split("."):
        if not isinstance(target, dict) or part not in target:
            return None
        target = target[part]
    return target


def evaluate_rejection(
    spec: StrategySpec,
    *,
    full_metrics: dict[str, Any],
    oos_report: dict[str, Any],
    baselines: list[dict[str, Any]],
    sweep: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """逐条评估否决条件(旧字符串 + 结构化规则),返回命中与未评估明细。"""
    hits: list[dict[str, Any]] = []
    unevaluated: list[dict[str, Any]] = []
    ctx = {
        "full_metrics": full_metrics,
        "oos_report": oos_report,
        "baselines": baselines,
        "sweep": sweep,
    }
    for criterion in spec.validation.rejection_criteria:
        if criterion in _LEGACY_CRITERIA:
            _eval_legacy_criterion(criterion, spec, ctx, hits, unevaluated)
        else:
            unevaluated.append({
                "criterion": criterion,
                "reason": "未知否决条件,未执行(可改用结构化 rejection_rules)",
            })
    for rule in spec.validation.rejection_rules:
        _eval_structured_rule(rule, ctx, hits, unevaluated)
    verdict = (
        "rejected" if hits else ("incomplete" if unevaluated else "passed")
    )
    return {"verdict": verdict, "hits": hits, "unevaluated": unevaluated}


def _segment_view(ctx: dict[str, Any], segment: str) -> dict[str, Any]:
    if segment == "full":
        return ctx["full_metrics"]
    oos_report = ctx["oos_report"]
    if not oos_report.get("available"):
        return {}
    return oos_report.get(segment) or {}


def _best_baseline_annual(
    baselines: list[dict[str, Any]], segment: str,
) -> float | None:
    key = {"full": "metrics", "in_sample": "in_sample_metrics",
           "oos": "oos_metrics"}[segment]
    values = [
        entry[key]["annual_return"]
        for entry in baselines
        if entry["status"] == "ok"
        and entry.get(key) is not None
        and isinstance(entry[key].get("annual_return"), (int, float))
    ]
    return max(values) if values else None


def _eval_legacy_criterion(
    criterion: str,
    spec: StrategySpec,
    ctx: dict[str, Any],
    hits: list[dict[str, Any]],
    unevaluated: list[dict[str, Any]],
) -> None:
    if criterion == "no_net_oos_increment":
        oos = _segment_view(ctx, "oos")
        actual = oos.get("annual_return")
        if actual is None:
            unevaluated.append({
                "criterion": criterion,
                "reason": "锁定样本外段不可用或区间过短无法年化",
            })
            return
        best = _best_baseline_annual(ctx["baselines"], "oos")
        if best is None:
            unevaluated.append({
                "criterion": criterion,
                "reason": "对照基线在样本外段不可用",
            })
            return
        excess = round(float(actual) - float(best), 4)
        if excess <= 0:
            hits.append({
                "criterion": criterion,
                "metric": "excess_annual_return_vs_best_baseline",
                "segment": "oos",
                "op": "lte",
                "threshold": 0,
                "actual": excess,
                "detail": (
                    f"样本外年化 {actual:.2%} 未超过最佳基线 {best:.2%}"
                    f"(净增量 {excess:.2%})"
                ),
            })
    elif criterion == "unstable_parameters":
        if not spec.validation.parameter_scans:
            unevaluated.append({
                "criterion": criterion,
                "reason": "规格未声明 parameter_scans,无扫描依据",
            })
            return
        sweep = ctx.get("sweep")
        if sweep is None:
            unevaluated.append({
                "criterion": criterion,
                "reason": "缺少按声明扫描的结果(经 /api/backtest/sweep declared=true 提供)",
            })
            return
        if sweep.get("status") != "evaluated":
            unevaluated.append({
                "criterion": criterion,
                "reason": sweep.get("reason", "扫描结果不可用"),
            })
            return
        if sweep["unstable"]:
            hits.append({
                "criterion": criterion,
                "metric": "annual_return",
                "segment": "full",
                "op": "gt",
                "threshold": 0.5,
                "actual": sweep["better_share"],
                "detail": (
                    f"扫描组合中 {sweep['better_share']:.0%} 优于当前参数"
                    f"(当前年化中位数 {sweep['current']:.2%},"
                    f"扫描中位数 {sweep['median']:.2%})"
                ),
            })
    elif criterion == "capacity_failure":
        trips = ctx["full_metrics"].get("round_trips")
        if trips is None:
            unevaluated.append({
                "criterion": criterion,
                "reason": "回测结果缺少回合统计",
            })
            return
        if trips == 0:
            hits.append({
                "criterion": criterion,
                "metric": "round_trips",
                "segment": "full",
                "op": "lte",
                "threshold": 0,
                "actual": 0,
                "detail": "全区间没有一个完整回合,策略在该股票池无法形成有效持仓",
            })


def _eval_structured_rule(
    rule,
    ctx: dict[str, Any],
    hits: list[dict[str, Any]],
    unevaluated: list[dict[str, Any]],
) -> None:
    label = rule.description or f"{rule.segment}.{rule.metric}"
    if rule.metric == "excess_annual_return_vs_best_baseline":
        view = _segment_view(ctx, rule.segment)
        actual_metric = view.get("annual_return")
        best = _best_baseline_annual(ctx["baselines"], rule.segment)
        actual = (
            round(float(actual_metric) - float(best), 4)
            if actual_metric is not None and best is not None else None
        )
    else:
        value = _segment_view(ctx, rule.segment).get(rule.metric)
        actual = float(value) if isinstance(value, (int, float)) else None
    if actual is None:
        unevaluated.append({
            "criterion": label,
            "reason": f"指标 {rule.metric}({rule.segment} 段)不可得",
        })
        return
    if _compare(actual, rule.op, rule.threshold):
        hits.append({
            "criterion": label,
            "metric": rule.metric,
            "segment": rule.segment,
            "op": rule.op,
            "threshold": rule.threshold,
            "actual": round(actual, 4),
            "detail": (
                f"{rule.segment} 段 {rule.metric} = {actual:.4f} "
                f"命中 {rule.op} {rule.threshold}"
            ),
        })


def evaluate_validation(
    spec: StrategySpec,
    *,
    frames: dict[str, pd.DataFrame],
    start: date,
    end: date,
    equity: pd.Series,
    metrics: dict[str, Any],
    sweep: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """回测完成后的规格验证报告:基线对比 + OOS 分段 + 否决判定。

    ``frames`` 为回测已加载的行情(含预热段,本函数只取 [start, end] 窗口);
    ``equity``/``metrics`` 为本次回测的组合净值与全区间指标。
    """
    baselines = _baseline_results(spec, frames, start, end)
    oos_report = build_oos_report(spec.validation.locked_oos, equity)
    report_baselines: list[dict[str, Any]] = []
    for entry in baselines:
        item = {
            key: value for key, value in entry.items()
            if key not in {"equity", "in_sample_metrics", "oos_metrics"}
        }
        if entry["status"] == "ok":
            item["delta"] = {
                name: _diff(metrics.get(name), entry["metrics"].get(name))
                for name in _BASELINE_DELTA_METRICS
            }
        report_baselines.append(item)
    rejection = evaluate_rejection(
        spec,
        full_metrics=metrics,
        oos_report=oos_report,
        baselines=baselines,
        sweep=sweep,
    )
    return {
        "baselines": report_baselines,
        "oos": oos_report,
        "rejection": rejection,
    }


__all__ = [
    "BASELINE_NAMES", "MIN_OOS_BARS", "OOS_FRACTION", "baseline_equity",
    "build_oos_report", "evaluate_declared_sweep", "evaluate_rejection",
    "evaluate_validation", "segment_metrics", "split_oos",
]
