"""多因子评分持有策略(组合):评分 Top N 等权,每月调仓,不择时。

打分口径与选股 pipeline 一致(mom20/mom60/ma20_slope 加权),保持
"选股池"与"组合策略"同一套评分逻辑。
"""
from __future__ import annotations

import pandas as pd

from ...selection.pipeline import SCORE_WEIGHTS

NAME = "multifactor_hold"
KIND = "portfolio"
DEFAULT_PARAMS = {"top_n": 20}


def target_weights(dates, pool_dfs: dict[str, pd.DataFrame],
                   params: dict | None = None) -> pd.DataFrame:
    """返回目标权重矩阵:行=交易日,列=股票,值∈[0,1]。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    top_n = int(p["top_n"])
    idx = pd.DatetimeIndex(dates)
    close = pd.DataFrame(
        {c: d.set_index("date")["close"] for c, d in pool_dfs.items()}
    ).reindex(idx)

    ma20 = close.rolling(20).mean()
    score = (
        SCORE_WEIGHTS["mom20"] * (close / close.shift(20) - 1)
        + SCORE_WEIGHTS["mom60"] * (close / close.shift(60) - 1)
        + SCORE_WEIGHTS["ma20_slope"] * (ma20 / ma20.shift(5) - 1)
    )

    month = pd.Series(idx.year * 100 + idx.month, index=idx)
    rebalance = month.ne(month.shift()).to_numpy()

    weights = pd.DataFrame(0.0, index=idx, columns=close.columns)
    cur = pd.Series(0.0, index=close.columns)
    for i in range(len(idx)):
        if rebalance[i]:
            s = score.iloc[i].dropna()
            top = s.nlargest(top_n)
            cur = pd.Series(0.0, index=close.columns)
            if len(top):
                cur.loc[top.index] = 1.0 / top_n
        weights.iloc[i] = cur
    return weights
