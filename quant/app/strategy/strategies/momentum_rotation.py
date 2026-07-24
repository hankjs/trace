"""动量轮动策略(组合):池内动量 Top N,每周调仓,趋势破位清零。

- 打分:w_mom20 × mom20 + w_mom60 × mom60(20/60 日动量加权);
- 调仓:每周首个交易日,取 Top N 等权;
- 风控:每日检查,收盘跌破 ma20 的个股当日权重清零(回升后恢复)。
"""
from __future__ import annotations

import pandas as pd

NAME = "momentum_rotation"
KIND = "portfolio"
DEFAULT_PARAMS = {"top_n": 10, "w_mom20": 0.6, "w_mom60": 0.4}


def target_weights(dates, pool_dfs: dict[str, pd.DataFrame],
                   params: dict | None = None) -> pd.DataFrame:
    """返回目标权重矩阵:行=交易日,列=股票,值∈[0,1]。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    top_n = int(p["top_n"])
    idx = pd.DatetimeIndex(dates)
    close = pd.DataFrame(
        {c: d.set_index("date")["close"] for c, d in pool_dfs.items()}
    ).reindex(idx)

    score = p["w_mom20"] * (close / close.shift(20) - 1) \
        + p["w_mom60"] * (close / close.shift(60) - 1)
    ma20 = close.rolling(20).mean()

    iso = idx.isocalendar()
    week = pd.Series(iso["year"].to_numpy() * 100 + iso["week"].to_numpy(), index=idx)
    rebalance = week.ne(week.shift()).to_numpy()

    weights = pd.DataFrame(0.0, index=idx, columns=close.columns)
    cur = pd.Series(0.0, index=close.columns)
    for i in range(len(idx)):
        if rebalance[i]:
            s = score.iloc[i].dropna()
            top = s.nlargest(top_n)
            cur = pd.Series(0.0, index=close.columns)
            if len(top):
                cur.loc[top.index] = 1.0 / top_n
        broken = (close.iloc[i] < ma20.iloc[i]).fillna(False)
        weights.iloc[i] = cur.where(~broken, 0.0)
    return weights
