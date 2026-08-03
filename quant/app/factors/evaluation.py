"""因子有效性评估引擎:IC/RankIC/ICIR、分层收益、多空组合、覆盖率。

只负责确定性计算与落库,不实现 A2A/REST 协议层。
"""
from __future__ import annotations

import logging
import math
import threading
from collections import defaultdict
from datetime import date, datetime, timedelta
from typing import Any

import numpy as np
import pandas as pd
from sqlalchemy import func, select
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
from ..factors.engine import evaluate_factor, evaluate_factor_cross_section
from ..factors.defs import load_all_defs
from ..models import FactorDaily, FactorDef, FactorEvaluation, Stock, TradeCalendar
from ..strategy.multiple_testing import factor_multiplicity_report
from ..strategy.operators import CROSS_SECTION_OPS
from ..strategy.spec import (
    expression_mode,
    parse_expression,
    validate_expression,
    _walk_expression,
)

logger = logging.getLogger(__name__)

MAX_LAYERS = 10
DEFAULT_FILTERS = ["st", "suspended", "lt_60d"]
MIN_LIST_DAYS = 60

# 支持的中性化维度。industry 取 quant_stock.industry(当前状态,见下方说明),
# market_cap 取估值快照的 total_market_cap(point-in-time)。
NEUTRALIZE_MODES = ("industry", "market_cap")

# 前瞻期(交易日)上限与默认值。horizons 用于 IC 衰减曲线:
# 同一因子在不同持有期的 IC 变化决定它该配什么调仓频率。
MAX_HORIZON_DAYS = 60
MAX_HORIZONS = 6
DEFAULT_HORIZONS = (1, 5, 10, 20)

# 多重检验回溯窗口:统计同一账号近期评估次数作为试验次数下界
MULTIPLICITY_LOOKBACK_DAYS = 30


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
    *,
    extra_fields: list[str] | None = None,
) -> dict[str, pd.DataFrame]:
    """批量读取日线,返回 {code: DataFrame indexed by date}。

    extra_fields 走快照 PIT 合并(如市值中性化需要的 market_cap)。
    """
    frames = load_bars_df_bulk(
        db, codes, start=start, end=end, extra_fields=extra_fields,
    )
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


def _load_cross_section_factor_values(
    db: Session,
    expression: dict[str, Any],
    codes: list[str],
    rebalance_dates: list[date],
    lookback_start: date,
    end: date,
    stocks: dict[str, Stock],
    cancel_event: threading.Event | None,
    min_bars: int = 1,
) -> dict[tuple[str, date], float]:
    """只在调仓日按全池截面现算因子值。

    截面因子不落 FactorDaily(依赖当期全池),评估时直接按调仓点求值。
    每次只构造该调仓日所需回看窗口,峰值内存与全区间无关。
    """
    needed = _used_fields(expression)
    extra_fields = sorted(needed & set(SNAPSHOT_SPEC_FIELDS))
    industries = {
        code: (stocks[code].industry or "")
        for code in codes
        if code in stocks
    }
    out: dict[tuple[str, date], float] = {}
    window_days = max(min_bars * 2, 60)

    for day in rebalance_dates:
        _check_cancel(cancel_event)
        # 回看窗口:自然日近似,load_bars_df_bulk 会按 date 过滤
        win_start = day - timedelta(days=window_days * 2)
        if win_start < lookback_start:
            win_start = lookback_start
        frames = load_bars_df_bulk(
            db, codes, start=win_start, end=day,
            extra_fields=extra_fields or None,
        )
        if not frames:
            continue
        try:
            cs_frame = evaluate_factor_cross_section(
                expression, frames, industries=industries,
            )
        except Exception:  # noqa: BLE001
            logger.warning("截面因子求值失败 day=%s", day)
            continue
        # 取该调仓日那一行
        day_ts = pd.Timestamp(day)
        if day_ts not in cs_frame.index:
            # 尝试按 date 归一化匹配
            matched = [i for i in cs_frame.index if pd.Timestamp(i).date() == day]
            if not matched:
                continue
            day_ts = matched[0]
        row = cs_frame.loc[day_ts]
        for code, value in row.items():
            if value is None or pd.isna(value):
                continue
            try:
                v = float(value)
            except (TypeError, ValueError):
                continue
            if not math.isfinite(v):
                continue
            out[(str(code), day)] = v
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


