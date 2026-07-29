/** 数字格式化与涨跌配色(A股:涨红跌绿)。 */

export function fmtPrice(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  return v.toFixed(2)
}

export function fmtQty(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  return v % 1 === 0 ? String(v) : v.toFixed(2)
}

/** 金额千分位,2 位小数 */
export function fmtAmount(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  return v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}

/** 大金额缩写:1.23亿 / 4567万 */
export function fmtBigAmount(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  const abs = Math.abs(v)
  if (abs >= 1e8) return (v / 1e8).toFixed(2) + '亿'
  if (abs >= 1e4) return (v / 1e4).toFixed(2) + '万'
  return v.toFixed(2)
}

/** 百分比:v 为小数(0.0123 -> +1.23%)。用于涨跌/收益,带符号。 */
export function fmtPct(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  if (v === 0) return '0.00%'
  return (v > 0 ? '+' : '') + (v * 100).toFixed(2) + '%'
}

/**
 * 覆盖率:v 为 0~1 小数(0.1105 -> 11.1%)。
 * 无正负号;勿用 fmtPct(覆盖率会变成 +11.05%)。
 */
export function fmtCoverage(v: number | null | undefined, digits = 1): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  const pct = Math.min(1, Math.max(0, v)) * 100
  return pct.toFixed(digits) + '%'
}

/** 带符号金额 */
export function fmtSigned(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  return (v >= 0 ? '+' : '') + fmtAmount(v)
}

/** 涨跌 Tailwind 文本色类:涨红跌绿,平/空为次要色 */
export function pnlClass(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v) || v === 0) return 'text-text-secondary'
  return v > 0 ? 'text-up' : 'text-down'
}

/** 聚合指标按 mean -> 原字段 -> median 的固定口径读取。 */
export function aggregateMetric(
  metrics: Record<string, unknown> | null | undefined,
  key: string
): number | undefined {
  const source = metrics ?? {}
  for (const candidate of [`${key}_mean`, key, `${key}_median`]) {
    const value = source[candidate]
    if (typeof value === 'number' && !Number.isNaN(value)) return value
  }
  return undefined
}

/** 浏览器本地日期，避免中国时区凌晨被 UTC ISO 字符串回退一天。 */
export function localDateISO(value = new Date()): string {
  const year = value.getFullYear()
  const month = String(value.getMonth() + 1).padStart(2, '0')
  const day = String(value.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}
