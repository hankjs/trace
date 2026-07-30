import { readonly, shallowReadonly, shallowRef } from 'vue'
import { api, type CatalogEntry, type CatalogPayload, type CatalogSection } from './api'

const numberOperators = ['gte', 'lte', 'gt', 'lt', 'eq', 'between']

export const operatorLabels: Record<string, string> = {
  gte: '大于等于',
  lte: '小于等于',
  gt: '大于',
  lt: '小于',
  eq: '等于',
  ne: '不等于',
  between: '介于',
  in: '属于',
  not_in: '不属于',
  contains: '包含',
  is_null: '为空',
  not_null: '不为空',
}

export const categoryLabels: Record<string, string> = {
  trend: '趋势',
  momentum: '动量',
  volatility: '波动',
  liquidity: '成交活跃度',
  valuation: '估值',
  quality: '盈利质量',
  growth: '成长',
  risk: '财务风险',
  basic: '基础信息',
  technical: '技术面',
  entry: '入场提示',
  exit: '退出提示',
  watch: '观察提示',
  single: '单只股票策略',
  portfolio: '组合策略',
  return: '收益',
  drawdown: '风险',
  trading: '交易统计',
}

export const fallbackCatalog: CatalogPayload = {
  factors: [
    {
      key: 'mom20',
      name: '近20个交易日涨跌幅',
      description: '比较当前价格与20个交易日前的价格，观察近期上涨或下跌的幅度。',
      category: 'momentum',
      unit: '%',
      formula: '当前收盘价 / 20日前收盘价 - 1',
      caliber: '使用前复权日线收盘价计算。',
      caveat: '上涨较多不代表未来仍会上涨，需结合波动与成交情况判断。',
    },
    {
      key: 'mom60',
      name: '近60个交易日涨跌幅',
      description: '衡量约三个月的中期价格趋势。',
      category: 'momentum',
      unit: '%',
      formula: '当前收盘价 / 60日前收盘价 - 1',
      caliber: '使用前复权日线收盘价计算。',
      caveat: '对上市时间较短或长期停牌的股票参考价值有限。',
    },
    {
      key: 'rsi14',
      name: '近期强弱程度',
      description: 'RSI 14 比较最近14日上涨与下跌力度，数值越高表示近期越强。',
      category: 'momentum',
      unit: '0-100',
      formula: '100 - 100 / (1 + 14日平均上涨幅度 / 14日平均下跌幅度)',
      caliber: '通常70以上称为偏热，30以下称为偏弱。',
      caveat: '强趋势中可能长期保持高位或低位，不能单独作为买卖依据。',
    },
    {
      key: 'atr_pct',
      name: '日常价格波动幅度',
      description: '把真实波动幅度换算为价格占比，用于比较不同价位股票的波动。',
      category: 'volatility',
      unit: '%',
      formula: '14日平均真实波幅 / 收盘价',
      caliber: '真实波幅会考虑当日高低价和前一日收盘价。',
      caveat: '波动大只表示不确定性较高，不等于收益更高。',
    },
    {
      key: 'vol_ratio5',
      name: '成交量活跃倍数',
      description: '今日成交量相对过去5日平均成交量的倍数。',
      category: 'liquidity',
      unit: '倍',
      formula: '今日成交量 / 过去5日日均成交量',
      caliber: '1.5 表示约为近期平均成交量的1.5倍。',
      caveat: '放量可能来自上涨、下跌或事件冲击，需要结合价格方向。',
    },
    {
      key: 'ma20_slope',
      name: '20日平均价格趋势',
      description: '观察20日均线正在上升还是下降。',
      category: 'trend',
      unit: '%',
      formula: '当前20日均线 / 5个交易日前20日均线 - 1',
      caliber: '正数表示均线向上，负数表示均线向下。',
      caveat: '均线是滞后指标，快速反转时反应会较慢。',
    },
    {
      key: 'amount_avg20',
      name: '近20日日均成交额',
      description: '衡量股票近期每天平均成交的金额和流动性。',
      category: 'liquidity',
      unit: '元',
      formula: '近20日成交额合计 / 20',
      caliber: '成交额越高，通常越容易以接近市场价格成交。',
      caveat: '历史成交活跃不保证未来持续活跃。',
    },
    {
      key: 'pe_ttm',
      name: '滚动市盈率',
      description: '当前总市值相对最近四个季度净利润的倍数。',
      category: 'valuation',
      unit: '倍',
      formula: '总市值 / 最近四个季度归母净利润',
      caliber: '亏损公司通常没有可比的正市盈率。',
      caveat: '不同行业的合理区间差异很大，低市盈率也可能反映盈利下滑预期。',
    },
    {
      key: 'pb',
      name: '市净率',
      description: '股价相对每股净资产的倍数。',
      category: 'valuation',
      unit: '倍',
      formula: '总市值 / 归母净资产',
      caliber: '金融和重资产行业更常使用该指标比较估值。',
      caveat: '轻资产公司和资产质量较差的公司不适合只看市净率。',
    },
    {
      key: 'roe',
      name: '净资产收益率',
      description: '衡量公司使用股东投入资本创造利润的能力。',
      category: 'quality',
      unit: '%',
      formula: '归母净利润 / 平均归母净资产',
      caliber: '优先使用最近四个季度滚动口径。',
      caveat: '高负债或一次性收益可能推高 ROE，需要结合负债和现金流。',
    },
  ],
  indicators: [
    { key: 'ma', name: '简单移动平均线（MA）', description: '指定交易日窗口内收盘价的算术平均。', category: 'technical', formula: '窗口内收盘价合计 / 窗口交易日数', caveat: '均线属于滞后指标，窗口越长反应越慢。' },
    { key: 'ema', name: '指数移动平均线（EMA）', description: '对近期价格赋予更高权重的移动平均线。', category: 'technical', caveat: '反应更快，也可能受到更多短期噪声影响。' },
    { key: 'macd', name: '指数平滑异同移动平均线（MACD）', description: '比较快慢指数均线，并用信号线观察趋势变化。', category: 'technical', caliber: '常用参数为12、26、9。', caveat: '震荡行情中可能频繁反复。' },
    { key: 'rsi', name: '相对强弱指标（RSI）', description: '比较指定窗口内上涨与下跌的力度。', category: 'technical', unit: '0-100', caveat: '不能把70或30机械地视作买卖点。' },
    { key: 'atr', name: '平均真实波幅（ATR）', description: '综合当日振幅与跳空计算指定窗口的平均波动。', category: 'technical', caveat: '比较不同股价股票时应使用占价格比例。' },
    { key: 'volume_ratio', name: '成交量相对均量', description: '当日成交量相对过去指定窗口平均成交量的倍数。', category: 'technical', unit: '倍', caveat: '本系统为日频口径，不等同于盘中量比。' },
  ],
  strategy_templates: [
    {
      key: 'ma_cross',
      name: '双均线趋势策略',
      description: '短期均线向上穿过长期均线时进入模拟持有，向下穿过时退出。',
      category: 'single',
      caliber: '使用日线收盘后确认的均线状态。',
      caveat: '震荡行情可能频繁产生方向相反的提示。',
    },
    {
      key: 'breakout',
      name: '价格突破策略',
      description: '价格突破过去一段时间高点时进入模拟持有，跌破退出条件时离开。',
      category: 'single',
      caveat: '突破后可能快速回落，需要结合成交量和波动判断。',
    },
    {
      key: 'mean_reversion',
      name: '上升趋势超跌反弹策略',
      description: '在中期趋势仍向上时，寻找短期明显回落后的反弹机会。',
      category: 'single',
      caveat: '趋势真正反转时，短期超跌可能继续下跌。',
    },
    {
      key: 'volume_breakout',
      name: '缩量整理后放量突破策略',
      description: '寻找成交量收缩整理后，价格伴随放量向上突破的股票。',
      category: 'single',
      caveat: '事件驱动的短暂放量可能形成假突破。',
    },
    {
      key: 'momentum_rotation',
      name: '强势股票轮动策略',
      description: '定期比较股票池的中期强弱，模拟持有排名靠前的一组股票。',
      category: 'portfolio',
      caveat: '市场风格快速切换时，过去的强势股票可能回撤较大。',
    },
    {
      key: 'multifactor_hold',
      name: '多指标综合评分持有策略',
      description: '组合动量、趋势、波动和流动性等指标进行排序与模拟持有。',
      category: 'portfolio',
      caveat: '评分结果取决于指标、权重和股票池，不能视为确定性结论。',
    },
  ],
  signals: [
    {
      key: 'buy',
      name: '满足入场规则',
      description: '策略在当日收盘数据上由未模拟持有转为模拟持有。',
      category: 'entry',
      caveat: '这是研究提示，不是订单或交易指令。',
    },
    {
      key: 'sell',
      name: '满足退出规则',
      description: '策略在当日收盘数据上由模拟持有转为未持有。',
      category: 'exit',
      caveat: '真实卖出决定由用户在外部交易软件中自行确认。',
    },
    {
      key: 'watch',
      name: '继续观察',
      description: '条件接近或处于策略关注范围，但未形成入场或退出状态变化。',
      category: 'watch',
      caveat: '观察提示不代表必须采取任何操作。',
    },
    {
      key: 'add',
      name: '上调模拟仓位',
      description: '持有期间加仓规则触发，模拟目标仓位上调一个档位。',
      category: 'entry',
      caveat: '这是研究提示，不是订单或交易指令。',
    },
    {
      key: 'reduce',
      name: '下调模拟仓位',
      description: '持有期间减仓规则触发，模拟目标仓位下调一个档位。',
      category: 'exit',
      caveat: '真实卖出决定由用户在外部交易软件中自行确认。',
    },
  ],
  backtest_metrics: [
    { key: 'total_return', name: '区间总收益', description: '回测期末相对期初的模拟净值变化。', category: 'return', unit: '%', caveat: '历史收益不代表未来收益。' },
    { key: 'annual_return', name: '折算年化收益', description: '把区间收益按时间折算为一年口径，便于比较不同区间。', category: 'return', unit: '%', caveat: '短区间折算后的年化数字可能被明显放大。' },
    { key: 'max_drawdown', name: '最大回撤', description: '模拟净值从历史高点到随后低点的最大跌幅。', category: 'drawdown', unit: '%', caveat: '只描述历史样本中出现过的最差回撤。' },
    { key: 'sharpe', name: '夏普比率', description: '用波动调整后的收益比较策略承担风险后的回报。', category: 'return', caveat: '依赖收益分布和无风险利率假设。' },
    { key: 'win_rate', name: '盈利回合比例', description: '完成买卖回合中盈利回合所占比例。', category: 'trading', unit: '%', caveat: '高胜率不等于高收益，还要看每次盈亏大小。' },
    { key: 'trade_count', name: '模拟成交次数', description: '回测中发生的买入和卖出填充次数。', category: 'trading', caveat: '次数越多，费用和滑点假设对结果影响越大。' },
    { key: 'round_trips', name: '完整买卖回合', description: '完成一次模拟买入和卖出的交易回合数量。', category: 'trading' },
  ],
  filter_fields: [
    { key: 'pct_chg', name: '当日涨跌幅', description: '最新交易日相对前一交易日的价格变化。', category: 'technical', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'mom20', name: '近20日涨跌幅', description: '最近20个交易日的价格变化。', category: 'technical', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'mom60', name: '近60日涨跌幅', description: '最近60个交易日的价格变化。', category: 'technical', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'rsi14', name: '近期强弱程度', description: '14日 RSI，常见区间为0到100。', category: 'technical', unit: '0-100', data_type: 'number', operators: numberOperators },
    { key: 'vol_ratio5', name: '成交量活跃倍数', description: '今日成交量相对5日平均成交量的倍数。', category: 'technical', unit: '倍', data_type: 'number', operators: numberOperators },
    { key: 'amount_avg20', name: '近20日日均成交额', description: '近20日平均每天成交的金额。', category: 'technical', unit: '亿元', data_type: 'number', input_scale: 100000000, operators: numberOperators },
    { key: 'ma_bull', name: '均线保持向上排列', description: '短期均线在长期均线上方，表示近期趋势偏上。', category: 'technical', data_type: 'boolean', operators: ['eq'] },
    { key: 'high_dist', name: '距离近期最高价', description: '当前价格距离指定窗口最高价的幅度。', category: 'technical', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'pe_ttm', name: '滚动市盈率', description: '总市值相对最近四个季度净利润的倍数。', category: 'valuation', unit: '倍', data_type: 'number', operators: numberOperators },
    { key: 'pb', name: '市净率', description: '总市值相对净资产的倍数。', category: 'valuation', unit: '倍', data_type: 'number', operators: numberOperators },
    { key: 'ps_ttm', name: '滚动市销率', description: '总市值相对最近四个季度营业收入的倍数。', category: 'valuation', unit: '倍', data_type: 'number', operators: numberOperators },
    { key: 'dividend_yield', name: '股息率', description: '当前数据源未稳定提供，本字段不维护。', category: 'valuation', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators, available: false },
    { key: 'total_market_cap', name: '总市值', description: '全部已发行股份按当前价格计算的价值。', category: 'valuation', unit: '亿元', data_type: 'number', input_scale: 100000000, operators: numberOperators },
    { key: 'roe', name: '净资产收益率', description: '公司使用股东资本创造利润的能力。', category: 'quality', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'gross_margin', name: '毛利率', description: '营业收入扣除营业成本后剩余的比例。', category: 'quality', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'net_margin', name: '净利率', description: '归母净利润占营业收入的比例。', category: 'quality', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'cashflow_ratio', name: '经营现金流与净利润比', description: '经营活动现金流净额相对净利润的比例，用于观察利润含金量。', category: 'quality', unit: '倍', data_type: 'number', operators: numberOperators },
    { key: 'revenue_yoy', name: '营业收入增长率', description: '营业收入相对上年同期的变化。', category: 'growth', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'profit_yoy', name: '净利润增长率', description: '归母净利润相对上年同期的变化。', category: 'growth', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'debt_ratio', name: '资产负债率', description: '总负债占总资产的比例。', category: 'risk', unit: '%', data_type: 'number', input_scale: 0.01, operators: numberOperators },
    { key: 'industry', name: '所属行业', description: '股票所属的行业分类。', category: 'basic', data_type: 'string', operators: ['eq', 'ne', 'contains'] },
    { key: 'is_st', name: '风险警示股票', description: '是否带有 ST 或其他风险警示标识。', category: 'basic', data_type: 'boolean', operators: ['eq'] },
    { key: 'listing_days', name: '上市交易天数', description: '从上市日起计算的交易天数。', category: 'basic', unit: '天', data_type: 'number', operators: numberOperators },
  ],
}

