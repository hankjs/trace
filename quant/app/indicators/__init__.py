"""技术指标:纯 pandas 函数,输入为含 close/high/low/volume 列的 DataFrame。

所有函数返回与输入等长的 Series(或 DataFrame),前导不足窗口的位置为 NaN。
"""
from __future__ import annotations

import pandas as pd


def ma(close: pd.Series, window: int) -> pd.Series:
    """简单移动平均"""
    return close.rolling(window).mean()


def ema(close: pd.Series, window: int) -> pd.Series:
    """指数移动平均"""
    return close.ewm(span=window, adjust=False).mean()


def macd(close: pd.Series, fast: int = 12, slow: int = 26, signal: int = 9) -> pd.DataFrame:
    """MACD:返回 dif / dea / hist(柱状 = (dif-dea)*2,与常见行情软件一致)"""
    dif = ema(close, fast) - ema(close, slow)
    dea = dif.ewm(span=signal, adjust=False).mean()
    hist = (dif - dea) * 2
    return pd.DataFrame({"dif": dif, "dea": dea, "hist": hist})


def rsi(close: pd.Series, window: int = 14) -> pd.Series:
    """RSI(Wilder 平滑)"""
    delta = close.diff()
    gain = delta.clip(lower=0)
    loss = -delta.clip(upper=0)
    avg_gain = gain.ewm(alpha=1 / window, adjust=False).mean()
    avg_loss = loss.ewm(alpha=1 / window, adjust=False).mean()
    rs = avg_gain / avg_loss
    return 100 - 100 / (1 + rs)


def atr(high: pd.Series, low: pd.Series, close: pd.Series, window: int = 14) -> pd.Series:
    """ATR(平均真实波幅)"""
    prev_close = close.shift(1)
    tr = pd.concat(
        [high - low, (high - prev_close).abs(), (low - prev_close).abs()], axis=1
    ).max(axis=1)
    return tr.rolling(window).mean()


def volume_ratio(volume: pd.Series, window: int = 5) -> pd.Series:
    """量比:当日成交量 / 过去 N 日均量(日频近似口径)。

    分母为 0(停牌期间均量为 0)时置 NaN 而非 inf:inf 能满足任何
    `vol_ratio5 >= x` 筛选条件,又不被 NaN 过滤拦下,会把停牌股顶上榜首。
    """
    avg = volume.shift(1).rolling(window).mean()
    return volume / avg.where(avg > 0)
