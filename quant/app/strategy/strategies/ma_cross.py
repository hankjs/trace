"""双均线策略:快线上穿慢线(金叉)做多,下穿(死叉)空仓。"""
from __future__ import annotations

import pandas as pd

from ...indicators import ma
from ..overlays import overlay_defaults

NAME = "ma_cross"
KIND = "single"
DEFAULT_PARAMS = {"fast": 5, "slow": 20, **overlay_defaults()}
WATCH_GAP_PCT = 0.01  # 快慢线差距 <1% 时给 watch 预警


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    """返回每日目标仓位序列(1=持有, 0=空仓),与 df 等长。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    fast = ma(df["close"], int(p["fast"]))
    slow = ma(df["close"], int(p["slow"]))
    pos = (fast > slow).astype(int)
    pos[fast.isna() | slow.isna()] = 0
    return pos


def watch(df: pd.DataFrame, params: dict | None = None) -> dict | None:
    """临近触发:快慢线相对差距 <1% 时预警(方向为可能穿越的一侧)"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    fast = ma(df["close"], int(p["fast"]))
    slow = ma(df["close"], int(p["slow"]))
    if len(df) < 2 or pd.isna(fast.iat[-1]) or pd.isna(slow.iat[-1]):
        return None
    gap = (float(fast.iat[-1]) - float(slow.iat[-1])) / float(slow.iat[-1])
    if abs(gap) < WATCH_GAP_PCT:
        return {"type": "near_cross", "direction": "golden" if gap > 0 else "death",
                "gap_pct": round(gap, 4),
                "fast": round(float(fast.iat[-1]), 3),
                "slow": round(float(slow.iat[-1]), 3)}
    return None
