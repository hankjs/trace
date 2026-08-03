"""因子相关性与正交性检验。

三层判据:因子值截面相关 → IC 序列相关 → 正交化残差 IC。核心结论是残差 IC:
两个因子的裸 IC 都好看不代表有增量,只有对已有因子回归取残差后 IC 仍显著,
才说明带来了新信息。
"""
from __future__ import annotations

import logging
import math
import threading
import time
from datetime import date, datetime, timedelta
from typing import Any

import numpy as np
import pandas as pd
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.validation import validate_backtest_window
from ..models import FactorCorrelation, FactorDef
from .defs import load_enabled_defs
from .evaluation import (
    _check_cancel,
    _ic_series,
    _is_eligible,
    _load_expression_factor_values,
    _load_price_matrix,
    _load_saved_factor_values,
    _neutralize_cross_section,
    _newey_west_tstat,
    _period_market_caps,
    _rebalance_dates,
    _resolve_universe,
    _stocks_for_codes,
    _trading_days,
    normalize_neutralize,
)

logger = logging.getLogger(__name__)

MAX_BENCHMARKS = 20
# 单期截面回归的最小样本:与 _neutralize_cross_section 的门槛一致
MIN_CROSS_SECTION = 5
# |ρ| 超过这个值算「高相关」,用于统计稳定性占比。0.7 是因子研究常用阈值,
# 不是统计定理 —— 结论里必须把原始 ρ 一并给出,不能只报占比。
HIGH_CORR_THRESHOLD = 0.7

_DISCLAIMER = (
    "样本内统计,未扣交易成本,非投资建议;"
    "残差 IC 不显著即相对对照因子无增量,不得因裸 IC 好看而推荐。"
)


class CorrelationNotFoundError(ValueError):
    """不存在或不属于当前用户(统一按不存在处理,防探测)。"""


def _pair_correlation(
    a: np.ndarray, b: np.ndarray,
) -> tuple[float | None, float | None]:
    """单期截面上两个因子值的 (Pearson, Spearman)。样本不足或零方差返回 None。"""
    valid = np.isfinite(a) & np.isfinite(b)
    x, y = a[valid], b[valid]
    if len(x) < MIN_CROSS_SECTION or x.std() == 0 or y.std() == 0:
        return None, None
    pearson = float(np.corrcoef(x, y)[0, 1])
    spearman = float(pd.Series(x).corr(pd.Series(y), method="spearman"))
    if not math.isfinite(pearson) or not math.isfinite(spearman):
        return None, None
    return pearson, spearman


def _orthogonalize(
    target: np.ndarray, benchmarks: list[np.ndarray],
) -> np.ndarray | None:
    """target 对 benchmarks 做截面回归,返回残差。

    无法可靠求解时返回 None(调用方跳过该期),不返回原值 —— 把裸因子当残差
    会直接得出「有增量」的错误结论。
    """
    n = len(target)
    if n < MIN_CROSS_SECTION or not benchmarks:
        return None
    columns: list[np.ndarray] = [np.ones(n)]
    for b in benchmarks:
        if len(b) != n:
            return None
        columns.append(np.asarray(b, dtype=float))
    design = np.column_stack(columns)
    # 行数必须严格大于列数,否则过拟合,残差无意义
    if design.shape[0] <= design.shape[1]:
        return None
    # 设计矩阵奇异(对照因子共线)时跳过,不能退化成裸因子
    try:
        rank = np.linalg.matrix_rank(design)
    except np.linalg.LinAlgError:
        return None
    if rank < design.shape[1]:
        return None
    try:
        coef, *_ = np.linalg.lstsq(design, target, rcond=None)
    except np.linalg.LinAlgError:
        return None
    residual = target - design @ coef
    if not np.all(np.isfinite(residual)):
        return None
    return residual


def _mean_std(vals: list[float]) -> tuple[float | None, float | None]:
    if not vals:
        return None, None
    arr = np.asarray(vals, dtype=float)
    return float(arr.mean()), float(arr.std(ddof=0))


