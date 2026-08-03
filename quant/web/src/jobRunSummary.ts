/**
 * 定时任务执行结果 → 中文状态描述。
 *
 * 各 job 的返回值形状互不相同(见 app/scheduler.py 的 JOB_DEFS),
 * 页面上原先直接 JSON.stringify 截断到 120 字符,于是「交易日历同步」
 * 这类任务在表格里显示成 {"start":"2026-01-01","end":... 的原始 JSON。
 * 这里按 job_id 归一成人话;未登记的 job 或形状对不上时降级为通用摘要,
 * 保证新增任务不会又冒出裸 JSON。
 *
 * 原始 JSON 仍通过 `rawResult` 暴露,挂在单元格 title 上供排障。
 */
import type { AdminJobRun } from './api'

type Dict = Record<string, unknown>

function isDict(value: unknown): value is Dict {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** 「共 5532 只」这类计数片段;值缺失时返回 null 以便整段省略。 */
function count(label: string, value: unknown, unit = ''): string | null {
  const n = num(value)
  return n === null ? null : `${label} ${n}${unit}`
}

function join(parts: (string | null)[]): string {
  return parts.filter((p): p is string => p !== null && p !== '').join(',')
}

/** 数组长度(失败/空列表这类字段) */
function len(value: unknown): number | null {
  return Array.isArray(value) ? value.length : null
}

function summarizeTradeCalendar(r: Dict): string {
  const changed = num(r.changed)
  const parts = [
    count('覆盖', r.days, ' 天'),
    count('交易日', r.open_days, ' 天'),
    changed === null ? null : changed > 0 ? `变更 ${changed} 行` : '无变更',
  ]
  const range = r.start && r.end ? `${String(r.start)} ~ ${String(r.end)}` : null
  return join([range, ...parts]) || '已完成'
}

function summarizeStockList(r: Dict): string {
  return join([
    count('名录共', r.total, ' 只'),
    count('新增', r.imported),
    count('更新', r.updated),
    count('新标退市', r.delisted),
  ]) || '已完成'
}

function summarizeValuations(r: Dict): string {
  return join([
    r.date ? String(r.date) : null,
    count('拉取', r.fetched, ' 只'),
    count('入库', r.upserted, ' 行'),
    count('行业更新', r.industry_updated),
    count('请求', r.requests, ' 次'),
  ]) || '已完成'
}

function summarizeFundamentals(r: Dict): string {
  return join([
    count('报告期', r.period_count, ' 个'),
    count('入库', r.upserted, ' 行'),
    count('请求', r.requests, ' 次'),
  ]) || '已完成'
}

function summarizePrunePlans(r: Dict): string {
  const deleted = num(r.deleted)
  return join([
    count('研究计划链', r.chains, ' 条'),
    deleted === null ? null : deleted > 0 ? `清理 ${deleted} 版` : '无需清理',
    count('信号引用保留', r.protected_kept, ' 版'),
  ]) || '已完成'
}

/** 盘后日线(手动触发 job_daily_bars 时可见) */
function summarizeDailyBars(r: Dict): string {
  const failed = len(r.failed)
  const empty = len(r.empty)
  return join([
    count('请求', r.requested, ' 只'),
    count('成功', r.succeeded, ' 只'),
    failed ? `失败 ${failed} 只` : null,
    empty ? `无当日行情 ${empty} 只` : null,
  ]) || '已完成'
}

const FORMATTERS: Record<string, (r: Dict) => string> = {
  sync_trade_calendar: summarizeTradeCalendar,
  sync_stock_list: summarizeStockList,
  sync_valuations: summarizeValuations,
  sync_fundamentals: summarizeFundamentals,
  prune_research_plans: summarizePrunePlans,
  evening_pipeline: summarizeDailyBars,
}

/** 兜底:把 dict 里的数值字段拼成「键 值」,绝不吐 JSON。 */
function summarizeGeneric(r: Dict): string {
  const parts: string[] = []
  for (const [key, value] of Object.entries(r)) {
    const n = num(value)
    if (n !== null) parts.push(`${key} ${n}`)
    else if (Array.isArray(value) && value.length) parts.push(`${key} ${value.length}`)
    if (parts.length >= 4) break
  }
  return parts.join(',') || '已完成'
}

/**
 * 一条执行记录的中文摘要。
 *
 * 顺序要紧:running/failed 先判,再看 skipped(非交易日/时间窗外任务
 * 自行跳过,状态仍是 finished),最后才按 job 归一结果。
 */
export function summarizeJobRun(jobId: string, record: AdminJobRun): string {
  if (record.status === 'running') return '执行中…'
  if (record.status === 'failed') return `失败: ${record.error ?? '未知错误'}`

  const result = record.result
  if (result == null) return '已完成'
  if (typeof result === 'string') return result
  if (!isDict(result)) return '已完成'

  if (result.skipped === true) {
    // 各 job 内部守卫:非交易日、盘中时间窗外、远端返回空结果
    return '已跳过(非交易日或数据源无数据)'
  }

  const formatter = FORMATTERS[jobId] ?? summarizeGeneric
  return formatter(result)
}

/** 原始结果 JSON,挂在 title 上供排障;不可序列化时返回空串。 */
export function rawResult(record: AdminJobRun): string {
  if (record.result == null) return ''
  try {
    return JSON.stringify(record.result, null, 2)
  } catch {
    return ''
  }
}