const catalog = shallowRef<CatalogPayload>(fallbackCatalog)
const loading = shallowRef(false)
const loaded = shallowRef(false)
const usingFallback = shallowRef(true)

function normalizeEntries(remote: CatalogEntry[] | undefined, fallback: CatalogEntry[]): CatalogEntry[] {
  if (!remote?.length) return fallback
  const fallbackMap = new Map(fallback.map((entry) => [entry.key, entry]))
  const merged = remote.map((entry) => ({
    ...fallbackMap.get(entry.key),
    ...entry,
    caveat: entry.caveat ?? entry.limits ?? fallbackMap.get(entry.key)?.caveat,
    data_type: entry.data_type ?? entry.value_type ?? fallbackMap.get(entry.key)?.data_type,
  }))
  const remoteKeys = new Set(remote.map((entry) => entry.key))
  return [...merged, ...fallback.filter((entry) => !remoteKeys.has(entry.key))]
}

export async function loadCatalog(): Promise<CatalogPayload> {
  if (loaded.value || loading.value) return catalog.value
  loading.value = true
  try {
    const remote = await api.catalog()
    catalog.value = {
      factors: normalizeEntries(remote.factors, fallbackCatalog.factors),
      indicators: normalizeEntries(remote.indicators, fallbackCatalog.indicators),
      strategy_templates: normalizeEntries(remote.strategy_templates, fallbackCatalog.strategy_templates),
      signals: normalizeEntries(remote.signals, fallbackCatalog.signals),
      backtest_metrics: normalizeEntries(remote.backtest_metrics, fallbackCatalog.backtest_metrics),
      filter_fields: normalizeEntries(remote.filter_fields, fallbackCatalog.filter_fields),
    }
    usingFallback.value = false
  } catch {
    catalog.value = fallbackCatalog
    usingFallback.value = true
  } finally {
    loading.value = false
    loaded.value = true
  }
  return catalog.value
}

