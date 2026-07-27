"""放量突破策略:缩量平台后放量突破买入。

平台:过去 window 日的高低点区间(振幅 <= range_max 视为收敛平台),
且缩量(5 日均量 < 20 日均量);
入场:收盘价放量(> vol_mult × 20 日均量)突破平台上沿;
出场:收盘跌破平台下沿,或跌破 入场价 - atr_mult × ATR14(止损)。
"""
from __future__ import annotations

import pandas as pd

from ...indicators import atr
from ..overlays import overlay_defaults

NAME = "volume_breakout"
KIND = "single"
DEFAULT_PARAMS = {
    "window": 20, "range_max": 0.15, "vol_mult": 2.0, "atr_mult": 2.0,
    "max_entry_premium": 0.0,
    **overlay_defaults(),
}
WATCH_HIGH_DIST = 0.02  # 距平台上沿 <2% 时给 watch 预警


def entry_observation_line(
    df: pd.DataFrame,
    params: dict | None = None,
) -> pd.Series:
    p = {**DEFAULT_PARAMS, **(params or {})}
    return df["high"].shift(1).rolling(int(p["window"])).max()


def _positions_and_risk_line(
    df: pd.DataFrame,
    params: dict | None = None,
) -> tuple[pd.Series, pd.Series]:
    """同时计算目标仓位和模板实际使用的逐日 ATR 风险线。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    n = int(p["window"])
    high_n = entry_observation_line(df, p)
    low_n = df["low"].shift(1).rolling(n).min()
    vol_ma5 = df["volume"].shift(1).rolling(5).mean()
    vol_ma20 = df["volume"].shift(1).rolling(n).mean()
    atr14 = atr(df["high"], df["low"], df["close"], 14)

    pos = pd.Series(0, index=df.index, dtype=int)
    risk_line = pd.Series(float("nan"), index=df.index, dtype=float)
    holding = 0
    entry_px = stop = 0.0
    for i in range(len(df)):
        c = df["close"].iat[i]
        hi, lo = high_n.iat[i], low_n.iat[i]
        if pd.isna(hi) or pd.isna(lo):
            pos.iat[i] = holding
            continue
        if holding == 0:
            contracted = (hi - lo) / c <= p["range_max"]
            shrink = pd.notna(vol_ma5.iat[i]) and vol_ma5.iat[i] < vol_ma20.iat[i]
            burst = pd.notna(vol_ma20.iat[i]) and df["volume"].iat[i] > p["vol_mult"] * vol_ma20.iat[i]
            if contracted and shrink and burst and c > hi:
                holding = 1
                entry_px = c
                stop = entry_px - p["atr_mult"] * (atr14.iat[i] if pd.notna(atr14.iat[i]) else 0)
                risk_line.iat[i] = stop
        else:
            stop = max(stop, entry_px - p["atr_mult"] * (atr14.iat[i] if pd.notna(atr14.iat[i]) else 0))
            # 即使本日随后触发退出，也保留真正参与判断的风险线供计划解释。
            risk_line.iat[i] = stop
            if c < lo or c < stop:
                holding = 0
        pos.iat[i] = holding
    return pos, risk_line


def positions(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    return _positions_and_risk_line(df, params)[0]


def native_risk_line(df: pd.DataFrame, params: dict | None = None) -> pd.Series:
    """模板实际使用的风险线；未形成模拟持有状态时保持空值。"""
    return _positions_and_risk_line(df, params)[1]


def watch(df: pd.DataFrame, params: dict | None = None) -> dict | None:
    """临近触发:缩量平台已形成、收盘距平台上沿 <2% 时预警"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    n = int(p["window"])
    high_n = entry_observation_line(df, p)
    low_n = df["low"].shift(1).rolling(n).min()
    if len(df) < n + 1 or pd.isna(high_n.iat[-1]):
        return None
    c = float(df["close"].iat[-1])
    hi, lo = float(high_n.iat[-1]), float(low_n.iat[-1])
    vol_ma5 = df["volume"].shift(1).rolling(5).mean().iat[-1]
    vol_ma20 = df["volume"].shift(1).rolling(n).mean().iat[-1]
    if pd.isna(vol_ma5) or pd.isna(vol_ma20):
        return None
    contracted = (hi - lo) / c <= p["range_max"]
    shrink = vol_ma5 < vol_ma20
    dist = hi / c - 1
    if contracted and shrink and 0 < dist < WATCH_HIGH_DIST:
        return {"type": "near_platform_high", "close": c,
                "platform_high": round(hi, 3), "dist": round(dist, 4)}
    return None
