"""具体策略集合。新增策略:实现 positions(df, params) 并在 REGISTRY 注册。"""
from __future__ import annotations

from . import breakout, ma_cross

# 策略注册表:名字 -> 模块(需实现 positions(df, params) -> pd.Series[0/1])
REGISTRY = {
    ma_cross.NAME: ma_cross,
    breakout.NAME: breakout,
}
