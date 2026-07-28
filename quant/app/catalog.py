"""面向用户的固定研究目录。

英文 key 是数据库、策略和 API 的稳定标识；中文名称、解释、单位与限制只在
这里维护，避免前后端各自翻译。目录只描述日频研究和模拟回测，不代表交易建议。
"""
from __future__ import annotations

from copy import deepcopy
from typing import Any


def _field(
    key: str,
    name: str,
    description: str,
    *,
    category: str,
    unit: str | None,
    direction: str,
    limits: str,
    value_type: str = "number",
    operators: tuple[str, ...] = (
        "eq", "ne", "gt", "gte", "lt", "lte", "between", "is_null", "not_null",
    ),
    source: str = "technical",
    available: bool = True,
    input_scale: float | None = None,
) -> dict[str, Any]:
    return {
        "key": key,
        "name": name,
        "description": description,
        "category": category,
        "unit": unit,
        "direction": direction,
        "limits": limits,
        "value_type": value_type,
        "input_scale": (
            input_scale if input_scale is not None
            else (0.01 if unit == "%" else 1.0) if value_type == "number"
            else None
        ),
        "operators": list(operators),
        "source": source,
        "available": available,
    }


FACTOR_FIELDS: dict[str, dict[str, Any]] = {
    "mom20": _field(
        "mom20", "近20日涨跌幅", "当前收盘价相对20个交易日前的变化幅度。",
        category="趋势与动量", unit="%", direction="数值越高表示近期走势越强",
        limits="使用复权日线计算，仅描述过去20个交易日，不代表未来收益。",
    ),
    "mom60": _field(
        "mom60", "近60日涨跌幅", "当前收盘价相对60个交易日前的变化幅度。",
        category="趋势与动量", unit="%", direction="数值越高表示中期走势越强",
        limits="使用复权日线计算，短期反转时可能滞后。",
    ),
    "rsi14": _field(
        "rsi14", "近期强弱程度（RSI 14）", "比较近14日上涨和下跌力度，取值通常为0至100。",
        category="趋势与动量", unit="0-100", direction="高值偏强、低值偏弱，不能简单等同买卖点",
        limits="强趋势中可长期处于高位或低位，应结合趋势和估值判断。",
    ),
    "atr_pct": _field(
        "atr_pct", "日常价格波动幅度", "14日平均真实波幅占当前收盘价的比例。",
        category="风险与波动", unit="%", direction="数值越高表示价格波动通常越大",
        limits="反映历史波动，不预测方向；停牌或异常价格会影响口径。",
    ),
    "vol_ratio5": _field(
        "vol_ratio5", "成交量相对5日平均", "当日成交量相对过去5日平均成交量的倍数。",
        category="成交与流动性", unit="倍", direction="大于1表示成交量高于近期平均",
        limits="日频近似量比，与行情软件的盘中量比口径不同。",
    ),
    "ma20_slope": _field(
        "ma20_slope", "20日平均价格趋势", "20日均线相对5个交易日前的变化幅度。",
        category="趋势与动量", unit="%", direction="正值表示20日均线向上",
        limits="均线是滞后指标，快速转折时反应较慢。",
    ),
    "amount_avg20": _field(
        "amount_avg20", "近20日日均成交额", "近20个交易日成交额的平均值。",
        category="成交与流动性", unit="亿元", direction="数值越高通常表示交易更活跃",
        limits="成交活跃不等同于公司质量或价格上涨。",
        input_scale=100_000_000,
    ),
}


MARKET_FILTER_FIELDS: dict[str, dict[str, Any]] = {
    "pct_chg": _field(
        "pct_chg", "当日涨跌幅", "当日收盘价相对前一交易日收盘价的变化幅度。",
        category="行情", unit="%", direction="正值上涨、负值下跌",
        limits="使用日线收盘口径；盘中快照仅用于展示，不参与日频筛选。",
    ),
    "high_dist": _field(
        "high_dist", "距离近期最高价", "当前收盘价相对指定窗口最高价的距离。",
        category="行情", unit="%", direction="越接近0表示越接近近期最高价",
        limits="窗口长度由筛选条件决定，接近新高不代表一定突破。",
    ),
    "ma_bull": _field(
        "ma_bull", "均线多头排列", "收盘价高于20日均线，且20日均线高于60日均线。",
        category="趋势与动量", unit=None, direction="满足表示中期趋势结构偏强",
        limits="属于滞后趋势条件，不能反映估值和基本面。",
        value_type="boolean", operators=("eq", "ne", "is_null", "not_null"),
    ),
}


