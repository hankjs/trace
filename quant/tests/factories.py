"""测试数据工厂:快速插入因子定义与选股配置。"""
from __future__ import annotations

from datetime import date

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.factors import invalidate_factor_cache
from app.models import FactorDef, SelectionConfig, Stock
from app.selection.config import invalidate_selection_config_cache


def seed_stock(db: Session, code: str, name: str = "测试股份",
               industry: str = "制造", list_date: date | None = None) -> Stock:
    """插入或返回已有股票基础信息。"""
    existing = db.get(Stock, code)
    if existing is not None:
        return existing
    stock = Stock(
        code=code, name=name, industry=industry,
        list_date=list_date,
    )
    db.add(stock)
    db.flush()
    return stock


def seed_factor_defs(db: Session, *, include_disabled: bool = False) -> list[FactorDef]:
    """插入 7 个系统因子定义(与 migration 0024 种子一致)。"""
    seed_data = [
        {
            "key": "mom20",
            "name": "近20日涨跌幅",
            "description": "当前收盘价相对20个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "数值越高表示近期走势越强",
            "limits": "使用复权日线计算，仅描述过去20个交易日，不代表未来收益。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 20},
            "expression_hash": "92dadb2caeb4523b4d7298763ea2ac2ffb8690f07dfc3b8d3f7c3712473a3d93",
            "min_bars": 21,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "mom60",
            "name": "近60日涨跌幅",
            "description": "当前收盘价相对60个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "数值越高表示中期走势越强",
            "limits": "使用复权日线计算，短期反转时可能滞后。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {"op": "momentum", "input": {"op": "field", "name": "close"}, "window": 60},
            "expression_hash": "9cd7a0c98b501e1e104f899aa4d5dc30197276e393eed5f1634f50541dbce69b",
            "min_bars": 61,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "rsi14",
            "name": "近期强弱程度（RSI 14）",
            "description": "比较近14日上涨和下跌力度，取值通常为0至100。",
            "category": "趋势与动量",
            "unit": "0-100",
            "direction": "高值偏强、低值偏弱，不能简单等同买卖点",
            "limits": "强趋势中可长期处于高位或低位，应结合趋势和估值判断。",
            "value_type": "number",
            "input_scale": 1.0,
            "expression": {"op": "rsi", "input": {"op": "field", "name": "close"}, "window": 14},
            "expression_hash": "5e36305481c85e09ac34a265bf1f94be2a5c8f89d775a1c6eefc1566a5e966b4",
            "min_bars": 15,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "atr_pct",
            "name": "日常价格波动幅度",
            "description": "14日平均真实波幅占当前收盘价的比例。",
            "category": "风险与波动",
            "unit": "%",
            "direction": "数值越高表示价格波动通常越大",
            "limits": "反映历史波动，不预测方向；停牌或异常价格会影响口径。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {
                "op": "divide",
                "left": {
                    "op": "atr",
                    "high": {"op": "field", "name": "high"},
                    "low": {"op": "field", "name": "low"},
                    "close": {"op": "field", "name": "close"},
                    "window": 14,
                },
                "right": {"op": "field", "name": "close"},
            },
            "expression_hash": "1cceae6cd71ca38caef6398a424b8f8feee10e1722628cb7029a6abe7ff44d64",
            "min_bars": 15,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "vol_ratio5",
            "name": "成交量相对5日平均",
            "description": "当日成交量相对过去5日平均成交量的倍数。",
            "category": "成交与流动性",
            "unit": "倍",
            "direction": "大于1表示成交量高于近期平均",
            "limits": "日频近似量比，与行情软件的盘中量比口径不同。",
            "value_type": "number",
            "input_scale": 1.0,
            "expression": {
                "op": "volume_ratio",
                "input": {"op": "field", "name": "volume"},
                "window": 5,
                "shift": 1,
            },
            "expression_hash": "c9867e7be9f7e04e044733ce5a0f41f0a0a794af10f8a410c4d63d7bb7dba7b3",
            "min_bars": 6,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "ma20_slope",
            "name": "20日平均价格趋势",
            "description": "20日均线相对5个交易日前的变化幅度。",
            "category": "趋势与动量",
            "unit": "%",
            "direction": "正值表示20日均线向上",
            "limits": "均线是滞后指标，快速转折时反应较慢。",
            "value_type": "number",
            "input_scale": 0.01,
            "expression": {
                "op": "subtract",
                "left": {
                    "op": "divide",
                    "left": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
                    "right": {
                        "op": "shift",
                        "input": {"op": "ma", "input": {"op": "field", "name": "close"}, "window": 20},
                        "periods": 5,
                    },
                },
                "right": {"op": "literal", "value": 1},
            },
            "expression_hash": "0a909dfc00af30c35fb9cb3c0341f69b2743216e3cf6a43ee140bfddd289751c",
            "min_bars": 25,
            "enabled": True,
            "is_system": True,
        },
        {
            "key": "amount_avg20",
            "name": "近20日日均成交额",
            "description": "近20个交易日成交额的平均值。",
            "category": "成交与流动性",
            "unit": "亿元",
            "direction": "数值越高通常表示交易更活跃",
            "limits": "成交活跃不等同于公司质量或价格上涨。",
            "value_type": "number",
            "input_scale": 100_000_000.0,
            "expression": {
                "op": "rolling_mean",
                "input": {"op": "field", "name": "amount"},
                "window": 20,
                "shift": 0,
            },
            "expression_hash": "5fdc2b2d77752d93bb91eeaa7b83a7fc29719f216702a5829451e6b2907a38d7",
            "min_bars": 20,
            "enabled": True,
            "is_system": True,
        },
    ]
    if include_disabled:
        seed_data.append({
            "key": "disabled_factor",
            "name": "禁用测试因子",
            "description": "",
            "category": "测试",
            "unit": None,
            "direction": "",
            "limits": "",
            "value_type": "number",
            "input_scale": 1.0,
            "expression": {"op": "field", "name": "close"},
            "expression_hash": "disabled-hash",
            "min_bars": 1,
            "enabled": False,
            "is_system": False,
        })

    defs: list[FactorDef] = []
    for data in seed_data:
        existing = db.execute(
            select(FactorDef).where(FactorDef.key == data["key"])
        ).scalar_one_or_none()
        if existing is None:
            def_ = FactorDef(**data)
            db.add(def_)
            db.flush()
            defs.append(def_)
        else:
            defs.append(existing)
    invalidate_factor_cache()
    return defs


def seed_selection_config(
    db: Session,
    *,
    overrides: dict | None = None,
) -> SelectionConfig:
    """插入默认选股配置(与 migration 0024 种子一致)。"""
    data = {
        "name": "default",
        "is_active": True,
        "score_weights": {"mom20": 0.5, "mom60": 0.3, "ma20_slope": 0.2},
        "vol_confirm": {"factor": "vol_ratio5", "cap": 3.0, "weight": 0.05},
        "hard_filters": [
            {"type": "exclude_st"},
            {"type": "exclude_suspended"},
            {"type": "min_bars", "value": 120},
            {"type": "factor_gte", "factor": "amount_avg20", "value": 50000000},
            {"type": "row_flag", "field": "above_ma20", "value": True},
        ],
        "top_n": 30,
    }
    if overrides:
        data.update(overrides)
    existing = db.execute(
        select(SelectionConfig).where(SelectionConfig.is_active.is_(True))
    ).scalar_one_or_none()
    if existing is not None:
        for key, value in data.items():
            setattr(existing, key, value)
        db.flush()
        invalidate_selection_config_cache()
        return existing
    config = SelectionConfig(**data)
    db.add(config)
    db.flush()
    invalidate_selection_config_cache()
    return config
