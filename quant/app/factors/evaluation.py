"""因子有效性评估引擎:IC/RankIC/ICIR、分层收益、多空组合、覆盖率。

只负责确定性计算与落库,不实现 A2A/REST 协议层。
"""
from __future__ import annotations

import logging
import math
import threading
from collections import defaultdict
from datetime import date, timedelta
from typing import Any

import numpy as np
import pandas as pd
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..backtest.engine import TRADING_DAYS
from ..backtest.validation import validate_backtest_window
from ..data.ingest import (
    BAR_FIELDS,
    SNAPSHOT_SPEC_FIELDS,
    load_bars_df,
    load_bars_df_bulk,
    snapshot_available_fields,
)
from ..data.universe import (
    IncompleteListingDataError,
    MissingPoolHistoryError,
    resolve_pool_during,
)
from ..factors.engine import evaluate_factor
from ..models import FactorDaily, FactorDef, FactorEvaluation, Stock, TradeCalendar
from ..strategy.spec import parse_expression, validate_expression, _walk_expression

logger = logging.getLogger(__name__)

MAX_LAYERS = 10
DEFAULT_FILTERS = ["st", "suspended", "lt_60d"]
MIN_LIST_DAYS = 60


class EvaluationCancelledError(Exception):
    """取消事件在检查点被置位时抛出,调用方负责把评估行标为 cancelled。"""


class FactorNotFoundError(ValueError):
    """指定的 factor_key 不存在。"""


def _check_cancel(cancel_event: threading.Event | None) -> None:
    if cancel_event is not None and cancel_event.is_set():
        raise EvaluationCancelledError("已在检查点中断,已完成部分不计入结果")


def _available_fields(db: Session) -> frozenset[str]:
    return BAR_FIELDS | snapshot_available_fields(db)


def _trading_days(db: Session, start: date, end: date) -> list[date]:
    """返回 [start, end] 内的交易日;日历缺失时降级为工作日。"""
    rows = db.execute(
        TradeCalendar.__table__.select()
        .where(
            TradeCalendar.date >= start,
            TradeCalendar.date <= end,
            TradeCalendar.is_open.is_(True),
        )
        .order_by(TradeCalendar.date)
    ).all()
    if rows:
        return [r.date for r in rows]
    days: list[date] = []
    d = start
    while d <= end:
        if d.weekday() < 5:
            days.append(d)
        d += timedelta(days=1)
    return days


def _rebalance_dates(trading_days: list[date], rebalance: str) -> list[date]:
    """按 weekly/monthly 从交易日序列中选出调仓日。"""
    if not trading_days:
        return []
    if rebalance == "weekly":
        # 取每周最后一个交易日(周五或节前最后一个交易日)
        weeks: dict[int, date] = {}
        for d in trading_days:
            weeks[d.isocalendar().week] = d
        return sorted(weeks.values())
    if rebalance == "monthly":
        months: dict[tuple[int, int], date] = {}
        for d in trading_days:
            months[(d.year, d.month)] = d
        return sorted(months.values())
    raise ValueError(f"不支持的 rebalance: {rebalance}")


def _resolve_universe(
    db: Session,
    *,
    user_id: str,
    start: date,
    end: date,
    pool_id: int | None,
    codes: list[str] | None,
) -> tuple[list[str], dict[str, Any]]:
    """解析评估域,返回 (codes, universe_snapshot)。

    codes/pool_id 互斥规则与 _prepare_backtest 一致;缺省 = 全 A 可交易域。
    """
    if codes and pool_id is not None:
        raise ValueError("codes 与 pool_id 只能选其一")
    if codes:
        universe = {"size": len(codes), "filters": []}
        return [c.lower() for c in dict.fromkeys(codes)], universe
    if pool_id is not None:
        # pool 成分取区间并集,具体资格在逐日过滤时处理
        resolved = resolve_pool_during(
            db, start, end, kind="static", pool_id=pool_id,
        )
        universe = {"size": len(resolved), "filters": [], "pool_id": pool_id}
        return [c.lower() for c in resolved], universe

    try:
        resolved = resolve_pool_during(
            db, start, end, kind="all",
            min_list_days=MIN_LIST_DAYS,
        )
    except (IncompleteListingDataError, MissingPoolHistoryError) as exc:
        raise ValueError(str(exc)) from exc
    universe = {"size": len(resolved), "filters": DEFAULT_FILTERS}
    return [c.lower() for c in resolved], universe