BASIC_FILTER_FIELDS: dict[str, dict[str, Any]] = {
    "industry": _field(
        "industry", "所属行业", "股票基础信息中的行业分类。",
        category="基础信息", unit=None, direction="用于限定或排除行业范围",
        limits="行业分类来自股票基础资料，可能存在缺失或数据源口径差异。",
        value_type="string",
        operators=("eq", "ne", "in", "not_in", "is_null", "not_null"),
        source="stock",
    ),
    "is_st": _field(
        "is_st", "风险警示股票", "股票名称是否包含ST或退市风险标识。",
        category="基础信息", unit=None, direction="通常用于排除风险警示股票",
        limits="依据当前库存名称近似；历史筛选不能还原当时风险标识。",
        value_type="boolean", operators=("eq", "ne", "is_null", "not_null"),
        source="stock",
    ),
    "listing_days": _field(
        "listing_days", "上市交易天数", "截至筛选日，数据库中已有日线数据的交易日数量。",
        category="基础信息", unit="交易日", direction="数值越高表示可用于研究的历史日线越长",
        limits="这是库内日线条数，不等同于自然日，也会受本地数据完整度影响。",
        source="market",
    ),
}


# 基本面字段在统一字典中固定 key。available 表示筛选能力已启用；某个交易日
# 是否有对应数据由筛选响应的数据日期、命中统计和空值表达。
FUNDAMENTAL_FILTER_FIELDS: dict[str, dict[str, Any]] = {
    "pe_ttm": _field(
        "pe_ttm", "市盈率 PE（TTM）", "总市值相对最近12个月归母净利润的倍数。",
        category="估值", unit="倍", direction="在盈利为正且同业可比时，较低通常表示估值较低",
        limits="亏损公司数值可能无意义；应优先做行业内比较。",
        source="fundamental",
    ),
    "pb": _field(
        "pb", "市净率 PB", "总市值相对归属母公司股东权益的倍数。",
        category="估值", unit="倍", direction="同业可比时，较低通常表示账面估值较低",
        limits="轻资产公司和资产质量差异会降低可比性。",
        source="fundamental",
    ),
    "ps_ttm": _field(
        "ps_ttm", "市销率 PS（TTM）", "总市值相对最近12个月营业收入的倍数。",
        category="估值", unit="倍", direction="同业可比时，较低通常表示收入估值较低",
        limits="不反映利润率和现金流，不能单独判断便宜。",
        source="fundamental",
    ),
    "dividend_yield": _field(
        "dividend_yield", "股息率", "近12个月现金分红相对当前股价的比例。",
        category="估值", unit="%", direction="数值越高表示历史现金分红回报越高",
        limits="历史分红不保证延续；除权和特殊分红会影响口径。",
        source="fundamental",
    ),
    "total_market_cap": _field(
        "total_market_cap", "总市值", "全部已发行股份按当前价格计算的市场价值。",
        category="规模", unit="亿元", direction="仅表示公司市场规模，无单一优劣方向",
        limits="随股价变化，不等同于企业价值或净资产。",
        source="fundamental", input_scale=100_000_000,
    ),
    "roe": _field(
        "roe", "净资产收益率 ROE", "归母净利润相对平均归母净资产的比例。",
        category="盈利质量", unit="%", direction="持续较高通常表示股东资本使用效率较好",
        limits="高负债或一次性收益可能抬高ROE，应结合负债和现金流。",
        source="fundamental",
    ),
    "revenue_yoy": _field(
        "revenue_yoy", "营业收入同比增长率", "本期营业收入相对上年同期的变化幅度。",
        category="成长", unit="%", direction="正值表示收入同比增长",
        limits="单期波动较大，应结合连续多个报告期和行业周期。",
        source="fundamental",
    ),
    "profit_yoy": _field(
        "profit_yoy", "归母净利润同比增长率", "本期归母净利润相对上年同期的变化幅度。",
        category="成长", unit="%", direction="正值表示归母净利润同比增长",
        limits="低基数、亏损转盈和一次性损益会造成极端值。",
        source="fundamental",
    ),
    "gross_margin": _field(
        "gross_margin", "毛利率", "营业收入扣除营业成本后的毛利占收入比例。",
        category="盈利质量", unit="%", direction="同业可比时，较高通常表示产品盈利空间较好",
        limits="行业差异很大，应做行业内比较。",
        source="fundamental",
    ),
    "net_margin": _field(
        "net_margin", "净利率", "净利润占营业收入的比例。",
        category="盈利质量", unit="%", direction="同业可比时，较高通常表示最终盈利能力较好",
        limits="一次性损益和会计口径会影响单期数值。",
        source="fundamental",
    ),
    "debt_ratio": _field(
        "debt_ratio", "资产负债率", "总负债占总资产的比例。",
        category="财务风险", unit="%", direction="数值越高通常表示财务杠杆越高",
        limits="银行等金融行业口径特殊，不能与普通行业直接比较。",
        source="fundamental",
    ),
    "cashflow_ratio": _field(
        "cashflow_ratio", "经营现金流与净利润比", "经营活动现金流净额相对净利润的比例。",
        category="盈利质量", unit="倍", direction="长期接近或高于1通常表示利润现金含量较好",
        limits="单期营运资金变化会导致较大波动，净利润为负时需谨慎解释。",
        source="fundamental",
    ),
}


