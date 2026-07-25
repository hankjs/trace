"""组合策略共享的纯函数。"""
from __future__ import annotations

from collections.abc import Sequence

import pandas as pd


def close_price_matrix(dates, pool_dfs: dict[str, pd.DataFrame]) -> pd.DataFrame:
    """将各标的日线对齐为收盘价矩阵。"""
    index = pd.DatetimeIndex(dates)
    return pd.DataFrame(
        {code: frame.set_index("date")["close"]
         for code, frame in pool_dfs.items()}
    ).reindex(index)


def top_n_rebalance_weights(
    scores: pd.DataFrame,
    rebalance: Sequence[bool] | pd.Series,
    top_n: int,
    *,
    eligibility: pd.DataFrame | None = None,
    risk_blocked: pd.DataFrame | None = None,
) -> pd.DataFrame:
    """按调仓掩码选择评分 Top-N，并返回逐日等权目标。

    eligibility 是股票池成员资格：退出后会从当前目标中移除，重新进入时需等
    下一次调仓。risk_blocked 是临时风险过滤：只清零当日输出，不改变底层目标，
    条件恢复后可恢复原权重。
    """
    if top_n <= 0:
        raise ValueError("top_n 必须大于 0")

    index = scores.index
    columns = scores.columns
    if isinstance(rebalance, pd.Series):
        rebalance_mask = rebalance.reindex(index, fill_value=False).astype(bool)
    else:
        if len(rebalance) != len(index):
            raise ValueError("调仓掩码长度必须与评分矩阵一致")
        rebalance_mask = pd.Series(rebalance, index=index, dtype=bool)

    eligible = (
        eligibility.reindex(index=index, columns=columns).fillna(False).astype(bool)
        if eligibility is not None
        else pd.DataFrame(True, index=index, columns=columns)
    )
    blocked = (
        risk_blocked.reindex(index=index, columns=columns).fillna(False).astype(bool)
        if risk_blocked is not None
        else pd.DataFrame(False, index=index, columns=columns)
    )

    weights = pd.DataFrame(0.0, index=index, columns=columns)
    current = pd.Series(0.0, index=columns)
    for position in range(len(index)):
        if rebalance_mask.iat[position]:
            ranked = scores.iloc[position].where(eligible.iloc[position]).dropna()
            selected = ranked.nlargest(top_n)
            current = pd.Series(0.0, index=columns)
            if len(selected):
                current.loc[selected.index] = 1.0 / len(selected)
        current = current.where(eligible.iloc[position], 0.0)
        weights.iloc[position] = current.where(~blocked.iloc[position], 0.0)
    return weights