def _verdict(
    *,
    residual_n: int,
    residual_p: float | None,
    residual_ic: float | None,
    raw_ic: float | None,
    skipped: int,
    attempted: int,
) -> tuple[str, str]:
    """服务端判定:不让 agent 自己解读残差 IC。"""
    if residual_n < 6 or (attempted > 0 and skipped > attempted / 2):
        return (
            "inconclusive",
            f"样本不足或跳过过多(n_periods={residual_n}, skipped={skipped}/"
            f"{attempted}),无法判定相对对照因子是否有增量",
        )
    if residual_p is None or residual_p >= 0.05:
        return (
            "no_increment",
            "残差 IC 不显著(p≥0.05 或无法计算),相对对照因子无增量;"
            "即使裸 IC 好看也不得推荐",
        )
    # p < 0.05:要求与裸 IC 同号,避免符号翻转被当成「有增量」
    if (
        residual_ic is not None
        and raw_ic is not None
        and residual_ic * raw_ic > 0
    ):
        return (
            "has_increment",
            "残差 IC 显著(p<0.05)且与裸 IC 同号,相对对照因子仍有增量信息",
        )
    return (
        "inconclusive",
        "残差 IC 显著但与裸 IC 异号或裸 IC 缺失,方向不稳定,暂不下增量结论",
    )