FILTER_FIELDS: dict[str, dict[str, Any]] = {
    **BASIC_FILTER_FIELDS,
    **FACTOR_FIELDS,
    **MARKET_FILTER_FIELDS,
    **FUNDAMENTAL_FILTER_FIELDS,
}


INDICATORS: dict[str, dict[str, Any]] = {
    "ma": _field(
        "ma", "简单移动平均线（MA）", "指定交易日窗口内收盘价的算术平均。",
        category="技术指标", unit="价格", direction="价格在均线上方通常表示该窗口趋势偏强",
        limits="滞后指标，窗口越长反应越慢。", operators=(),
    ),
    "ema": _field(
        "ema", "指数移动平均线（EMA）", "对近期价格赋予更高权重的移动平均线。",
        category="技术指标", unit="价格", direction="比同窗口简单均线更快响应价格变化",
        limits="更灵敏也可能产生更多短期噪声。", operators=(),
    ),
    "macd": _field(
        "macd", "指数平滑异同移动平均线（MACD）", "比较快慢指数均线，并用信号线观察趋势变化。",
        category="技术指标", unit="价格差", direction="DIF相对DEA的位置和柱体变化用于观察动量",
        limits="参数为12、26、9的常见口径，震荡市可能频繁反复。", operators=(),
    ),
    "rsi": _field(
        "rsi", "相对强弱指标（RSI）", "比较指定窗口内上涨和下跌力度。",
        category="技术指标", unit="0-100", direction="高值偏强、低值偏弱",
        limits="不能把70或30机械地视作买卖点。", operators=(),
    ),
    "atr": _field(
        "atr", "平均真实波幅（ATR）", "综合当日振幅与跳空计算指定窗口的平均波动。",
        category="技术指标", unit="价格", direction="数值越高表示绝对价格波动越大",
        limits="不同股价的股票应使用ATR占价格比例比较。", operators=(),
    ),
    "volume_ratio": _field(
        "volume_ratio", "成交量相对均量", "当日成交量相对过去指定窗口平均成交量的倍数。",
        category="技术指标", unit="倍", direction="大于1表示成交量高于近期平均",
        limits="本系统为日频口径，不等同于盘中量比。", operators=(),
    ),
}


def _param(
    key: str,
    name: str,
    description: str,
    default: int | float,
    *,
    value_type: str,
    unit: str | None,
    minimum: int | float,
    maximum: int | float,
    step: int | float,
) -> dict[str, Any]:
    return {
        "key": key,
        "name": name,
        "description": description,
        "default": default,
        "value_type": value_type,
        "unit": unit,
        "minimum": minimum,
        "maximum": maximum,
        "step": step,
    }


def _overlay_param(key: str, name: str, description: str) -> dict[str, Any]:
    """统一覆盖层参数元数据；fixed_pct.value 使用 0..1 小数比例。"""
    defaults = {
        "risk_overlay": {
            "enabled": False, "type": "fixed_pct", "value": 0.08,
            "atr_period": 14,
        },
        "take_profit": {
            "enabled": False, "type": "fixed_pct", "value": 0.20,
            "atr_period": 14,
        },
    }
    return {
        "key": key,
        "name": name,
        "description": description,
        "default": defaults[key],
        "value_type": "overlay",
        "unit": "fixed_pct 为小数比例；atr_multiple 为 ATR 倍数",
        "minimum": None,
        "maximum": None,
        "step": None,
        "fields": {
            "enabled": {"name": "是否启用", "value_type": "boolean"},
            "type": {
                "name": "计算类型", "value_type": "enum",
                "options": [
                    {"value": "fixed_pct", "name": "相对模拟入场价的固定比例"},
                    {"value": "atr_multiple", "name": "相对模拟入场价的 ATR 倍数"},
                ],
            },
            "value": {
                "name": "覆盖距离", "value_type": "number",
                "unit_by_type": {
                    "fixed_pct": "小数比例（0.08 表示 8%）",
                    "atr_multiple": "ATR 倍数",
                },
            },
            "atr_period": {
                "name": "ATR 计算窗口", "value_type": "integer", "unit": "交易日",
                "minimum": 2, "maximum": 250,
            },
        },
    }


def _plan_capability(
    *,
    plan_type: str,
    entry_type: str,
    entry_name: str,
    native_exit_name: str,
    native_exit_condition: str,
    native_price_line: bool,
) -> dict[str, Any]:
    """六模板共用的结构化中文研究计划能力声明。"""
    return {
        "plan_type": plan_type,
        "plan_type_name": "单标的研究计划" if plan_type == "single" else "组合调仓研究计划",
        "entry_observation": {
            "type": entry_type,
            "name": entry_name,
            "price_line_supported": entry_type in {"line", "range"},
        },
        "native_exit": {
            "name": native_exit_name,
            "condition": native_exit_condition,
            "price_line_supported": native_price_line,
        },
        "overlays": {
            "risk_overlay": {
                "name": "用户启用的风险覆盖层", "default_enabled": False,
                "types": ["fixed_pct", "atr_multiple"],
                "confirmation": "T 日收盘确认，T+1 日开盘模拟退出",
            },
            "take_profit": {
                "name": "可选止盈覆盖层", "default_enabled": False,
                "types": ["fixed_pct", "atr_multiple"],
                "confirmation": "T 日收盘确认，T+1 日开盘模拟退出",
            },
        },
        "execution": {
            "signal_time": "T 日收盘后",
            "simulation_time": "T+1 日开盘",
            "real_execution": "外部手工确认",
        },
    }