def _load_price_matrix(
    db: Session,
    codes: list[str],
    start: date,
    end: date,
) -> dict[str, pd.DataFrame]:
    """批量读取日线,返回 {code: DataFrame indexed by date}。"""
    frames = load_bars_df_bulk(db, codes, start=start, end=end)
    result: dict[str, pd.DataFrame] = {}
    for code, df in frames.items():
        if df.empty:
            continue
        cdf = df.set_index("date")
        result[code] = cdf
    return result


def _load_saved_factor_values(
    db: Session,
    factor_key: str,
    codes: list[str],
    dates: set[date],
) -> dict[tuple[str, date], float]:
    """从 quant_factor_daily 读取已计算的因子值。"""
    if not codes or not dates:
        return {}
    rows = db.execute(
        FactorDaily.__table__.select().where(
            FactorDaily.date.in_(list(dates)),
            FactorDaily.code.in_(codes),
        )
    ).all()
    out: dict[tuple[str, date], float] = {}
    for r in rows:
        value = (r.values or {}).get(factor_key)
        if value is None:
            continue
        try:
            v = float(value)
        except (TypeError, ValueError):
            continue
        if not math.isfinite(v):
            continue
        out[(r.code, r.date)] = v
    return out


def _used_fields(expr: dict) -> set[str]:
    return {
        node.name
        for node in _walk_expression(parse_expression(expr))
        if node.op == "field" and node.name is not None
    }


def _load_expression_factor_values(
    db: Session,
    expression: dict[str, Any],
    codes: list[str],
    lookback_start: date,
    end: date,
    cancel_event: threading.Event | None,
) -> dict[tuple[str, date], float]:
    """逐只计算 ad-hoc 表达式的因子值。"""
    needed = _used_fields(expression)
    extra_fields = sorted(needed & set(SNAPSHOT_SPEC_FIELDS))
    out: dict[tuple[str, date], float] = {}
    for i, code in enumerate(codes):
        if i % 200 == 0:
            _check_cancel(cancel_event)
        df = load_bars_df(
            db, code, start=lookback_start, end=end,
            extra_fields=extra_fields or None,
        )
        if df.empty or len(df) < 2:
            continue
        try:
            series = evaluate_factor(expression, df)
        except Exception:  # noqa: BLE001
            logger.warning("表达式求值失败 %s", code)
            continue
        for _, row in df.iterrows():
            d = row["date"]
            v = series.iloc[_]
            if v is None or pd.isna(v):
                continue
            v = float(v)
            if not math.isfinite(v):
                continue
            out[(code, d)] = v
    return out


def _stocks_for_codes(db: Session, codes: list[str]) -> dict[str, Stock]:
    """批量读取股票元数据。"""
    if not codes:
        return {}
    rows = db.execute(
        select(Stock).where(Stock.code.in_(codes))
    ).scalars().all()
    return {r.code: r for r in rows}


def _is_eligible(
    day: date,
    code: str,
    price_matrix: dict[str, pd.DataFrame],
    stocks: dict[str, Stock],
    universe_filters: list[str],
) -> bool:
    """单日单票是否属于有效域(剔除 ST/停牌/上市不足等)。"""
    frame = price_matrix.get(code)
    if frame is None:
        return False
    try:
        row = frame.loc[day]
    except KeyError:
        return False
    # 停牌判断:无有效 close 或成交量为 0
    if pd.isna(row.get("close")) or float(row["close"]) <= 0:
        return False
    if "suspended" in universe_filters and float(row.get("volume", 0)) <= 0:
        return False
    if "st" in universe_filters:
        is_st = row.get("is_st")
        if is_st is True or is_st == 1:
            return False
    if "lt_60d" in universe_filters:
        stock = stocks.get(code)
        if stock is None or stock.list_date is None:
            return False
        if stock.list_date + timedelta(days=MIN_LIST_DAYS) > day:
            return False
        if stock.delist_date is not None and stock.delist_date <= day:
            return False
    return True


