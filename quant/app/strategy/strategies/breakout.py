"""N 日突破策略:收盘创 N 日新高做多,跌破 N 日新低(或出场线)空仓。

入场:close > 前 N 日最高价的最大值
出场:close < 前 N 日最低价的最小值
"""
from __future__ import annotations

import pandas as pd

from ..overlays import overlay_defaults

NAME = "breakout"
KIND = "single"
DEFAULT_PARAMS = {
    "entry": 20, "exit": 10, "max_entry_premium": 0.0,
    **overlay_defaults(),
}
WATCH_HIGH_DIST = 0.02  # 距 N 日新高 <2% 时给 watch 预警


def entry_observation_line(
    df: pd.DataFrame,
    params: dict | None = None,
) -> pd.Series:
    p = {**DEFAULT_PARAMS, **(params or {})}
    return df["high"].shift(1).rolling(int(p["entry"])).max()


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    p = {**DEFAULT_PARAMS, **(params or {})}
    exit_n = int(p["exit"])
    entry_line = entry_observation_line(df, p)
    exit_line = df["low"].shift(1).rolling(exit_n).min()

    pos = pd.Series(0, index=df.index, dtype=int)
    holding = 0
    for i in range(len(df)):
        if holding == 0 and pd.notna(entry_line.iat[i]) and df["close"].iat[i] > entry_line.iat[i]:
            holding = 1
        elif holding == 1 and pd.notna(exit_line.iat[i]) and df["close"].iat[i] < exit_line.iat[i]:
            holding = 0
        pos.iat[i] = holding
    return pos


def watch(df: pd.DataFrame, params: dict | None = None) -> dict | None:
    """临近触发:空仓状态下收盘距入场线(N 日新高)<2% 时预警"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    pos = positions(df, params)
    if pos.empty or pos.iat[-1] != 0:
        return None
    entry_line = entry_observation_line(df, p)
    if pd.isna(entry_line.iat[-1]):
        return None
    c = float(df["close"].iat[-1])
    line = float(entry_line.iat[-1])
    dist = line / c - 1
    if 0 < dist < WATCH_HIGH_DIST:
        return {"type": "near_entry_line", "close": c,
                "entry_line": round(line, 3), "dist": round(dist, 4)}
    return None