def compute_factor_correlation(
    db: Session,
    *,
    user_id: str,
    expression: dict | None = None,
    factor_key: str | None = None,
    benchmark_keys: list[str] | None = None,
    start: date,
    end: date,
    pool_id: int | None = None,
    codes: list[str] | None = None,
    rebalance: str = "weekly",
    neutralize: list[str] | None = None,
    cancel_event: threading.Event | None = None,
) -> FactorCorrelation:
    """计算相关性与正交性并落库,返回 FactorCorrelation 行。"""
    t0 = time.monotonic()
    if (expression is None) == (factor_key is None):
        raise ValueError("必须且只能提供 expression 或 factor_key 之一")
    if rebalance not in {"weekly", "monthly"}:
        raise ValueError("rebalance 只支持 weekly 或 monthly")
    validate_backtest_window(start, end)
    modes = normalize_neutralize(neutralize)

    notes: list[str] = []
    # 对照因子集
    raw_keys = list(benchmark_keys) if benchmark_keys is not None else None
    if raw_keys is None:
        enabled = load_enabled_defs(db)
        raw_keys = [d.key for d in enabled if d.is_system]
        if len(raw_keys) > MAX_BENCHMARKS:
            raise ValueError(
                f"启用的系统因子共 {len(raw_keys)} 个,超过对照上限 "
                f"{MAX_BENCHMARKS},请显式指定 benchmark_keys"
            )
    # 去重保序
    seen: set[str] = set()
    keys: list[str] = []
    for k in raw_keys:
        k = str(k)
        if k not in seen:
            seen.add(k)
            keys.append(k)
    if factor_key is not None and factor_key in keys:
        keys = [k for k in keys if k != factor_key]
        notes.append(f"对照集中剔除了待检因子自身 {factor_key}")
    if not keys:
        raise ValueError("对照因子集为空,请指定至少 1 个 benchmark_keys")
    if len(keys) > MAX_BENCHMARKS:
        raise ValueError(
            f"对照因子最多 {MAX_BENCHMARKS} 个,收到 {len(keys)} 个"
        )

    # 对照因子必须都存在
    existing = {
        row.key
        for row in db.execute(
            select(FactorDef).where(FactorDef.key.in_(keys))
        ).scalars()
    }
    missing = [k for k in keys if k not in existing]
    if missing:
        raise ValueError(f"对照因子不存在: {', '.join(missing)}")

    eval_codes, universe = _resolve_universe(
        db, user_id=user_id, start=start, end=end,
        pool_id=pool_id, codes=codes,
    )
    if not eval_codes:
        raise ValueError("评估域内没有可用股票")

    expr_hash: str | None = None
    if expression is not None:
        from ..strategy.spec import validate_expression
        from .evaluation import _available_fields

        validation = validate_expression(
            expression, require_type="number",
            available_fields=_available_fields(db),
        )
        if not validation.valid:
            issues = validation.capability.issues
            raise ValueError(
                "表达式校验失败: "
                + "; ".join(i.message for i in issues[:5])
            )
        expr_hash = validation.expression_hash

    row = FactorCorrelation(
        user_id=user_id,
        factor_key=factor_key,
        expression=expression,
        expression_hash=expr_hash,
        benchmark_keys=keys,
        start=start,
        end=end,
        pool_id=pool_id,
        codes=codes,
        rebalance=rebalance,
        neutralize=modes or None,
        universe=universe,
        status="running",
    )
    db.add(row)
    db.commit()
    db.refresh(row)

    try:
        trading_days = _trading_days(db, start, end)
        dates = _rebalance_dates(trading_days, rebalance)
        if len(dates) < 2:
            raise ValueError("调仓日不足 2 个,无法计算前瞻收益")

        lookback_start = start - timedelta(days=200)
        price_matrix = _load_price_matrix(
            db, eval_codes, lookback_start, end,
        )
        stocks = _stocks_for_codes(db, eval_codes)
        all_rebalance_dates = set(dates)

        if factor_key is not None:
            target_values = _load_saved_factor_values(
                db, factor_key, eval_codes, all_rebalance_dates,
            )
        else:
            target_values = _load_expression_factor_values(
                db, expression, eval_codes, lookback_start, end,
                cancel_event,
            )

        bench_values: dict[str, dict[tuple[str, date], float]] = {}
        for bk in keys:
            bench_values[bk] = _load_saved_factor_values(
                db, bk, eval_codes, all_rebalance_dates,
            )

        # 累积序列
        pair_pearson: dict[str, list[float]] = {k: [] for k in keys}
        pair_spearman: dict[str, list[float]] = {k: [] for k in keys}
        target_ics: list[float] = []
        target_rank_ics: list[float] = []
        bench_ics: dict[str, list[float]] = {k: [] for k in keys}
        residual_ics: list[float] = []
        residual_rank_ics: list[float] = []
        skipped_periods = 0
        attempted_periods = 0

        for i, current in enumerate(dates[:-1]):
            _check_cancel(cancel_event)
            nxt = dates[i + 1]

            code_list: list[str] = []
            target_list: list[float] = []
            ret_list: list[float] = []
            bench_lists: dict[str, list[float]] = {k: [] for k in keys}

            for code in eval_codes:
                if not _is_eligible(
                    current, code, price_matrix, stocks, universe["filters"],
                ):
                    continue
                tv = target_values.get((code, current))
                if tv is None:
                    continue
                bv_ok = True
                period_b: dict[str, float] = {}
                for bk in keys:
                    bv = bench_values[bk].get((code, current))
                    if bv is None:
                        bv_ok = False
                        break
                    period_b[bk] = bv
                if not bv_ok:
                    continue
                cur_frame = price_matrix.get(code)
                if cur_frame is None:
                    continue
                try:
                    cur_price = cur_frame.at[current, "close"]
                    next_price = cur_frame.at[nxt, "close"]
                except KeyError:
                    continue
                if pd.isna(next_price) or float(next_price) <= 0:
                    continue
                if pd.isna(cur_price) or float(cur_price) <= 0:
                    continue
                ret = float(next_price) / float(cur_price) - 1
                code_list.append(code)
                target_list.append(float(tv))
                ret_list.append(ret)
                for bk in keys:
                    bench_lists[bk].append(period_b[bk])

            if len(code_list) < MIN_CROSS_SECTION:
                continue
            attempted_periods += 1

            target_arr = np.asarray(target_list, dtype=float)
            rets_arr = np.asarray(ret_list, dtype=float)
            bench_arrs = {
                k: np.asarray(bench_lists[k], dtype=float) for k in keys
            }

            if modes:
                industries = [
                    (stocks[c].industry if c in stocks else "") for c in code_list
                ]
                caps = (
                    _period_market_caps(price_matrix, code_list, current)
                    if "market_cap" in modes else None
                )
                target_arr = _neutralize_cross_section(
                    target_arr, industries=industries,
                    market_caps=caps, modes=modes,
                )
                for k in keys:
                    bench_arrs[k] = _neutralize_cross_section(
                        bench_arrs[k], industries=industries,
                        market_caps=caps, modes=modes,
                    )

            # 因子值相关
            for k in keys:
                p, s = _pair_correlation(target_arr, bench_arrs[k])
                if p is not None:
                    pair_pearson[k].append(p)
                    pair_spearman[k].append(s)

            # 裸 IC
            tic, tric = _ic_series(target_arr, rets_arr)
            if tic is not None:
                target_ics.append(tic)
                target_rank_ics.append(tric)
            for k in keys:
                bic, _ = _ic_series(bench_arrs[k], rets_arr)
                if bic is not None:
                    bench_ics[k].append(bic)

            # 正交化残差 IC
            residual = _orthogonalize(
                target_arr, [bench_arrs[k] for k in keys],
            )
            if residual is None:
                skipped_periods += 1
                continue
            # 先判残差是否实质为零:浮点噪声下 residual.std 可能是 1e-15
            # 而非精确 0,_ic_series 仍可能给出假相关,必须先短路。
            if float(np.nanstd(residual)) < 1e-10:
                residual_ics.append(0.0)
                residual_rank_ics.append(0.0)
                continue
            ric, rric = _ic_series(residual, rets_arr)
            if ric is not None:
                residual_ics.append(ric)
                residual_rank_ics.append(rric)

        pairs: list[dict[str, Any]] = []
        for k in keys:
            p_mean, p_std = _mean_std(pair_pearson[k])
            s_mean, s_std = _mean_std(pair_spearman[k])
            n_p = len(pair_pearson[k])
            high_ratio = (
                sum(1 for v in pair_pearson[k] if abs(v) > HIGH_CORR_THRESHOLD) / n_p
                if n_p else None
            )
            # IC 序列相关:两条 IC 对齐截断到公共长度
            t_ic = target_ics
            b_ic = bench_ics[k]
            ic_corr = None
            if len(t_ic) >= 3 and len(b_ic) >= 3:
                m = min(len(t_ic), len(b_ic))
                pc, _ = _pair_correlation(
                    np.asarray(t_ic[:m], dtype=float),
                    np.asarray(b_ic[:m], dtype=float),
                )
                ic_corr = pc
            pairs.append({
                "factor_key": k,
                "pearson_mean": p_mean,
                "pearson_std": p_std,
                "spearman_mean": s_mean,
                "spearman_std": s_std,
                "high_corr_ratio": high_ratio,
                "n_periods": n_p,
                "ic_correlation": ic_corr,
            })

        residual_arr = np.asarray(residual_ics, dtype=float) if residual_ics else np.array([])
        r_t, r_p = (
            _newey_west_tstat(residual_arr) if len(residual_arr) >= 6
            else (None, None)
        )
        raw_arr = np.asarray(target_ics, dtype=float) if target_ics else np.array([])
        raw_t, raw_p = (
            _newey_west_tstat(raw_arr) if len(raw_arr) >= 6 else (None, None)
        )
        raw_ic_mean = float(raw_arr.mean()) if len(raw_arr) else None
        residual_ic_mean = float(residual_arr.mean()) if len(residual_arr) else None
        residual_rank_mean = (
            float(np.mean(residual_rank_ics)) if residual_rank_ics else None
        )

        verdict, reason = _verdict(
            residual_n=len(residual_ics),
            residual_p=r_p,
            residual_ic=residual_ic_mean,
            raw_ic=raw_ic_mean,
            skipped=skipped_periods,
            attempted=attempted_periods,
        )

        elapsed = time.monotonic() - t0
        result = {
            "pairs": pairs,
            "raw": {
                "ic_mean": raw_ic_mean,
                "ic_t_stat": raw_t,
                "ic_p_value": raw_p,
                "n_periods": len(target_ics),
            },
            "residual": {
                "ic_mean": residual_ic_mean,
                "rank_ic_mean": residual_rank_mean,
                "ic_t_stat": r_t,
                "ic_p_value": r_p,
                "n_periods": len(residual_ics),
                "skipped_periods": skipped_periods,
            },
            "verdict": verdict,
            "verdict_reason": reason,
            "note": "; ".join(notes) if notes else None,
            "elapsed_seconds": round(elapsed, 3),
            "disclaimer": _DISCLAIMER,
            "high_corr_threshold": HIGH_CORR_THRESHOLD,
        }
        row.result = result
        row.status = "done"
        row.finished_at = datetime.now()
        db.commit()
        db.refresh(row)
        return row
    except Exception as exc:
        row.status = "failed"
        row.error = str(exc)[:4000]
        row.finished_at = datetime.now()
        db.commit()
        db.refresh(row)
        raise


