"""具体策略集合。

两类契约:
- 单标的(KIND="single"): positions(df, params) -> pd.Series[0/1]
- 组合(KIND="portfolio"): target_weights(dates, pool_dfs, params, eligibility=None)
  -> DataFrame(行日期×列股票)，eligibility 为动态成分股可选掩码。

单标的策略可选实现 watch(df, params) -> dict | None,临近触发时给出预警原因。
新增策略:实现对应契约并在 REGISTRY 注册。
"""
from __future__ import annotations

from . import (breakout, ma_cross, mean_reversion, momentum_rotation,
               multifactor_hold, volume_breakout)

# 策略注册表:名字 -> 模块
REGISTRY = {
    ma_cross.NAME: ma_cross,
    breakout.NAME: breakout,
    mean_reversion.NAME: mean_reversion,
    volume_breakout.NAME: volume_breakout,
    momentum_rotation.NAME: momentum_rotation,
    multifactor_hold.NAME: multifactor_hold,
}

SINGLE_STRATEGIES = [n for n, m in REGISTRY.items() if m.KIND == "single"]
PORTFOLIO_STRATEGIES = [n for n, m in REGISTRY.items() if m.KIND == "portfolio"]


def strategy_kind(name: str) -> str:
    mod = REGISTRY.get(name)
    if mod is None:
        raise ValueError(f"未知策略: {name},可选: {list(REGISTRY)}")
    return mod.KIND