def _neutralize_cross_section(
    values: np.ndarray,
    *,
    industries: list[str],
    market_caps: np.ndarray | None,
    modes: list[str],
) -> np.ndarray:
    """对单期截面因子值做中性化,返回残差。

    行业以哑变量进入(丢弃一列避免与截距共线),市值取 log 后进入。
    最小二乘解不出来(奇异矩阵/样本过少)时原样返回,由调用方按未中性化处理 ——
    宁可少一层处理,也不能拿 NaN 冒充残差。
    """
    n = len(values)
    if n < 5 or not modes:
        return values

    columns: list[np.ndarray] = [np.ones(n)]
    if "industry" in modes:
        # 空行业单独归一类,避免全部塞进基准组导致行业效应残留
        labels = [ind or "__unknown__" for ind in industries]
        uniq = sorted(set(labels))
        if len(uniq) > 1:
            # 丢弃第一类作基准组
            for name in uniq[1:]:
                columns.append(
                    np.fromiter((1.0 if l == name else 0.0 for l in labels),
                                dtype=float, count=n)
                )
    if "market_cap" in modes and market_caps is not None:
        caps = np.asarray(market_caps, dtype=float)
        # 市值缺失或非正时用截面中位数补,log 化压缩量级
        positive = caps[np.isfinite(caps) & (caps > 0)]
        if len(positive) >= 3:
            fill = float(np.median(positive))
            caps = np.where(np.isfinite(caps) & (caps > 0), caps, fill)
            columns.append(np.log(caps))

    if len(columns) <= 1:
        # 除截距外没有任何有效解释变量,中性化退化为去均值,无意义
        return values

    design = np.column_stack(columns)
    if design.shape[0] <= design.shape[1]:
        return values
    try:
        coef, *_ = np.linalg.lstsq(design, values, rcond=None)
    except np.linalg.LinAlgError:
        return values
    residual = values - design @ coef
    if not np.all(np.isfinite(residual)):
        return values
    return residual


def _newey_west_tstat(series: np.ndarray) -> tuple[float | None, float | None]:
    """IC 序列均值的 Newey-West t 值与双尾 p 值。

    IC 序列有自相关(尤其是 horizon 跨越多个调仓间隔时),普通 t 检验会高估
    显著性。滞后阶数按 Newey-West 常用经验值 floor(4*(n/100)^(2/9))。
    p 值用正态近似(样本量小时略偏乐观,已在 disclaimer 里声明)。
    """
    n = len(series)
    if n < 6:
        return None, None
    mean = float(series.mean())
    demeaned = series - mean
    gamma0 = float(np.dot(demeaned, demeaned) / n)
    if gamma0 <= 0:
        return None, None

    max_lag = int(math.floor(4 * (n / 100) ** (2 / 9)))
    max_lag = max(0, min(max_lag, n - 2))
    variance = gamma0
    for lag in range(1, max_lag + 1):
        gamma = float(np.dot(demeaned[lag:], demeaned[:-lag]) / n)
        weight = 1 - lag / (max_lag + 1)
        variance += 2 * weight * gamma
    if variance <= 0:
        return None, None

    se = math.sqrt(variance / n)
    if se <= 0:
        return None, None
    t_stat = mean / se
    if not math.isfinite(t_stat):
        return None, None
    # 双尾 p:正态近似,erfc(|t|/sqrt(2))
    p_value = math.erfc(abs(t_stat) / math.sqrt(2))
    return float(t_stat), float(p_value)


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