_OVERLAY_PARAMS = [
    _overlay_param(
        "risk_overlay", "统一风险覆盖层",
        "默认关闭；按模拟入场价的固定比例或入场信号日 ATR 计算风险失效线。",
    ),
    _overlay_param(
        "take_profit", "可选止盈覆盖层",
        "默认关闭；按模拟入场价的固定收益率或入场信号日 ATR 计算止盈参考线。",
    ),
]


# 算法模板字典,键 = `app/strategy/strategies` 里的模块 NAME。
# 这里描述的是**算法**(有哪些参数、什么含义、什么限制),不是用户的策略实例 ——
# 后者是 `quant_strategy` 的行(模板 + 一组参数 + 名字),见 alembic 0012。
# `kind` 与模块的 `KIND` 必须一致,tests/test_catalog.py 交叉校验。
STRATEGY_TEMPLATES: dict[str, dict[str, Any]] = {
    "ma_cross": {
        "key": "ma_cross", "name": "双均线趋势策略",
        "description": "短期均线上穿长期均线时进入模拟持有，下穿时退出。",
        "unit": None, "direction": "跟随价格趋势", "kind": "single", "kind_name": "单只股票",
        "limits": "均线滞后，震荡行情可能频繁切换；信号仅供研究，不会提交订单。",
        "constraints": ["fast < slow"],
        "plan_capability": _plan_capability(
            plan_type="single", entry_type="none", entry_name="快慢均线关系",
            native_exit_name="均线下穿退出", native_exit_condition="短期均线下穿长期均线",
            native_price_line=False,
        ),
        "params": [
            _param("fast", "短期均线天数", "较灵敏的均线窗口。", 5,
                   value_type="integer", unit="交易日", minimum=2, maximum=120, step=1),
            _param("slow", "长期均线天数", "用于确认中期趋势的均线窗口。", 20,
                   value_type="integer", unit="交易日", minimum=3, maximum=250, step=1),
            *_OVERLAY_PARAMS,
        ],
    },
    "breakout": {
        "key": "breakout", "name": "价格突破策略",
        "description": "收盘价突破过去一段时间高点时进入模拟持有，跌破短期低点时退出。",
        "unit": None, "direction": "跟随价格突破", "kind": "single", "kind_name": "单只股票",
        "limits": "假突破可能造成快速反转；信号在收盘后产生，回测按下一交易日开盘模拟成交。",
        "constraints": [],
        "plan_capability": _plan_capability(
            plan_type="single", entry_type="line", entry_name="过去 N 日高点观察线",
            native_exit_name="区间低点退出", native_exit_condition="收盘价跌破过去 M 日低点",
            native_price_line=True,
        ),
        "params": [
            _param("entry", "入场观察天数", "突破此前多少个交易日的最高价。", 20,
                   value_type="integer", unit="交易日", minimum=5, maximum=250, step=1),
            _param("exit", "退出观察天数", "跌破此前多少个交易日的最低价。", 10,
                   value_type="integer", unit="交易日", minimum=2, maximum=120, step=1),
            _param(
                "max_entry_premium", "最大观察溢价",
                "高于突破线仍可继续观察的最大比例；0 表示只展示观察线。",
                0.0, value_type="number", unit="比例",
                minimum=0.0, maximum=0.5, step=0.01,
            ),
            *_OVERLAY_PARAMS,
        ],
    },
    "mean_reversion": {
        "key": "mean_reversion", "name": "上升趋势中的超跌反弹策略",
        "description": "长期趋势向上且RSI偏低时进入模拟持有，强弱修复或趋势破坏时退出。",
        "unit": None, "direction": "在上升趋势中观察短期回落", "kind": "single", "kind_name": "单只股票",
        "limits": "超跌后仍可能继续下跌，RSI阈值不能单独作为真实交易依据。",
        "constraints": ["rsi_buy < rsi_sell"],
        "plan_capability": _plan_capability(
            plan_type="single", entry_type="none", entry_name="RSI 与长期均线条件",
            native_exit_name="强弱修复或趋势破坏", native_exit_condition="RSI 修复或收盘跌破长期均线",
            native_price_line=True,
        ),
        "params": [
            _param("rsi_buy", "RSI偏弱阈值", "低于该值时视为短期偏弱。", 30,
                   value_type="number", unit="0-100", minimum=5, maximum=50, step=1),
            _param("rsi_sell", "RSI修复阈值", "高于该值时视为短期强弱已修复。", 55,
                   value_type="number", unit="0-100", minimum=30, maximum=95, step=1),
            _param("ma", "长期趋势天数", "判断收盘价是否仍处于长期均线上方。", 60,
                   value_type="integer", unit="交易日", minimum=20, maximum=250, step=1),
            *_OVERLAY_PARAMS,
        ],
    },
    "volume_breakout": {
        "key": "volume_breakout", "name": "缩量整理后的放量突破策略",
        "description": "价格区间收敛且成交缩量后，放量突破平台上沿时进入模拟持有。",
        "unit": None, "direction": "观察整理后的量价突破", "kind": "single", "kind_name": "单只股票",
        "limits": "成交量和平台阈值对结果敏感，突发消息可能导致跳空和较大滑点。",
        "constraints": [],
        "plan_capability": _plan_capability(
            plan_type="single", entry_type="line", entry_name="整理平台上沿观察线",
            native_exit_name="平台或波动风险退出", native_exit_condition="收盘跌破平台下沿或模板 ATR 风险线",
            native_price_line=True,
        ),
        "params": [
            _param("window", "整理平台天数", "计算平台高低点和长期均量的窗口。", 20,
                   value_type="integer", unit="交易日", minimum=10, maximum=120, step=1),
            _param("range_max", "平台最大振幅", "平台高低点相对收盘价允许的最大幅度。", 0.15,
                   value_type="number", unit="比例", minimum=0.02, maximum=0.5, step=0.01),
            _param("vol_mult", "放量倍数", "突破时成交量相对窗口均量的最低倍数。", 2.0,
                   value_type="number", unit="倍", minimum=1.0, maximum=10.0, step=0.1),
            _param("atr_mult", "波动止损倍数", "入场价下方保留多少倍ATR波动空间。", 2.0,
                   value_type="number", unit="倍ATR", minimum=0.5, maximum=10.0, step=0.1),
            _param(
                "max_entry_premium", "最大观察溢价",
                "高于平台上沿仍可继续观察的最大比例；0 表示只展示观察线。",
                0.0, value_type="number", unit="比例",
                minimum=0.0, maximum=0.5, step=0.01,
            ),
            *_OVERLAY_PARAMS,
        ],
    },
    "momentum_rotation": {
        "key": "momentum_rotation", "name": "强势股票轮动策略",
        "description": "每周从股票池选取综合动量较强的股票等权模拟持有，并过滤跌破20日均线的股票。",
        "unit": None, "direction": "在股票池内轮换相对强势标的", "kind": "portfolio", "kind_name": "股票组合",
        "limits": "需要完整股票池和足够历史数据；高换手阶段会受到费用和滑点影响。",
        "constraints": ["w_mom20 + w_mom60 建议等于 1"],
        "plan_capability": _plan_capability(
            plan_type="portfolio_rebalance", entry_type="portfolio_rebalance", entry_name="周度 Top N 目标权重",
            native_exit_name="趋势资格过滤", native_exit_condition="收盘跌破 20 日均线或调仓调出",
            native_price_line=True,
        ),
        "params": [
            _param("top_n", "持有股票数量", "每次调仓最多等权持有的股票数。", 10,
                   value_type="integer", unit="只", minimum=1, maximum=100, step=1),
            _param("w_mom20", "20日动量权重", "近20日涨跌幅在综合分数中的权重。", 0.6,
                   value_type="number", unit="权重", minimum=0, maximum=1, step=0.05),
            _param("w_mom60", "60日动量权重", "近60日涨跌幅在综合分数中的权重。", 0.4,
                   value_type="number", unit="权重", minimum=0, maximum=1, step=0.05),
            *_OVERLAY_PARAMS,
        ],
    },
    "multifactor_hold": {
        "key": "multifactor_hold", "name": "多指标综合评分持有策略",
        "description": "按动量和均线趋势综合评分，每月选取排名靠前的股票等权模拟持有。",
        "unit": None, "direction": "持有综合评分较高的股票组合", "kind": "portfolio", "kind_name": "股票组合",
        "limits": "评分目前以技术因子为主，不等同于公司基本面质量；组合结果是历史模拟。",
        "constraints": [],
        "plan_capability": _plan_capability(
            plan_type="portfolio_rebalance", entry_type="portfolio_rebalance", entry_name="月度 Top N 目标权重",
            native_exit_name="排名变化调出", native_exit_condition="计划调仓时不再进入 Top N",
            native_price_line=False,
        ),
        "params": [
            _param("top_n", "持有股票数量", "每次调仓最多等权持有的股票数。", 20,
                   value_type="integer", unit="只", minimum=1, maximum=100, step=1),
            *_OVERLAY_PARAMS,
        ],
    },
}