def _ic_series(factors: np.ndarray, returns: np.ndarray) -> tuple[float | None, float | None]:
    """返回 (Pearson IC, Spearman RankIC)。样本过少或标准差为 0 时返回 None。"""
    valid = ~np.isnan(factors) & ~np.isnan(returns)
    x, y = factors[valid], returns[valid]
    if len(x) < 3 or x.std() == 0 or y.std() == 0:
        return None, None
    ic = np.corrcoef(x, y)[0, 1]
    rank_ic = pd.Series(x).corr(pd.Series(y), method="spearman")
    if ic is None or rank_ic is None or not math.isfinite(ic) or not math.isfinite(rank_ic):
        return None, None
    return float(ic), float(rank_ic)


def _annualize(total_return: float, days: int) -> float | None:
    """按自然日年化;天数过短返回 None。"""
    if days <= 0 or total_return is None:
        return None
    try:
        return float((1 + total_return) ** (365 / days) - 1)
    except (ValueError, ZeroDivisionError):
        return None


def _max_drawdown(equity: pd.Series) -> float:
    peak = equity.cummax()
    dd = equity / peak - 1
    return float(dd.min())


def evaluate_factor_efficacy(
    db: Session,
    *,
    user_id: str,
    expression: dict[str, Any] | None = None,
    factor_key: str | None = None,
    start: date,
    end: date,
    pool_id: int | None = None,
    codes: list[str] | None = None,
    layers: int = 10,
    rebalance: str = "weekly",
    cancel_event: threading.Event | None = None,
) -> FactorEvaluation:
    """评估因子有效性并落库,返回 FactorEvaluation 行。

    取消检查点:按标的批次(每 200 只)与每个调仓日截面检查 cancel_event,
    置位则抛出 EvaluationCancelledError,调用方负责将行标 cancelled 且不写 result。
    """
    if (expression is None) == (factor_key is None):
        raise ValueError("必须且只能提供 expression 或 factor_key 之一")
    if rebalance not in {"weekly", "monthly"}:
        raise ValueError("rebalance 只支持 weekly 或 monthly")
    validate_backtest_window(start, end)
    layers = max(1, min(int(layers), MAX_LAYERS))

    # 解析评估域
    eval_codes, universe = _resolve_universe(
        db, user_id=user_id, start=start, end=end,
        pool_id=pool_id, codes=codes,
    )
    if not eval_codes:
        raise ValueError("评估域内没有可用股票")

    # ad-hoc 表达式先校验
    expr_hash: str | None = None
    lookback_start = start - timedelta(days=200)
    if expression is not None:
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
        min_bars = validation.min_bars or 1
        lookback_start = start - timedelta(days=max(min_bars * 2, 200))

    # 预先落库运行中状态,便于 A2A/REST 查询进度
    row = FactorEvaluation(
        user_id=user_id,
        factor_key=factor_key,
        expression=expression,
        expression_hash=expr_hash,
        start=start,
        end=end,
        pool_id=pool_id,
        codes=eval_codes if codes else None,
        layers=layers,
        rebalance=rebalance,
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
            raise ValueError("区间内调仓点不足,无法计算 IC")

        # 统一价格矩阵(含预热段,避免表达式求值缺历史)
        price_start = min(lookback_start, start)
        price_matrix = _load_price_matrix(db, eval_codes, price_start, end)
        stocks = _stocks_for_codes(db, eval_codes)

        # 读取/计算因子值
        all_rebalance_dates = set(dates)
        if factor_key is not None:
            factor_values = _load_saved_factor_values(
                db, factor_key, eval_codes, all_rebalance_dates,
            )
        else:
            factor_values = _load_expression_factor_values(
                db, expression, eval_codes, lookback_start, end,
                cancel_event,
            )

        # 逐期计算
        period_ics: list[float] = []
        period_rank_ics: list[float] = []
        layer_period_returns: list[dict[int, float]] = []
        full_sample_returns: list[float] = []
        eligible_obs = 0
        valid_obs = 0

        for i, current in enumerate(dates[:-1]):
            _check_cancel(cancel_event)
            nxt = dates[i + 1]

            period_factors: list[float] = []
            period_returns: list[float] = []
            code_list: list[str] = []

            for code in eval_codes:
                if not _is_eligible(
                    current, code, price_matrix, stocks, universe["filters"],
                ):
                    continue
                eligible_obs += 1
                f = factor_values.get((code, current))
                if f is None:
                    continue
                valid_obs += 1
                cur_frame = price_matrix.get(code)
                cur_price = cur_frame.at[current, "close"]
                try:
                    next_price = cur_frame.at[nxt, "close"]
                except KeyError:
                    continue
                if pd.isna(next_price) or float(next_price) <= 0:
                    continue
                ret = float(next_price) / float(cur_price) - 1
                period_factors.append(f)
                period_returns.append(ret)
                code_list.append(code)

            if len(period_factors) < layers * 2:
                continue

            factors_arr = np.array(period_factors, dtype=float)
            rets_arr = np.array(period_returns, dtype=float)
            ic, rank_ic = _ic_series(factors_arr, rets_arr)
            if ic is not None:
                period_ics.append(ic)
                period_rank_ics.append(rank_ic)

            full_sample_returns.append(float(np.mean(rets_arr)))

            # 分层
            order = np.argsort(factors_arr)
            layer_size = max(1, len(order) // layers)
            layer_rets: dict[int, float] = {}
            for layer in range(layers):
                start_idx = layer * layer_size
                if layer == layers - 1:
                    end_idx = len(order)
                else:
                    end_idx = (layer + 1) * layer_size
                idx = order[start_idx:end_idx]
                layer_rets[layer + 1] = float(np.mean(rets_arr[idx]))
            layer_period_returns.append(layer_rets)

        if not period_ics:
            raise ValueError("有效样本过少,无法计算 IC")

        ic_arr = np.array(period_ics)
        rank_arr = np.array(period_rank_ics)
        ic_mean = float(ic_arr.mean())
        ic_std = float(ic_arr.std())
        icir = ic_mean / ic_std if ic_std > 0 else None
        rank_ic_mean = float(rank_arr.mean())
        rank_ic_std = float(rank_arr.std())
        rank_icir = rank_ic_mean / rank_ic_std if rank_ic_std > 0 else None
        positive_ratio = float((ic_arr > 0).sum() / len(ic_arr))

        # 分层收益
        layer_equities: dict[int, list[float]] = {l: [1.0] for l in range(1, layers + 1)}
        full_equity = [1.0]
        for rets in layer_period_returns:
            full_equity.append(full_equity[-1] * (1 + full_sample_returns[len(full_equity) - 1]))
            for l in range(1, layers + 1):
                layer_equities[l].append(layer_equities[l][-1] * (1 + rets[l]))

        total_days = max(1, (dates[-1] - dates[0]).days)
        layer_summary: list[dict[str, Any]] = []
        for l in range(1, layers + 1):
            eq = layer_equities[l]
            total_ret = eq[-1] / eq[0] - 1
            annual = _annualize(total_ret, total_days)
            excess = (eq[-1] / full_equity[-1]) - 1 if full_equity[-1] else None
            layer_summary.append({
                "layer": l,
                "total_return": round(total_ret, 4),
                "annual_return": None if annual is None else round(annual, 4),
                "excess": None if excess is None else round(excess, 4),
            })

        # 多空:顶-底
        top_eq = pd.Series(layer_equities[layers], index=range(len(layer_equities[layers])))
        bottom_eq = pd.Series(layer_equities[1], index=range(len(layer_equities[1])))
        ls_eq = top_eq - bottom_eq + 1.0  # 净值起点 1
        ls_total = ls_eq.iloc[-1] / ls_eq.iloc[0] - 1
        ls_annual = _annualize(ls_total, total_days)
        ls_max_dd = _max_drawdown(ls_eq)

        # 换手率:顶底组合等权,统计每期间权重变化
        turnovers: list[float] = []
        prev_top: set[str] = set()
        prev_bottom: set[str] = set()
        for i, current in enumerate(dates[:-1]):
            if i >= len(layer_period_returns):
                break
            # 重建当期顶底 code 集合
            # 需要按当期因子排序重新计算,这里与上面分层逻辑保持一致
            # 为了可读性与性能,重算一次当期排序(数据量不大)
            # 也可在循环中缓存,当前实现优先简洁
            factors_snapshot = []
            codes_snapshot = []
            for code in eval_codes:
                if not _is_eligible(
                    current, code, price_matrix, stocks, universe["filters"],
                ):
                    continue
                f = factor_values.get((code, current))
                if f is None:
                    continue
                factors_snapshot.append(f)
                codes_snapshot.append(code)
            if len(factors_snapshot) < layers * 2:
                continue
            order = np.argsort(np.array(factors_snapshot))
            layer_size = max(1, len(order) // layers)
            top_idx = order[-layer_size:]
            bottom_idx = order[:layer_size]
            top_set = {codes_snapshot[j] for j in top_idx}
            bottom_set = {codes_snapshot[j] for j in bottom_idx}
            if i > 0:
                # 等权组合,每边权重和为 0.5(长) / -0.5(短)
                # turnover = sum(|delta w|) / 2
                union = prev_top | prev_bottom | top_set | bottom_set
                turnover = 0.0
                for code in union:
                    old_w = 0.0
                    if code in prev_top:
                        old_w += 0.5 / len(prev_top)
                    elif code in prev_bottom:
                        old_w -= 0.5 / len(prev_bottom)
                    new_w = 0.0
                    if code in top_set:
                        new_w += 0.5 / len(top_set)
                    elif code in bottom_set:
                        new_w -= 0.5 / len(bottom_set)
                    turnover += abs(new_w - old_w)
                turnovers.append(turnover / 2)
            prev_top = top_set
            prev_bottom = bottom_set

        avg_turnover = float(np.mean(turnovers)) if turnovers else 0.0
        periods_per_year = len(dates[:-1]) / max(1, total_days / 365)
        turnover_annual = round(avg_turnover * periods_per_year, 4)

        # 覆盖率
        coverage_ratio = valid_obs / eligible_obs if eligible_obs else 0.0

        result = {
            "ic": {
                "ic_mean": round(ic_mean, 4),
                "icir": None if icir is None else round(icir, 4),
                "rank_ic_mean": round(rank_ic_mean, 4),
                "rank_icir": None if rank_icir is None else round(rank_icir, 4),
                "positive_ratio": round(positive_ratio, 4),
                "n_periods": len(period_ics),
            },
            "layers": layer_summary,
            "long_short": {
                "annual_return": None if ls_annual is None else round(ls_annual, 4),
                "max_drawdown": round(ls_max_dd, 4),
                "turnover": turnover_annual,
            },
            "coverage": {
                "factor_value_ratio": round(coverage_ratio, 4),
                "eligible_observations": eligible_obs,
                "valid_observations": valid_obs,
                "notes": ["历史样本内统计,不含未来收益保证"],
            },
            "window": {
                "start": str(start),
                "end": str(end),
                "rebalance": rebalance,
            },
        }

        row.status = "done"
        row.result = result
        row.finished_at = datetime.now()
        db.commit()
        db.refresh(row)
        return row

    except EvaluationCancelledError:
        row.status = "cancelled"
        row.error = "已在检查点中断,已完成部分不计入结果"
        row.finished_at = datetime.now()
        db.commit()
        db.refresh(row)
        raise
    except Exception as exc:
        row.status = "failed"
        row.error = str(exc)[:4000]
        row.finished_at = datetime.now()
        db.commit()
        db.refresh(row)
        raise


# 避免循环引用:datetime 在函数内导入
from datetime import datetime  # noqa: E402


__all__ = [
    "EvaluationCancelledError",
    "FactorNotFoundError",
    "evaluate_factor_efficacy",
]