def _period_market_caps(
    price_matrix: dict[str, pd.DataFrame],
    code_list: list[str],
    day: date,
) -> np.ndarray:
    """取当期截面的 PIT 总市值;缺失记 NaN 由中性化侧按中位数补。"""
    caps: list[float] = []
    for code in code_list:
        frame = price_matrix.get(code)
        value = float("nan")
        if frame is not None and "market_cap" in frame.columns:
            try:
                raw = frame.at[day, "market_cap"]
            except KeyError:
                raw = None
            if raw is not None and not pd.isna(raw):
                value = float(raw)
        caps.append(value)
    return np.array(caps, dtype=float)


def _horizon_returns(
    price_matrix: dict[str, pd.DataFrame],
    code_list: list[str],
    current: date,
    horizon: int,
    trading_days: list[date],
    day_index: dict[date, int],
) -> np.ndarray:
    """当期截面向前 horizon 个交易日的收益;取不到未来价则记 NaN。

    末尾若不足 horizon 个交易日,整期为 NaN,该期不进入对应 horizon 的 IC 序列 ——
    长 horizon 的 n_periods 天然少于短 horizon,这是样本事实,不做补齐。
    """
    base = day_index.get(current)
    out: list[float] = []
    target: date | None = None
    if base is not None and base + horizon < len(trading_days):
        target = trading_days[base + horizon]
    for code in code_list:
        value = float("nan")
        frame = price_matrix.get(code)
        if target is not None and frame is not None:
            try:
                cur_price = frame.at[current, "close"]
                fut_price = frame.at[target, "close"]
            except KeyError:
                cur_price = fut_price = None
            if (
                cur_price is not None and fut_price is not None
                and not pd.isna(cur_price) and not pd.isna(fut_price)
                and float(cur_price) > 0 and float(fut_price) > 0
            ):
                value = float(fut_price) / float(cur_price) - 1
        out.append(value)
    return np.array(out, dtype=float)


def _count_prior_evaluations(
    db: Session, *, user_id: str, exclude_id: int | None,
) -> int:
    """同一用户近期已完成的评估数,作为多重检验的试验次数下界。

    只数 done:失败/取消的评估没有产出可挑选的指标,不构成「多看了一眼」。
    """
    since = datetime.now() - timedelta(days=MULTIPLICITY_LOOKBACK_DAYS)
    q = select(func.count(FactorEvaluation.id)).where(
        FactorEvaluation.user_id == user_id,
        FactorEvaluation.status == "done",
        FactorEvaluation.created_at >= since,
    )
    if exclude_id is not None:
        q = q.where(FactorEvaluation.id != exclude_id)
    return int(db.execute(q).scalar() or 0)


def _best_p_value(
    primary_p: float | None, ic_decay: list[dict[str, Any]],
) -> float | None:
    """本次评估里最小的 p 值(最容易被当成结论的那个)。"""
    candidates = [p for p in [primary_p] if p is not None]
    candidates.extend(
        item["ic_p_value"] for item in ic_decay
        if item.get("ic_p_value") is not None
    )
    return min(candidates) if candidates else None


def _build_ic_decay(
    horizon_days: list[int],
    horizon_ics: dict[int, list[float]],
    horizon_rank_ics: dict[int, list[float]],
) -> list[dict[str, Any]]:
    """组装 IC 衰减曲线;每个 horizon 带自己的 t 值与样本数。"""
    items: list[dict[str, Any]] = []
    for h in horizon_days:
        series = np.array(horizon_ics.get(h) or [], dtype=float)
        rank_series = np.array(horizon_rank_ics.get(h) or [], dtype=float)
        if len(series) == 0:
            items.append({
                "horizon_days": h,
                "ic_mean": None,
                "rank_ic_mean": None,
                "icir": None,
                "ic_t_stat": None,
                "ic_p_value": None,
                "n_periods": 0,
            })
            continue
        mean = float(series.mean())
        std = float(series.std())
        t_stat, p_value = _newey_west_tstat(series)
        items.append({
            "horizon_days": h,
            "ic_mean": round(mean, 4),
            "rank_ic_mean": (
                round(float(rank_series.mean()), 4) if len(rank_series) else None
            ),
            "icir": None if std <= 0 else round(mean / std, 4),
            "ic_t_stat": None if t_stat is None else round(t_stat, 4),
            "ic_p_value": None if p_value is None else round(p_value, 6),
            "n_periods": int(len(series)),
        })
    return items


