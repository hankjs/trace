import type {
  PortfolioChangeType,
  ResearchPlanStatus,
  ResearchPriceReference,
  StrategyOverlayConfig,
  StrategyParamValue,
} from './api'

export const RESEARCH_PLAN_BOUNDARY = '本计划根据日频数据和策略规则生成，仅用于研究。真实买卖、价格、数量和风险决策由你在外部交易应用中确认。'

export const DEFAULT_RISK_OVERLAY: StrategyOverlayConfig = {
  enabled: false,
  type: 'fixed_pct',
  value: 0.08,
  atr_period: 14,
}

export const DEFAULT_TAKE_PROFIT: StrategyOverlayConfig = {
  enabled: false,
  type: 'fixed_pct',
  value: 0.20,
  atr_period: 14,
}

const statusNames: Record<ResearchPlanStatus, string> = {
  active: '当前有效',
  needs_review: '需要重新评估',
  invalidated: '已失效',
  exit_triggered: '已触发退出',
  expired: '已过期',
}

const changeNames: Record<PortfolioChangeType, string> = {
  new: '新增',
  keep: '保留',
  increase: '增仓',
  decrease: '减仓',
  remove: '清仓',
  risk_filtered: '风险过滤',
}

export function researchPlanStatusName(status?: ResearchPlanStatus | null): string {
  return status ? statusNames[status] : '计划待生成'
}

export function portfolioChangeName(change: PortfolioChangeType, fallback?: string): string {
  return fallback || changeNames[change]
}

export function isOverlayConfig(value: StrategyParamValue | undefined): value is StrategyOverlayConfig {
  if (!value || typeof value !== 'object') return false
  return typeof value.enabled === 'boolean'
    && (value.type === 'fixed_pct' || value.type === 'atr_multiple')
    && typeof value.value === 'number'
}

export function overlayFromParams(
  params: Record<string, StrategyParamValue> | null | undefined,
  key: 'risk_overlay' | 'take_profit'
): StrategyOverlayConfig {
  const fallback = key === 'risk_overlay' ? DEFAULT_RISK_OVERLAY : DEFAULT_TAKE_PROFIT
  const value = params?.[key]
  return isOverlayConfig(value)
    ? { ...fallback, ...value, atr_period: value.atr_period ?? fallback.atr_period }
    : { ...fallback }
}

/**
 * 关闭的全新覆盖层不写入参数；已有覆盖层关闭时保留 enabled=false，确保能覆盖旧值。
 */
export function overlayParamSnapshot(
  risk: StrategyOverlayConfig,
  takeProfit: StrategyOverlayConfig,
  source?: Record<string, StrategyParamValue> | null
): Record<string, StrategyParamValue> {
  const result: Record<string, StrategyParamValue> = {}
  if (risk.enabled || isOverlayConfig(source?.risk_overlay)) result.risk_overlay = { ...risk }
  if (takeProfit.enabled || isOverlayConfig(source?.take_profit)) result.take_profit = { ...takeProfit }
  return result
}

export function overlaySummary(config: StrategyOverlayConfig, kind: 'risk' | 'take_profit'): string {
  if (!config.enabled) {
    return kind === 'risk'
      ? '未启用统一风险覆盖层，仍保留模板原生风险规则。'
      : '未设置止盈，按风险规则或策略原生条件退出。'
  }
  const action = kind === 'risk' ? '形成风险退出状态' : '形成止盈退出状态'
  if (config.type === 'atr_multiple') {
    return `以 ${config.atr_period} 日 ATR 的 ${config.value} 倍作为覆盖层，T 日收盘确认后于 T+1 开盘模拟${action}。`
  }
  return `相对模拟入场价${kind === 'risk' ? '回落' : '上涨'} ${(config.value * 100).toFixed(2)}% 时，T 日收盘确认后于 T+1 开盘模拟${action}。`
}

export function priceReferenceText(reference: ResearchPriceReference): string {
  if (reference.lower != null && reference.upper != null) {
    return `${reference.lower.toFixed(2)} 至 ${reference.upper.toFixed(2)}`
  }
  if (reference.value != null) return reference.value.toFixed(2)
  return '按条件判断，未生成价格线'
}
