"""因子库:向量化计算,输入日线 DataFrame(open/high/low/close/volume/amount)。

factor_frame(df) 返回与 df 等长、含全部因子列的 DataFrame(前导窗口不足为 NaN);
latest_factors(df) 返回最后一行的因子 dict(供每日落库 quant_factor_daily)。
所有因子只用 T 日及以前数据,无未来函数。
"""
from __future__ import annotations

import numpy as np
import pandas as pd

from ..indicators import atr, ma, rsi, volume_ratio

FACTOR_COLUMNS = [
    "mom20", "mom60", "rsi14", "atr_pct", "vol_ratio5", "ma20_slope", "amount_avg20",
]

# 计算因子所需的最少历史条数(mom60 需要 61 条)
MIN_BARS = 80


def factor_frame(df: pd.DataFrame) -> pd.DataFrame:
    """逐日因子序列,索引与 df 对齐"""
    close, high, low = df["close"], df["high"], df["low"]
    ma20 = ma(close, 20)
    return pd.DataFrame(
        {
            "mom20": close / close.shift(20) - 1,
            "mom60": close / close.shift(60) - 1,
            "rsi14": rsi(close, 14),
            "atr_pct": atr(high, low, close, 14) / close,
            "vol_ratio5": volume_ratio(df["volume"], 5),
            "ma20_slope": ma20 / ma20.shift(5) - 1,
            "amount_avg20": df["amount"].rolling(20).mean(),
        },
        index=df.index,
    )


def latest_factors(df: pd.DataFrame) -> dict | None:
    """最后一个交易日的因子 dict;数据不足或当日因子全 NaN 时返回 None"""
    if len(df) < MIN_BARS:
        return None
    row = factor_frame(df).iloc[-1]
    if row.isna().all():
        return None
    return {k: (None if pd.isna(v) else round(float(v), 6))
            for k, v in row.items()}
