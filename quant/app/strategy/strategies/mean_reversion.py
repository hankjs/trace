"""均值回复策略:大趋势向上时的超卖买入。

入场:RSI14 < rsi_buy(超卖)且收盘 > ma(大趋势向上)
出场:RSI14 > rsi_sell(修复)或收盘跌破 ma
"""
from __future__ import annotations

import pandas as pd

from ...indicators import ma, rsi
from ..overlays import overlay_defaults

NAME = "mean_reversion"
KIND = "single"
DEFAULT_PARAMS = {"rsi_buy": 30, "rsi_sell": 55, "ma": 60, **overlay_defaults()}
WATCH_RSI_GAP = 2.0  # RSI 距买入阈值 <2 时给 watch 预警


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    """返回每日目标仓位序列(1=持有, 0=空仓),与 df 等长。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    rsi14 = rsi(df["close"], 14)
    trend = ma(df["close"], int(p["ma"]))

    pos = pd.Series(0, index=df.index, dtype=int)
    holding = 0
    for i in range(len(df)):
        r, c, m = rsi14.iat[i], df["close"].iat[i], trend.iat[i]
        if pd.notna(r) and pd.notna(m):
            if holding == 0 and r < p["rsi_buy"] and c > m:
                holding = 1
            elif holding == 1 and (r > p["rsi_sell"] or c < m):
                holding = 0
        pos.iat[i] = holding
    return pos


def watch(df: pd.DataFrame, params: dict | None = None) -> dict | None:
    """临近触发:RSI 接近买入阈值且趋势向上时预警"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    rsi14 = rsi(df["close"], 14)
    trend = ma(df["close"], int(p["ma"]))
    if len(df) < 2 or pd.isna(rsi14.iat[-1]) or pd.isna(trend.iat[-1]):
        return None
    r, c, m = float(rsi14.iat[-1]), float(df["close"].iat[-1]), float(trend.iat[-1])
    gap = r - float(p["rsi_buy"])
    if 0 < gap < WATCH_RSI_GAP and c > m:
        return {"type": "near_rsi_buy", "rsi14": round(r, 2),
                "rsi_buy": p["rsi_buy"], "close": c, "ma": round(m, 3)}
    return None