def get_correlation(
    db: Session, *, user_id: str, correlation_id: int,
) -> dict[str, Any]:
    """按 id 取本人相关性结果;非本人按不存在处理。"""
    row = db.get(FactorCorrelation, correlation_id)
    if row is None or row.user_id != user_id:
        raise CorrelationNotFoundError(
            f"相关性结果 {correlation_id} 不存在"
        )
    return _correlation_detail(row)


def _correlation_detail(row: FactorCorrelation) -> dict[str, Any]:
    result = row.result or {}
    return {
        "correlation_id": row.id,
        "factor_key": row.factor_key,
        "expression_hash": row.expression_hash,
        "expression": row.expression,
        "benchmark_keys": row.benchmark_keys,
        "window": {
            "start": str(row.start),
            "end": str(row.end),
            "rebalance": row.rebalance,
        },
        "neutralize": row.neutralize,
        "universe": row.universe,
        "pairs": result.get("pairs", []),
        "raw": result.get("raw", {}),
        "residual": result.get("residual", {}),
        "verdict": result.get("verdict"),
        "verdict_reason": result.get("verdict_reason"),
        "note": result.get("note"),
        "elapsed_seconds": result.get("elapsed_seconds"),
        "disclaimer": result.get("disclaimer"),
        "status": row.status,
        "error": row.error,
        "created_at": (
            row.created_at.isoformat(sep=" ") if row.created_at else None
        ),
        "finished_at": (
            row.finished_at.isoformat(sep=" ") if row.finished_at else None
        ),
    }


