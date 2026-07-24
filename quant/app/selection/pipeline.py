"""选股 pipeline:每日对股票池过滤 + 打分,产出 Top N 候选池落 quant_pick。

过滤器:非 ST、非停牌(当日 volume>0)、上市 >120 天(库内日线 >=120 条)、
20 日日均成交额 > 5000 万;
打分:动量为主加权(mom20/mom60/ma20_slope),量能(vol_ratio5)做加分确认;
趋势过滤:收盘价在 ma20 之下的不参与打分。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..data.universe import current_pool
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
VOL_CONFIRM_WEIGHT = 0.02   # 每单位量比加分
TOP_N = 30
MIN_AMOUNT_AVG20 = 5e7      # 20 日日均成交额下限 5000 万
MIN_LIST_BARS = 120         # 上市 >120 天(以库内日线数近似)
LOOKBACK_DAYS = 200         # 计算因子加载的历史窗口


def score_row(row: dict) -> float | None:
    """单票打分;趋势过滤(close 低于 ma20 即 ma20_slope 相关条件外,
    这里要求 mom20 > -0.2 防止接飞刀)等硬过滤已在 pipeline 做,这里纯加权。"""
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


def compute_factor_rows(db: Session, codes: list[str],
                        day: date) -> list[dict]:
    """对 codes 计算 day 当日的因子,upsert quant_factor_daily,返回行列表。

    每行: {code, date, close, volume, above_ma20, bars, **FACTOR_COLUMNS}
    """
    start = day - timedelta(days=LOOKBACK_DAYS)
    rows = []
    for code in codes:
        df = load_bars_df(db, code, start=start, end=day)
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

    # upsert quant_factor_daily
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
        db.commit()
    return rows


def run_selection(db: Session, day: date | None = None,
                  top_n: int = TOP_N, codes: list[str] | None = None) -> dict:
    """跑一天选股:过滤 -> 打分 -> Top N 落 quant_pick。返回汇总。"""
    day = day or date.today()
    codes = codes or current_pool(db)
    if not codes:
        raise ValueError("股票池为空,请先同步成分股")

    names = dict(db.execute(select(Stock.code, Stock.name)).all())
    rows = compute_factor_rows(db, codes, day)

    picked = []
    n_filtered = {"st": 0, "suspended": 0, "new": 0, "amount": 0, "trend": 0}
    for r in rows:
        name = names.get(r["code"], "") or ""
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
        score = score_row(r)
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
