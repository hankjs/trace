"""N 日突破策略:收盘创 N 日新高做多,跌破 N 日新低(或出场线)空仓。

入场:close > 前 N 日最高价的最大值
出场:close < 前 N 日最低价的最小值
"""
from __future__ import annotations

import pandas as pd

NAME = "breakout"
DEFAULT_PARAMS = {"entry": 20, "exit": 10}


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    p = {**DEFAULT_PARAMS, **(params or {})}
    entry_n = int(p["entry"])
    exit_n = int(p["exit"])
    entry_line = df["high"].shift(1).rolling(entry_n).max()
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
