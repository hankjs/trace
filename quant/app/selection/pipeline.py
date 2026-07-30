"""选股 pipeline:每日对股票池过滤 + 打分,产出 Top N 候选池落 quant_pick。

配置化后,所有权重/过滤/确认项均来自 quant_selection_config 的 active 行,
因子集合来自 quant_factor_def 的启用定义。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

import pandas as pd
from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from ..data.clock import today_cst
from ..data.ingest import SNAPSHOT_SPEC_FIELDS, load_bars_df_bulk
from ..data.universe import current_pool, pool_at
from ..factors import evaluate_def_last, load_enabled_defs
from ..models import FactorDaily, Pick, Stock
from ..strategy.spec import _walk_expression, parse_expression
from .config import load_selection_config

logger = logging.getLogger(__name__)

LOOKBACK_DAYS = 200


def _used_fields(expr: dict) -> set[str]:
    """提取表达式引用的字段名。"""
    return {
        node.name
        for node in _walk_expression(parse_expression(expr))
        if node.op == "field" and node.name is not None
    }


def _snapshot_fields_for_defs(defs: list) -> list[str]:
    """返回 defs 表达式需要快照表供给的字段列表。"""
    needed: set[str] = set()
    for d in defs:
        needed |= _used_fields(d.expression)
    return sorted(needed & set(SNAPSHOT_SPEC_FIELDS))


def score_row(row: dict, db: Session) -> float | None:
    """单票打分(原始量纲加权)。

    保留给只有单行、无截面可比的调用方。**排序请勿用它**:不同因子
    量纲不同、无去极值,直接加权会比较失真。
    """
    config = load_selection_config(db)
    score = 0.0
    weights = config.score_weights or {}
    values = row.get("values", {})
    for k, w in weights.items():
        v = values.get(k)
        if v is None:
            return None
        score += w * v
    vol_conf = config.vol_confirm or {}
    factor = vol_conf.get("factor")
    if factor:
        vol = values.get(factor)
        if vol is not None:
            cap = vol_conf.get("cap", 0.0)
            weight = vol_conf.get("weight", 0.0)
            score += weight * min(vol, cap)
    return round(score, 6)


def _winsorized_zscore(values: pd.Series) -> pd.Series:
    """去极值(1%/99% 分位裁剪)后做 z-score;NaN 按截面中位数填充。

    无量纲化是横向可比的前提:不同因子原始尺度不同,
    直接加权等于偷偷改变相对权重。
    """
    filled = values.astype(float)
    median = filled.median()
    filled = filled.fillna(0.0 if pd.isna(median) else median)
    if len(filled) >= 5:  # 样本太少时分位裁剪没有意义
        lo, hi = filled.quantile(0.01), filled.quantile(0.99)
        filled = filled.clip(lower=lo, upper=hi)
    std = filled.std(ddof=0)
    if not std > 0:
        return pd.Series(0.0, index=filled.index)
    return (filled - filled.mean()) / std


def score_cross_section(
    rows: list[dict],
    *,
    weights: dict[str, float],
    vol_confirm: dict[str, Any] | None = None,
) -> dict[str, float]:
    """对同一截面的候选打分,返回 {code: score}。

    因子先去极值 + z-score 再加权,量纲统一;单因子缺失按截面中位数填充,
    不再因任一因子 NaN 就丢掉整只(静默缩小池子)。
    量能确认同样标准化后按小权重加成,不会盖过主项。
    """
    if not rows:
        return {}
    frame = pd.DataFrame(rows).set_index("code")
    values_frame = pd.DataFrame(
        [r.get("values", {}) for r in rows],
        index=frame.index,
    )
    total = pd.Series(0.0, index=frame.index)
    for key, weight in weights.items():
        col = values_frame[key] if key in values_frame else pd.Series(
            dtype=float, index=frame.index,
        )
        total += weight * _winsorized_zscore(col)
    if vol_confirm:
        factor = vol_confirm.get("factor")
        if factor:
            col = values_frame[factor] if factor in values_frame else pd.Series(
                dtype=float, index=frame.index,
            )
            capped = col.astype(float).clip(upper=vol_confirm.get("cap", 0.0))
            total += vol_confirm.get("weight", 0.0) * _winsorized_zscore(capped)
    return {str(code): round(float(v), 6) for code, v in total.items()}


def compute_factor_rows(db: Session, codes: list[str],
                        day: date) -> list[dict]:
    """对 codes 计算 day 当日的因子,upsert quant_factor_daily,返回行列表。

    每行: {code, date, close, volume, above_ma20, bars, values: {...}}
    """
    defs = load_enabled_defs(db)
    min_len = max((d.min_bars for d in defs), default=1)
    window_days = max(LOOKBACK_DAYS, min_len * 2)
    start = day - timedelta(days=window_days)
    extra_fields = _snapshot_fields_for_defs(defs)

    rows: list[dict] = []
    bars_by_code = load_bars_df_bulk(
        db, codes, start=start, end=day, extra_fields=extra_fields,
    )
    for code, df in bars_by_code.items():
        if len(df) < min_len or df["date"].iat[-1] != day:
            continue  # 当日无数据(停牌/非交易日/未更新)
        close = df["close"]
        ma20 = close.rolling(20).mean().iat[-1]
        values: dict[str, float] = {}
        for d in defs:
            if len(df) >= d.min_bars:
                v = evaluate_def_last(d, df)
                if v is not None:
                    values[d.key] = v
        if not values:
            continue
        rows.append({
            "code": code,
            "date": day,
            "close": float(close.iat[-1]),
            "volume": float(df["volume"].iat[-1]),
            "above_ma20": bool(close.iat[-1] >= ma20),
            "bars": len(df),
            "values": values,
        })

    if rows:
        db.execute(delete(FactorDaily).where(
            FactorDaily.date == day,
            FactorDaily.code.in_([r["code"] for r in rows]),
        ))
        db.execute(
            FactorDaily.__table__.insert(),
            [{"code": r["code"], "date": r["date"], "values": r["values"]}
             for r in rows],
        )
    return rows


def _apply_hard_filters(
    rows: list[dict],
    config,
    names: dict[str, str],
) -> tuple[list[dict], dict[str, int]]:
    """按配置中的 hard_filters 解释执行,返回幸存行与过滤计数。"""
    filters = config.hard_filters or []
    n_filtered: dict[str, int] = {}
    survivors: list[dict] = []

    def count(reason: str) -> None:
        n_filtered[reason] = n_filtered.get(reason, 0) + 1

    for r in rows:
        name = names.get(r["code"], "") or ""
        rejected = False
        for filt in filters:
            ftype = filt.get("type")
            if ftype == "exclude_st":
                if "ST" in name.upper() or "退" in name:
                    count("st")
                    rejected = True
                    break
            elif ftype == "exclude_suspended":
                if r["volume"] <= 0:
                    count("suspended")
                    rejected = True
                    break
            elif ftype == "min_bars":
                if r["bars"] < filt.get("value", 0):
                    count("new")
                    rejected = True
                    break
            elif ftype in ("factor_gte", "factor_lte", "factor_gt", "factor_lt"):
                factor_key = filt.get("factor")
                actual = r.get("values", {}).get(factor_key)
                threshold = filt.get("value")
                if actual is None:
                    count(f"factor_missing_{factor_key}")
                    rejected = True
                    break
                op = {
                    "factor_gte": lambda a, b: a >= b,
                    "factor_lte": lambda a, b: a <= b,
                    "factor_gt": lambda a, b: a > b,
                    "factor_lt": lambda a, b: a < b,
                }[ftype]
                if not op(actual, threshold):
                    count(f"{ftype}_{factor_key}")
                    rejected = True
                    break
            elif ftype == "row_flag":
                field = filt.get("field")
                expected = filt.get("value")
                if field == "above_ma20" and r.get("above_ma20") != expected:
                    count("trend")
                    rejected = True
                    break
            else:
                count(f"unknown_{ftype}")
                rejected = True
                break
        if not rejected:
            survivors.append(r)

    return survivors, n_filtered


def run_selection(db: Session, day: date | None = None,
                  top_n: int | None = None,
                  codes: list[str] | None = None,
                  factor_codes: list[str] | None = None) -> dict:
    """跑一天选股:过滤 -> 打分 -> Top N 落 quant_pick。返回汇总。

    factor_codes: 因子计算/落库范围,默认与选股池相同。盘后全市场任务
    (full_market_daily)传入当日有行情的全体代码,使 quant_factor_daily
    覆盖全 A;选股(过滤/打分/Top N)始终只在池内进行。
    """
    config = load_selection_config(db)
    day = day or today_cst()
    effective_top_n = top_n if top_n is not None else config.top_n
    if codes is None:
        codes = pool_at(db, day)
        if not codes and day >= today_cst():
            codes = current_pool(db)
    if not codes:
        raise ValueError("股票池为空,请先同步成分股")

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
    rows = compute_factor_rows(db, factor_codes if factor_codes is not None else codes, day)
    if factor_codes is not None:
        pool_set = set(codes)
        rows = [r for r in rows if r["code"] in pool_set]

    survivors, n_filtered = _apply_hard_filters(rows, config, names)

    scores = score_cross_section(
        survivors,
        weights=config.score_weights or {},
        vol_confirm=config.vol_confirm,
    )
    picked = []
    for r in survivors:
        score = scores.get(r["code"])
        if score is None:
            continue
        picked.append({**r, "score": score})

    picked.sort(key=lambda r: (-r["score"], r["code"]))
    top = picked[:effective_top_n]

    db.execute(delete(Pick).where(Pick.date == day))
    if top:
        db.execute(
            Pick.__table__.insert(),
            [{"date": day, "code": r["code"], "score": r["score"], "rank": i + 1,
              "factors": r["values"]}
             for i, r in enumerate(top)],
        )
    db.commit()
    logger.info("选股 %s: 池 %d,有效 %d,过滤 %s,入选 %d",
                day, len(codes), len(rows), n_filtered, len(top))
    return {"date": str(day), "pool": len(codes), "valid": len(rows),
            "filtered": n_filtered, "picked": len(top),
            "factor_scope": len(factor_codes) if factor_codes is not None else len(codes),
            "top": [{"code": r["code"], "score": r["score"], "rank": i + 1}
                    for i, r in enumerate(top[:10])]}


__all__ = [
    "LOOKBACK_DAYS",
    "compute_factor_rows",
    "run_selection",
    "score_cross_section",
    "score_row",
]
