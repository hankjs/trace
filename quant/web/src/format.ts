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

/** 百分比:v 为小数(0.0123 -> +1.23%) */
export function fmtPct(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  if (v === 0) return '0.00%'
  return (v > 0 ? '+' : '') + (v * 100).toFixed(2) + '%'
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