export function resetCatalog() {
  catalog.value = fallbackCatalog
  loading.value = false
  loaded.value = false
  usingFallback.value = true
}

export function catalogEntry(section: CatalogSection, key: string): CatalogEntry | undefined {
  return catalog.value[section].find((entry) => entry.key === key)
}

export function catalogName(section: CatalogSection, key: string): string {
  return catalogEntry(section, key)?.name ?? key
}

export function factorName(key: string): string {
  return catalogName('factors', key)
}

/** 算法模板的中文名。策略实例的名字由用户自定,取策略行的 name */
export function templateName(key: string): string {
  return catalogName('strategy_templates', key)
}

export function signalName(key: string): string {
  return catalogName('signals', key)
}

export function metricName(key: string): string {
  const base = key.replace(/_(mean|median)$/, '')
  const suffix = key.endsWith('_mean') ? '（平均）' : key.endsWith('_median') ? '（中位数）' : ''
  return `${catalogName('backtest_metrics', base)}${suffix}`
}

export function reasonText(reason: Record<string, unknown>, fallback = ''): string {
  if (fallback) return fallback
  const previous = reason.prev_position
  const current = reason.cur_position
  if (previous === 0 && current === 1) return '策略状态从“未模拟持有”变为“模拟持有”，因此产生入场提示。'
  if (previous === 1 && current === 0) return '策略状态从“模拟持有”变为“未持有”，因此产生退出提示。'
  if (typeof previous === 'number' && typeof current === 'number' && previous > 0) {
    if (current > previous) {
      return `持有期间加仓规则触发，模拟目标仓位从 ${+previous.toFixed(4)} 上调至 ${+current.toFixed(4)}。`
    }
    if (current > 0 && current < previous) {
      return `持有期间减仓规则触发，模拟目标仓位从 ${+previous.toFixed(4)} 下调至 ${+current.toFixed(4)}。`
    }
  }

  return Object.entries(reason)
    .map(([key, value]) => {
      const label = catalogEntry('factors', key)?.name ?? {
        prev_position: '此前策略状态',
        cur_position: '当前策略状态',
        fast: '短期参数',
        slow: '长期参数',
        price: '信号日收盘价',
        close: '收盘价',
      }[key] ?? key
      const text = typeof value === 'number' ? String(+value.toFixed(4)) : String(value)
      return `${label}：${text}`
    })
    .join('；')
}

export function useCatalog() {
  return {
    catalog: shallowReadonly(catalog),
    loading: readonly(loading),
    loaded: readonly(loaded),
    usingFallback: readonly(usingFallback),
    load: loadCatalog,
    reset: resetCatalog,
  }
}