def build_correlation_artifact(row: FactorCorrelation) -> dict[str, Any]:
    """A2A artifact 形状。"""
    result = row.result or {}
    return {
        "factor_correlation": {
            "correlation_id": row.id,
            "factor_key": row.factor_key,
            "expression_hash": row.expression_hash,
            "benchmark_keys": row.benchmark_keys,
            "window": {
                "start": str(row.start),
                "end": str(row.end),
                "rebalance": row.rebalance,
            },
            "neutralize": row.neutralize,
            "universe": row.universe,
            "pairs": result.get("pairs", []),
            "raw": result.get("raw", {}),
            "residual": result.get("residual", {}),
            "verdict": result.get("verdict"),
            "verdict_reason": result.get("verdict_reason"),
            "note": result.get("note"),
            "elapsed_seconds": result.get("elapsed_seconds"),
            "detail_ref": {"correlation_id": row.id},
            "status": row.status,
            "error": row.error,
            "disclaimer": result.get("disclaimer"),
        }
    }


__all__ = [
    "HIGH_CORR_THRESHOLD",
    "MAX_BENCHMARKS",
    "MIN_CROSS_SECTION",
    "CorrelationNotFoundError",
    "_orthogonalize",
    "_pair_correlation",
    "build_correlation_artifact",
    "compute_factor_correlation",
    "get_correlation",
]
