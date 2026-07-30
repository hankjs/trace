"""选股 pipeline:每日对股票池过滤 + 打分,产出 Top N 候选池落 quant_pick。

过滤器:非 ST、非停牌(当日 volume>0)、上市 >120 天(库内日线 >=120 条)、
20 日日均成交额 > 5000 万;
打分:动量为主加权(mom20/mom60/ma20_slope),量能(vol_ratio5)做加分确认;
趋势过滤:收盘价在 ma20 之下的不参与打分。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

import pandas as pd
from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df, load_bars_df_bulk
from ..data.universe import current_pool, pool_at
from ..factors import FACTOR_COLUMNS, MIN_BARS, factor_frame
from ..models import DailyBar, FactorDaily, Pick, Stock

logger = logging.getLogger(__name__)

# 打分权重(经验默认值,动量为主;调整改这里即可)
SCORE_WEIGHTS = {
    "mom20": 0.50,
    "mom60": 0.30,
    "ma20_slope": 0.20,
}
VOL_CONFIRM_CAP = 3.0       # 量比加分封顶
VOL_CONFIRM_WEIGHT = 0.02   # 每单位量比加分(score_row 的原始量纲口径)
# 截面 z-score 口径下的量能权重:各因子已无量纲,0.02 会小到失去意义。
# 取 0.05 使量能只做"确认"(主项动量权重合计 1.0),不会盖过动量。
VOL_CONFIRM_WEIGHT_Z = 0.05
TOP_N = 30
MIN_AMOUNT_AVG20 = 5e7      # 20 日日均成交额下限 5000 万
MIN_LIST_BARS = 120         # 上市 >120 天(以库内日线数近似)
LOOKBACK_DAYS = 200         # 计算因子加载的历史窗口


def score_row(row: dict) -> float | None:
    """单票打分(原始量纲加权)。

    保留给只有单行、无截面可比的调用方。**排序请勿用它**:mom20 与 mom60
    量纲不同、无去极值,vol_ratio5 的加成上限与真实动量价差同量级,高换手股
    能压过动量更强的股。截面排序统一走 score_cross_section()。
    """
    score = 0.0
    for k, w in SCORE_WEIGHTS.items():
        v = row.get(k)
        if v is None:
            return None
        score += w * v
    vol = row.get("vol_ratio5")
    if vol is not None:
        score += VOL_CONFIRM_WEIGHT * min(vol, VOL_CONFIRM_CAP)
    return round(score, 6)


def _winsorized_zscore(values: pd.Series) -> pd.Series:
    """去极值(1%/99% 分位裁剪)后做 z-score;NaN 按截面中位数填充。

    无量纲化是横向可比的前提:mom20(月度动量)与 mom60(季度动量)原始
    尺度差一倍以上,直接加权等于偷偷给 mom60 更大权重。
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


def score_cross_section(rows: list[dict]) -> dict[str, float]:
    """对同一截面的候选打分,返回 {code: score}。

    因子先去极值 + z-score 再加权,量纲统一;单因子缺失按截面中位数填充,
    不再因任一因子 NaN 就 `return None` 丢掉整只(静默缩小池子)。
    量能确认(vol_ratio5)同样标准化后按小权重加成,不会盖过动量主项。
    """
    if not rows:
        return {}
    frame = pd.DataFrame(rows).set_index("code")
    total = pd.Series(0.0, index=frame.index)
    for key, weight in SCORE_WEIGHTS.items():
        col = frame[key] if key in frame else pd.Series(dtype=float,
                                                        index=frame.index)
        total += weight * _winsorized_zscore(col)
    if "vol_ratio5" in frame:
        capped = frame["vol_ratio5"].astype(float).clip(upper=VOL_CONFIRM_CAP)
        total += VOL_CONFIRM_WEIGHT_Z * _winsorized_zscore(capped)
    return {str(code): round(float(v), 6) for code, v in total.items()}


