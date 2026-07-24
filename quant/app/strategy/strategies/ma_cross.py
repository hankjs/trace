"""双均线策略:快线上穿慢线(金叉)做多,下穿(死叉)空仓。"""
from __future__ import annotations

import pandas as pd

from ...indicators import ma

NAME = "ma_cross"
DEFAULT_PARAMS = {"fast": 5, "slow": 20}


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    """返回每日目标仓位序列(1=持有, 0=空仓),与 df 等长。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    fast = ma(df["close"], int(p["fast"]))
    slow = ma(df["close"], int(p["slow"]))
    pos = (fast > slow).astype(int)
    pos[fast.isna() | slow.isna()] = 0
    return pos