SIGNAL_SIDES: dict[str, dict[str, Any]] = {
    "buy": {
        "key": "buy", "name": "入场提示", "description": "策略模拟目标状态从未持有变为持有。",
        "unit": None, "direction": "进入模拟持有", "limits": "不是实际买入指令，需由用户自行研究和决定。",
    },
    "sell": {
        "key": "sell", "name": "退出提示", "description": "策略模拟目标状态从持有变为未持有。",
        "unit": None, "direction": "退出模拟持有", "limits": "不是实际卖出指令，系统不会连接券商或提交订单。",
    },
    "watch": {
        "key": "watch", "name": "临近触发", "description": "条件接近策略阈值，但尚未形成状态变化。",
        "unit": None, "direction": "继续观察", "limits": "条件可能在后续交易日失效，不表示必须采取行动。",
    },
    "add": {
        "key": "add", "name": "加仓提示", "description": "持有期间加仓规则触发，策略模拟目标仓位上调一个档位。",
        "unit": None, "direction": "提高模拟持有比例", "limits": "不是实际买入指令，需由用户自行研究和决定。",
    },
    "reduce": {
        "key": "reduce", "name": "减仓提示", "description": "持有期间减仓规则触发，策略模拟目标仓位下调一个档位。",
        "unit": None, "direction": "降低模拟持有比例", "limits": "不是实际卖出指令，系统不会连接券商或提交订单。",
    },
}