def compute_factor_rows(db: Session, codes: list[str],
                        day: date) -> list[dict]:
    """对 codes 计算 day 当日的因子,upsert quant_factor_daily,返回行列表。

    每行: {code, date, close, volume, above_ma20, bars, **FACTOR_COLUMNS}
    """
    start = day - timedelta(days=LOOKBACK_DAYS)
    rows = []
    # 批量加载全窗口日线后在内存分组,避免全 A 选股时 5000+ 次单股查询往返。
    bars_by_code = load_bars_df_bulk(db, codes, start=start, end=day)
    for code, df in bars_by_code.items():
        if len(df) < MIN_BARS or df["date"].iat[-1] != day:
            continue  # 当日无数据(停牌/非交易日/未更新)
        ff = factor_frame(df)
        last = ff.iloc[-1]
        if last.isna().all():
            continue
        ma20 = df["close"].rolling(20).mean().iat[-1]
        rows.append({
            "code": code,
            "date": day,
            "close": float(df["close"].iat[-1]),
            "volume": float(df["volume"].iat[-1]),
            "above_ma20": bool(df["close"].iat[-1] >= ma20),
            "bars": len(df),
            **{k: (None if v != v else float(v)) for k, v in last.items()},
        })

    # upsert quant_factor_daily(与 run_selection 共用同一事务,外层统一 commit)
    if rows:
        db.execute(delete(FactorDaily).where(
            FactorDaily.date == day,
            FactorDaily.code.in_([r["code"] for r in rows]),
        ))
        db.execute(
            FactorDaily.__table__.insert(),
            [{"code": r["code"], "date": r["date"],
              **{k: r[k] for k in FACTOR_COLUMNS}} for r in rows],
        )
    return rows


def run_selection(db: Session, day: date | None = None,
                  top_n: int = TOP_N, codes: list[str] | None = None) -> dict:
    """跑一天选股:过滤 -> 打分 -> Top N 落 quant_pick。返回汇总。"""
    day = day or date.today()
    if codes is None:
        codes = pool_at(db, day)
        if not codes and day >= date.today():
            codes = current_pool(db)
    if not codes:
        raise ValueError("股票池为空,请先同步成分股")

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
    rows = compute_factor_rows(db, codes, day)

    picked = []
    n_filtered = {"st": 0, "suspended": 0, "new": 0, "amount": 0, "trend": 0}
    survivors = []
    for r in rows:
        name = names.get(r["code"], "") or ""
        # 这里用当前名称判定 ST 是正确的:run_selection 跑的是**当日**选股,
        # 当日的当前状态就是正确状态。历史回溯口径必须用 quant_daily_bar.is_st
        # (逐日),见 alembic 0010 与 universe.all_market_pool。
        if "ST" in name.upper() or "退" in name:
            n_filtered["st"] += 1
            continue
        if r["volume"] <= 0:
            n_filtered["suspended"] += 1
            continue
        if r["bars"] < MIN_LIST_BARS:
            n_filtered["new"] += 1
            continue
        if (r["amount_avg20"] or 0) < MIN_AMOUNT_AVG20:
            n_filtered["amount"] += 1
            continue
        if not r["above_ma20"]:
            n_filtered["trend"] += 1
            continue
        survivors.append(r)

    # 硬过滤后在**同一截面**上标准化打分:量纲统一,单因子缺失不再丢整只
    scores = score_cross_section(survivors)
    for r in survivors:
        score = scores.get(r["code"])
        if score is None:
            continue
        picked.append({**r, "score": score})

    picked.sort(key=lambda r: (-r["score"], r["code"]))
    top = picked[:top_n]

    db.execute(delete(Pick).where(Pick.date == day))
    if top:
        # 空结果(节假日/数据未更新/全部被过滤)跳过批量 insert:
        # SQLAlchemy 对空参数列表会尝试默认值插入,违反非空约束
        db.execute(
            Pick.__table__.insert(),
            [{"date": day, "code": r["code"], "score": r["score"], "rank": i + 1,
              "factors": {k: r[k] for k in FACTOR_COLUMNS}}
             for i, r in enumerate(top)],
        )
    db.commit()
    logger.info("选股 %s: 池 %d,有效 %d,过滤 %s,入选 %d",
                day, len(codes), len(rows), n_filtered, len(top))
    return {"date": str(day), "pool": len(codes), "valid": len(rows),
            "filtered": n_filtered, "picked": len(top),
            "top": [{"code": r["code"], "score": r["score"], "rank": i + 1}
                    for i, r in enumerate(top[:10])]}
