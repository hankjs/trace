"""算法模板集合。

两类契约:
- 单标的(KIND="single"): positions(df, params) -> pd.Series[0/1]
- 组合(KIND="portfolio"): target_weights(dates, pool_dfs, params, eligibility=None)
  -> DataFrame(行日期×列股票)，eligibility 为动态成分股可选掩码。

单标的策略可选实现 watch(df, params) -> dict | None,临近触发时给出预警原因。
新增模板:实现对应契约、在 REGISTRY 注册,并在 `app/catalog.py` 的
`STRATEGY_TEMPLATES` 补参数元数据。

这里的模块是**算法模板**,不是用户看到的策略 —— 后者是 `quant_strategy` 的行
(模板 + 一组参数 + 用户起的名字),见 alembic 0012。
"""
from __future__ import annotations

from . import (breakout, ma_cross, mean_reversion, momentum_rotation,
               multifactor_hold, volume_breakout)

# 模板注册表:模板 key -> 模块
REGISTRY = {
    ma_cross.NAME: ma_cross,
    breakout.NAME: breakout,
    mean_reversion.NAME: mean_reversion,
    volume_breakout.NAME: volume_breakout,
    momentum_rotation.NAME: momentum_rotation,
    multifactor_hold.NAME: multifactor_hold,
}

SINGLE_TEMPLATES = [n for n, m in REGISTRY.items() if m.KIND == "single"]
PORTFOLIO_TEMPLATES = [n for n, m in REGISTRY.items() if m.KIND == "portfolio"]


def template_kind(name: str) -> str:
    mod = REGISTRY.get(name)
    if mod is None:
        raise ValueError(f"未知策略模板: {name},可选: {list(REGISTRY)}")
    return mod.KIND


def resolve_module(strategy) -> object:
    """取策略行对应的算法模块。

    模板 key 存在库里而模块在代码里,理论上会不一致(降级部署、手改数据库)。
    这里明确报错而不是静默跳过 —— 静默会让用户以为策略在跑却永远没有信号。
    """
    mod = REGISTRY.get(strategy.template)
    if mod is None:
        raise ValueError(
            f"策略「{strategy.name}」的模板 {strategy.template} 不存在于当前代码,"
            f"可选: {sorted(REGISTRY)}"
        )
    return mod