def normalize_neutralize(neutralize: list[str] | None) -> list[str]:
    """校验并去重中性化维度;非法值直接报错而不是静默忽略。"""
    if not neutralize:
        return []
    if isinstance(neutralize, str):
        neutralize = [neutralize]
    modes: list[str] = []
    for item in neutralize:
        mode = str(item).strip().lower()
        if mode not in NEUTRALIZE_MODES:
            raise ValueError(
                f"不支持的 neutralize 维度 {item!r};可用值: {list(NEUTRALIZE_MODES)}"
            )
        if mode not in modes:
            modes.append(mode)
    return modes


def normalize_horizons(horizons: list[int] | None) -> list[int]:
    """校验前瞻期列表;越界直接报错,避免 agent 以为跑了 250 日其实被截断。"""
    if horizons is None:
        return list(DEFAULT_HORIZONS)
    if isinstance(horizons, int):
        horizons = [horizons]
    if not horizons:
        return []
    out: list[int] = []
    for item in horizons:
        try:
            days = int(item)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"horizons 必须是整数交易日,收到 {item!r}") from exc
        if days < 1 or days > MAX_HORIZON_DAYS:
            raise ValueError(
                f"horizon {days} 越界,只支持 1..{MAX_HORIZON_DAYS} 个交易日"
            )
        if days not in out:
            out.append(days)
    if len(out) > MAX_HORIZONS:
        raise ValueError(f"horizons 最多 {MAX_HORIZONS} 个,收到 {len(out)} 个")
    return sorted(out)


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
    neutralize: list[str] | None = None,
    horizons: list[int] | None = None,
    cancel_event: threading.Event | None = None,
) -> FactorEvaluation:
    """评估因子有效性并落库,返回 FactorEvaluation 行。

    neutralize: 截面中性化维度子集,取值见 NEUTRALIZE_MODES。开启后 IC、分层与
    多空全部基于残差因子计算 —— 裸 IC 里混着行业与市值暴露,低 PE 类因子的
    IC 往往主要来自行业效应,不中性化会把行业 beta 误读成 alpha。
    horizons: 前瞻期(交易日)列表,用于 IC 衰减曲线;主 IC 仍按调仓间隔计算。

    取消检查点:按标的批次(每 200 只)与每个调仓日截面检查 cancel_event,
    置位则抛出 EvaluationCancelledError,调用方负责将行标 cancelled 且不写 result。
    """
    if (expression is None) == (factor_key is None):
        raise ValueError("必须且只能提供 expression 或 factor_key 之一")
    if rebalance not in {"weekly", "monthly"}:
        raise ValueError("rebalance 只支持 weekly 或 monthly")
    validate_backtest_window(start, end)
    layers = max(1, min(int(layers), MAX_LAYERS))

    modes = normalize_neutralize(neutralize)
    horizon_days = normalize_horizons(horizons)

    # 解析评估域
    eval_codes, universe = _resolve_universe(
        db, user_id=user_id, start=start, end=end,
        pool_id=pool_id, codes=codes,
    )
    if not eval_codes:
        raise ValueError("评估域内没有可用股票")

    # ad-hoc 表达式先校验;factor_key 路径也需要 expression 以判定截面模式
    expr_hash: str | None = None
    min_bars = 1
    lookback_start = start - timedelta(days=200)
    resolved_expression: dict[str, Any] | None = expression
    is_cross_section = False
    group_by_used: str | None = None

    if factor_key is not None:
        # factor_key 可只对应日值表键(无 FactorDef 行),此时按时序读库路径走;
        # 有定义时再看表达式模式以决定截面现算 vs 读库。
        defs = {d.key: d for d in load_all_defs(db)}
        snap = defs.get(factor_key)
        if snap is not None:
            resolved_expression = dict(snap.expression or {})
            expr_hash = snap.expression_hash
            min_bars = snap.min_bars or 1
            lookback_start = start - timedelta(days=max(min_bars * 2, 200))
        else:
            resolved_expression = None
    elif expression is not None:
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
        resolved_expression = expression

    if resolved_expression is not None:
        parsed_for_mode = parse_expression(resolved_expression)
        is_cross_section = expression_mode(parsed_for_mode) == "cross_section"
        for node in _walk_expression(parsed_for_mode):
            if node.op in CROSS_SECTION_OPS and node.group_by:
                group_by_used = node.group_by
                break

    # 预先落库运行中状态,便于 A2A/REST 查询进度
    row = FactorEvaluation(
        user_id=user_id,
        factor_key=factor_key,
        expression=expression if expression is not None else resolved_expression,
        expression_hash=expr_hash,
        start=start,
        end=end,
        pool_id=pool_id,
        codes=eval_codes if codes else None,
        layers=layers,
        rebalance=rebalance,
        neutralize=modes or None,
        horizons=horizon_days or None,
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
        day_index = {d: i for i, d in enumerate(trading_days)}

        # 统一价格矩阵(含预热段,避免表达式求值缺历史)。
        # 市值中性化需要 PIT 总市值,一并挂到价格帧上。
        price_start = min(lookback_start, start)
        price_extra = ["market_cap"] if "market_cap" in modes else None
        price_matrix = _load_price_matrix(
            db, eval_codes, price_start, end, extra_fields=price_extra,
        )
        stocks = _stocks_for_codes(db, eval_codes)

        # 读取/计算因子值:截面因子不落日值表,按调仓日全池现算
        all_rebalance_dates = set(dates)
        if is_cross_section:
            assert resolved_expression is not None
            factor_values = _load_cross_section_factor_values(
                db, resolved_expression, eval_codes, dates,
                lookback_start, end, stocks, cancel_event,
                min_bars=min_bars,
            )
        elif factor_key is not None:
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
        # 顶/底组合成分按期留存,供换手率计算复用(必须与分层同一份因子值,
        # 否则中性化后会出现「分层用残差、换手用裸值」的错位)
        period_extremes: list[tuple[set[str], set[str]]] = []
        horizon_ics: dict[int, list[float]] = {h: [] for h in horizon_days}
        horizon_rank_ics: dict[int, list[float]] = {h: [] for h in horizon_days}
        neutralized_periods = 0
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

            if modes:
                residual = _neutralize_cross_section(
                    factors_arr,
                    industries=[
                        (stocks[c].industry if c in stocks else "") for c in code_list
                    ],
                    market_caps=(
                        _period_market_caps(price_matrix, code_list, current)
                        if "market_cap" in modes else None
                    ),
                    modes=modes,
                )
                if residual is not factors_arr:
                    neutralized_periods += 1
                factors_arr = residual

            ic, rank_ic = _ic_series(factors_arr, rets_arr)
            if ic is not None:
                period_ics.append(ic)
                period_rank_ics.append(rank_ic)

            # IC 衰减:同一截面因子对多个前瞻期的预测力
            for h in horizon_days:
                fwd = _horizon_returns(
                    price_matrix, code_list, current, h, trading_days, day_index,
                )
                h_ic, h_rank_ic = _ic_series(factors_arr, fwd)
                if h_ic is not None:
                    horizon_ics[h].append(h_ic)
                    horizon_rank_ics[h].append(h_rank_ic)

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
            # 顶/底成分与分层同源,换手率直接复用,不再二次排序
            period_extremes.append((
                {code_list[j] for j in order[-layer_size:]},
                {code_list[j] for j in order[:layer_size]},
            ))

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

        # 显著性:IC 序列均值的 Newey-West t 值(IC 有自相关,普通 t 会高估)
        ic_t, ic_p = _newey_west_tstat(ic_arr)
        rank_ic_t, rank_ic_p = _newey_west_tstat(rank_arr)
        # ICIR 年化:按调仓频率折算,便于跨频率横向比较
        periods_per_year_nominal = 52.0 if rebalance == "weekly" else 12.0
        icir_annual = (
            None if icir is None else icir * math.sqrt(periods_per_year_nominal)
        )

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

        # 换手率:顶底组合等权,统计相邻期权重变化。成分直接取主循环留存的
        # period_extremes,保证与分层收益基于同一份(可能已中性化的)因子值。
        turnovers: list[float] = []
        for idx in range(1, len(period_extremes)):
            prev_top, prev_bottom = period_extremes[idx - 1]
            top_set, bottom_set = period_extremes[idx]
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

        avg_turnover = float(np.mean(turnovers)) if turnovers else 0.0
        periods_per_year = len(dates[:-1]) / max(1, total_days / 365)
        turnover_annual = round(avg_turnover * periods_per_year, 4)

        # 覆盖率
        coverage_ratio = valid_obs / eligible_obs if eligible_obs else 0.0
        ic_decay = _build_ic_decay(horizon_days, horizon_ics, horizon_rank_ics)

        result = {
            "ic": {
                "ic_mean": round(ic_mean, 4),
                "icir": None if icir is None else round(icir, 4),
                "icir_annual": None if icir_annual is None else round(icir_annual, 4),
                "rank_ic_mean": round(rank_ic_mean, 4),
                "rank_icir": None if rank_icir is None else round(rank_icir, 4),
                "positive_ratio": round(positive_ratio, 4),
                "n_periods": len(period_ics),
                "ic_t_stat": None if ic_t is None else round(ic_t, 4),
                "ic_p_value": None if ic_p is None else round(ic_p, 6),
                "rank_ic_t_stat": None if rank_ic_t is None else round(rank_ic_t, 4),
                "rank_ic_p_value": None if rank_ic_p is None else round(rank_ic_p, 6),
                "t_stat_method": "newey_west_normal_approx",
            },
            "ic_decay": ic_decay,
            "multiplicity": factor_multiplicity_report(
                n_prior_evaluations=_count_prior_evaluations(
                    db, user_id=user_id, exclude_id=row.id,
                ),
                n_horizons=len(horizon_days),
                best_p_value=_best_p_value(ic_p, ic_decay),
                lookback_days=MULTIPLICITY_LOOKBACK_DAYS,
            ),
            "neutralization": {
                "modes": modes,
                "applied_periods": neutralized_periods,
                "total_periods": len(period_ics),
                "note": (
                    "未中性化:IC 含行业与市值暴露"
                    if not modes
                    else "IC/分层/多空均基于截面回归残差"
                ),
            },
            "cross_section": {
                "is_cross_section": is_cross_section,
                "group_by": group_by_used,
                "note": (
                    "截面因子按调仓日全池现算,不经因子日值表;"
                    "行业分组用当前行业(非 PIT),历史分组存在轻微前视"
                    if is_cross_section else None
                ),
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


# 下划线前缀的名字同样导出:correlation.py 需要与本模块**逐字一致**的截面
# 取样与统计口径。复制一份实现会让两处的 IC 定义悄悄漂移,那比暴露私有名危险。
__all__ = [
    "DEFAULT_HORIZONS",
    "MAX_HORIZONS",
    "MAX_HORIZON_DAYS",
    "NEUTRALIZE_MODES",
    "EvaluationCancelledError",
    "FactorNotFoundError",
    "evaluate_factor_efficacy",
    "normalize_horizons",
    "normalize_neutralize",
    "_check_cancel",
    "_ic_series",
    "_is_eligible",
    "_load_expression_factor_values",
    "_load_price_matrix",
    "_load_saved_factor_values",
    "_neutralize_cross_section",
    "_newey_west_tstat",
    "_period_market_caps",
    "_rebalance_dates",
    "_resolve_universe",
    "_stocks_for_codes",
    "_trading_days",
]