MANUAL_TRADE_SIDES: dict[str, dict[str, Any]] = {
    "buy": {
        "key": "buy", "name": "买入", "description": "用户在外部交易应用完成后手工记录的买入成交。",
        "unit": None, "direction": "增加手工账本持仓", "limits": "只是手工记账，不是本系统生成或执行的订单。",
    },
    "sell": {
        "key": "sell", "name": "卖出", "description": "用户在外部交易应用完成后手工记录的卖出成交。",
        "unit": None, "direction": "减少手工账本持仓", "limits": "只是手工记账，不是本系统生成或执行的订单。",
    },
}


SIGNAL_REASON_TYPES: dict[str, dict[str, Any]] = {
    "position_change": {
        "key": "position_change", "name": "模拟持有状态变化",
        "description": "策略目标状态在相邻两个交易日发生变化。", "unit": None,
        "direction": "由 side 字段区分进入或退出", "limits": "按日线收盘数据判断，不是盘中触发。",
    },
    "near_cross": {
        "key": "near_cross", "name": "均线接近交叉",
        "description": "短期均线和长期均线的距离小于预警阈值。", "unit": "%",
        "direction": "可能接近金叉或死叉", "limits": "接近不代表下一交易日一定交叉。",
    },
    "near_entry_line": {
        "key": "near_entry_line", "name": "接近突破线",
        "description": "收盘价距离过去一段时间的最高价较近。", "unit": "%",
        "direction": "可能接近向上突破", "limits": "未满足突破条件，且可能回落。",
    },
    "near_rsi_buy": {
        "key": "near_rsi_buy", "name": "RSI接近偏弱阈值",
        "description": "长期趋势向上时，RSI接近策略设置的偏弱阈值。", "unit": "0-100",
        "direction": "可能接近超跌观察条件", "limits": "RSI可能继续走低，不能单独作为入场依据。",
    },
    "near_platform_high": {
        "key": "near_platform_high", "name": "接近整理平台上沿",
        "description": "缩量整理平台已形成，收盘价接近平台最高价。", "unit": "%",
        "direction": "可能接近放量突破条件", "limits": "尚未同时满足放量和突破要求。",
    },
    "watch_proximity": {
        "key": "watch_proximity", "name": "临近触发",
        "description": "入场表达式与触发条件的归一化间距在临近容差内。", "unit": "%",
        "direction": "可能接近策略入场条件", "limits": "临近不代表下一交易日一定触发，条件也可能走远失效。",
    },
    "unknown": {
        "key": "unknown", "name": "策略规则触发",
        "description": "历史信号未记录可识别的原因类型。", "unit": None,
        "direction": "请结合策略说明复核", "limits": "不会把内部JSON直接作为用户说明。",
    },
}


def _metric(key: str, name: str, description: str, unit: str,
            direction: str, limits: str) -> dict[str, str]:
    return {
        "key": key, "name": name, "description": description,
        "unit": unit, "direction": direction, "limits": limits,
    }


