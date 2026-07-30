"""多因子评分持有策略(组合):评分 Top N 等权,每月调仓,不择时。

打分口径与选股 pipeline  historically 一致(mom20/mom60/ma20_slope 加权);
新执行路径通过 StrategySpec score 表达式计算,本模块保留 legacy target_weights
以兼容旧回测入口。
"""
from __future__ import annotations

import pandas as pd

from ..rebalance import close_price_matrix, top_n_rebalance_weights
from ..overlays import overlay_defaults

NAME = "multifactor_hold"
KIND = "portfolio"
DEFAULT_PARAMS = {"top_n": 20, **overlay_defaults()}

# 与默认选股配置一致的历史权重;新规格使用 StrategySpec 中的 score 表达式。
_DEFAULT_SCORE_WEIGHTS = {"mom20": 0.5, "mom60": 0.3, "ma20_slope": 0.2}


def rebalance_mask(dates) -> pd.Series:
    """每月首个交易日形成计划调仓。"""
    idx = pd.DatetimeIndex(dates)
    month = pd.Series(idx.year * 100 + idx.month, index=idx)
    return month.ne(month.shift())


def target_weights(dates, pool_dfs: dict[str, pd.DataFrame],
                   params: dict | None = None,
                   eligibility: pd.DataFrame | None = None) -> pd.DataFrame:
    """返回目标权重矩阵:行=交易日,列=股票,值∈[0,1]。"""
    p = {**DEFAULT_PARAMS, **(params or {})}
    top_n = int(p["top_n"])
    idx = pd.DatetimeIndex(dates)
    close = close_price_matrix(idx, pool_dfs)

    ma20 = close.rolling(20).mean()
    score = (
        _DEFAULT_SCORE_WEIGHTS["mom20"] * (close / close.shift(20) - 1)
        + _DEFAULT_SCORE_WEIGHTS["mom60"] * (close / close.shift(60) - 1)
        + _DEFAULT_SCORE_WEIGHTS["ma20_slope"] * (ma20 / ma20.shift(5) - 1)
    )

    rebalance = rebalance_mask(idx).to_numpy()

    return top_n_rebalance_weights(
        score,
        rebalance,
        top_n,
        eligibility=eligibility,
    )