BACKTEST_METRICS: dict[str, dict[str, str]] = {
    "total_return": _metric(
        "total_return", "区间总收益率", "回测结束净值相对开始净值的累计变化。", "%",
        "越高表示该历史区间模拟收益越高", "依赖所选区间，不能外推未来表现。"),
    "annual_return": _metric(
        "annual_return", "年化收益率", "将回测区间收益按252个交易日折算为一年。", "%/年",
        "越高表示历史复合增长速度越高", "短周期年化可能被显著放大。"),
    "max_drawdown": _metric(
        "max_drawdown", "最大回撤", "净值从历史高点到随后低点的最大跌幅。", "%",
        "绝对值越小表示历史下跌幅度较小", "只覆盖回测区间内已发生的风险。"),
    "sharpe": _metric(
        "sharpe", "夏普比率", "年化收益相对日收益波动的比值，本系统未扣无风险利率。", "比率",
        "越高表示单位历史波动对应的收益越高", "对收益分布和区间敏感，不能单独评价策略。"),
    "win_rate": _metric(
        "win_rate", "盈利交易占比", "已完成模拟交易中盈利交易所占比例。", "%",
        "越高表示历史盈利交易占比越高", "高胜率仍可能因少数大亏损而整体亏损。"),
    "trade_count": _metric(
        "trade_count", "模拟成交次数", "回测撮合产生的买入、卖出或调仓订单笔数。", "笔",
        "用于观察交易频率和成本敏感度", "组合调仓可能在同一天产生多笔成交。"),
    "round_trips": _metric(
        "round_trips", "完整交易次数", "从进入到退出构成的已完成模拟交易数量。", "次",
        "样本越多通常越便于评估稳定性", "未退出的持有状态不计为完整交易。"),
    "annual_return_mean": _metric(
        "annual_return_mean", "平均年化收益率", "参数扫描或股票样本的年化收益率算术平均。", "%/年",
        "越高表示样本平均历史表现越高", "容易受少数极端结果影响。"),
    "annual_return_median": _metric(
        "annual_return_median", "年化收益率中位数", "参数扫描或股票样本年化收益率的中间值。", "%/年",
        "越高表示典型样本历史表现越高", "不反映样本间差异和尾部风险。"),
    "total_return_mean": _metric(
        "total_return_mean", "平均区间收益率", "股票样本区间总收益率的算术平均。", "%",
        "越高表示样本平均区间表现越高", "容易受极端值影响。"),
    "total_return_median": _metric(
        "total_return_median", "区间收益率中位数", "股票样本区间总收益率的中间值。", "%",
        "越高表示典型样本区间表现越高", "不表示任一只股票的实际结果。"),
    "max_drawdown_median": _metric(
        "max_drawdown_median", "最大回撤中位数", "股票样本最大回撤的中间值。", "%",
        "绝对值越小表示典型样本回撤较小", "不能反映最差样本的回撤。"),
    "sharpe_median": _metric(
        "sharpe_median", "夏普比率中位数", "股票样本夏普比率的中间值。", "比率",
        "越高表示典型样本风险收益比更高", "样本较少时代表性有限。"),
    "win_rate_mean": _metric(
        "win_rate_mean", "平均盈利交易占比", "股票样本盈利交易占比的算术平均。", "%",
        "越高表示样本平均胜率越高", "不反映单笔盈亏大小。"),
}


def _ordered_items(items: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    return [deepcopy(item) for item in items.values()]


def catalog_payload() -> dict[str, Any]:
    """返回可直接 JSON 序列化的完整目录副本。"""
    return {
        "version": 1,
        "product_boundary": {
            "frequency": "daily",
            "frequency_name": "日频研究",
            "execution": "manual_external",
            "execution_name": "用户在外部交易应用中自行决定并手工操作",
            "notice": "本系统只提供研究、模拟回测和手工记账，不连接券商，不提交订单。",
        },
        "factors": _ordered_items(FACTOR_FIELDS),
        "indicators": _ordered_items(INDICATORS),
        "filter_fields": _ordered_items(FILTER_FIELDS),
        # 算法模板元数据。策略实例(用户/公共)走 GET /api/strategies,
        # 那是随用户变化的业务数据,不属于这份静态目录。
        "strategy_templates": _ordered_items(STRATEGY_TEMPLATES),
        "signals": _ordered_items(SIGNAL_SIDES),
        "signal_sides": _ordered_items(SIGNAL_SIDES),
        "manual_trade_sides": _ordered_items(MANUAL_TRADE_SIDES),
        "signal_reason_types": _ordered_items(SIGNAL_REASON_TYPES),
        "backtest_metrics": _ordered_items(BACKTEST_METRICS),
    }


def template_name(key: str) -> str:
    """算法模板的中文名。策略实例的名字由用户自定,取 `quant_strategy.name`。"""
    return STRATEGY_TEMPLATES.get(key, {}).get("name", key)


def template_params(key: str) -> list[dict[str, Any]]:
    """模板的参数元数据(前端表单与后端校验共用同一份)。"""
    return STRATEGY_TEMPLATES.get(key, {}).get("params", [])


def template_defaults(key: str) -> dict[str, Any]:
    return {p["key"]: p["default"] for p in template_params(key)}


def signal_side_name(key: str) -> str:
    return SIGNAL_SIDES.get(key, {}).get("name", key)


def manual_trade_side_name(key: str) -> str:
    return MANUAL_TRADE_SIDES.get(key, {}).get("name", key)


def _fmt_number(value: Any, digits: int = 2) -> str | None:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    return f"{float(value):.{digits}f}"


def _fmt_percent(value: Any) -> str | None:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    return _fmt_number(abs(float(value)) * 100)


def signal_reason_type(reason: dict[str, Any] | None) -> str:
    if not isinstance(reason, dict):
        return "unknown"
    reason_type = reason.get("type")
    if isinstance(reason_type, str) and reason_type in SIGNAL_REASON_TYPES:
        return reason_type
    if "prev_position" in reason or "cur_position" in reason:
        return "position_change"
    return "unknown"


def render_signal_reason(template: str, side: str,
                         reason: dict[str, Any] | None,
                         display_name: str | None = None) -> str:
    """把信号原因转换为面向用户的中文句子，不直接拼接内部 JSON。

    `template` 是算法模板 key(决定用哪套措辞),`display_name` 是策略实例的
    名字 —— 只有兜底分支会用到它。两者分开传:同一模板可以有多个策略实例
    (「我的双均线 10/30」),句子里该出现用户起的名字,不是模板名。
    """
    reason = reason if isinstance(reason, dict) else {}
    # 通用 watch 信号(规格化临近判定)优先使用引擎生成的可读说明;
    # 其 reason 也带 prev_position,不能落入下方 position_change 措辞。
    watch = reason.get("watch")
    if (
        side == "watch" and isinstance(watch, dict)
        and isinstance(watch.get("summary"), str)
    ):
        return watch["summary"]
    # 加减仓是档位变化而非状态翻转,先于下方 buy/sell 的模板措辞处理。
    if side in {"add", "reduce"} and (
        "prev_position" in reason or "cur_position" in reason
    ):
        prev_text = _fmt_number(reason.get("prev_position")) or "0"
        cur_text = _fmt_number(reason.get("cur_position")) or "0"
        if side == "add":
            return (f"持有期间加仓规则触发，策略模拟目标仓位从{prev_text}"
                    f"上调至{cur_text}。")
        return (f"持有期间减仓规则触发，策略模拟目标仓位从{prev_text}"
                f"下调至{cur_text}。")
    reason_type = signal_reason_type(reason)
    defaults = template_defaults(template)
    params = {**defaults, **(reason.get("params") if isinstance(reason.get("params"), dict) else {})}

    if reason_type == "position_change":
        if template == "ma_cross":
            relation = "上穿" if side == "buy" else "下穿"
            return (f"{params.get('fast', 5)}日均线{relation}{params.get('slow', 20)}日均线，"
                    f"策略模拟状态变为{'持有' if side == 'buy' else '未持有'}。")
        if template == "breakout":
            if side == "buy":
                return (f"收盘价突破此前{params.get('entry', 20)}个交易日最高价，"
                        "策略模拟状态变为持有。")
            return (f"收盘价跌破此前{params.get('exit', 10)}个交易日最低价，"
                    "策略模拟状态变为未持有。")
        if template == "mean_reversion":
            if side == "buy":
                return (f"收盘价仍在{params.get('ma', 60)}日均线上方，且RSI 14低于"
                        f"{params.get('rsi_buy', 30)}，策略模拟状态变为持有。")
            return (f"RSI 14高于{params.get('rsi_sell', 55)}或收盘价跌破"
                    f"{params.get('ma', 60)}日均线，策略模拟状态变为未持有。")
        if template == "volume_breakout":
            if side == "buy":
                return (f"价格在{params.get('window', 20)}日平台整理后放量突破上沿，"
                        "策略模拟状态变为持有。")
            return "收盘价跌破整理平台下沿或波动止损线，策略模拟状态变为未持有。"
        prev = "持有" if reason.get("prev_position") == 1 else "未持有"
        cur = "持有" if reason.get("cur_position") == 1 else "未持有"
        label = display_name or template_name(template)
        return f"{label}的模拟目标状态从“{prev}”变为“{cur}”。"

    if reason_type == "near_cross":
        direction = {"golden": "金叉", "death": "死叉"}.get(
            reason.get("direction"), "均线交叉"
        )
        gap = _fmt_percent(reason.get("gap_pct"))
        suffix = f"，当前相差约{gap}%" if gap is not None else ""
        return (f"{params.get('fast', 5)}日均线与{params.get('slow', 20)}日均线距离较近"
                f"{suffix}，可能接近{direction}，尚未形成新的状态变化。")

    if reason_type == "near_entry_line":
        dist = _fmt_percent(reason.get("dist"))
        suffix = f"约{dist}%" if dist is not None else "很近"
        return (f"收盘价距离{params.get('entry', 20)}日突破线还有{suffix}，"
                "尚未满足向上突破条件。")

    if reason_type == "near_rsi_buy":
        rsi_value = _fmt_number(reason.get("rsi14"))
        value_text = f"当前RSI 14为{rsi_value}，" if rsi_value is not None else ""
        return (f"{value_text}接近偏弱阈值{params.get('rsi_buy', 30)}，且价格仍在"
                f"{params.get('ma', 60)}日均线上方，尚未形成新的状态变化。")

    if reason_type == "near_platform_high":
        dist = _fmt_percent(reason.get("dist"))
        suffix = f"约{dist}%" if dist is not None else "很近"
        return (f"缩量整理平台已经形成，收盘价距离{params.get('window', 20)}日平台上沿"
                f"还有{suffix}，尚未同时满足放量突破条件。")

    return "已记录策略规则触发结果，请结合策略说明和当日数据复核。"
