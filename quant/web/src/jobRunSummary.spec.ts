/**
 * 定时任务结果摘要:断言页面上不再出现裸 JSON。
 *
 * 用例里的 result 形状直接照抄后端真实返回(app/data/calendar.py、
 * ingest.py、fundamentals.py、research_plan/retention.py),
 * 否则测试通过但线上仍旧渲染 JSON。
 */
import { describe, expect, it } from 'vitest'
import type { AdminJobRun } from './api'
import { rawResult, summarizeJobRun } from './jobRunSummary'

function run(result: unknown, overrides: Partial<AdminJobRun> = {}): AdminJobRun {
  return {
    status: 'finished',
    started_at: '2026-08-03T03:09:59',
    finished_at: '2026-08-03T03:10:00',
    result,
    ...overrides,
  }
}

/** 摘要里不允许出现 JSON 的结构字符 */
function expectNoJson(text: string) {
  expect(text).not.toMatch(/[{}[\]"]/)
}

describe('summarizeJobRun', () => {
  it('交易日历同步不再渲染原始 JSON', () => {
    // 截图里的实际值:{"start":"2026-01-01","end":"2027-01-31",...}
    const text = summarizeJobRun('sync_trade_calendar', run({
      start: '2026-01-01', end: '2027-01-31',
      days: 365, open_days: 242, changed: 0, skipped: false,
    }))

    expectNoJson(text)
    expect(text).toContain('2026-01-01 ~ 2027-01-31')
    expect(text).toContain('覆盖 365 天')
    expect(text).toContain('交易日 242 天')
    expect(text).toContain('无变更')
  })

  it('交易日历有变更时报出变更行数', () => {
    const text = summarizeJobRun('sync_trade_calendar', run({
      start: '2026-01-01', end: '2027-01-31',
      days: 365, open_days: 242, changed: 7, skipped: false,
    }))
    expect(text).toContain('变更 7 行')
  })

  it('全市场估值快照不再渲染原始 JSON', () => {
    // 截图里的实际值:{"date":"2026-07-31","requests":6,"fetched":5532,...}
    const text = summarizeJobRun('sync_valuations', run({
      date: '2026-07-31', requests: 6, fetched: 5532,
      upserted: 5532, industry_updated: 0,
      coverage: { pe_ttm: 5532, pb: 5531 },
    }))

    expectNoJson(text)
    expect(text).toContain('2026-07-31')
    expect(text).toContain('拉取 5532 只')
    expect(text).toContain('入库 5532 行')
    // coverage 是嵌套 dict,绝不能被拼进摘要
    expect(text).not.toContain('pe_ttm')
  })

  it('名录同步汇总新增/更新/退市', () => {
    const text = summarizeJobRun('sync_stock_list', run({
      imported: 3, updated: 12, delisted: 1, total: 5532,
      reconciled_inserted: 0, reconciled_delist_fixed: 0,
    }))
    expectNoJson(text)
    expect(text).toContain('名录共 5532 只')
    expect(text).toContain('新增 3')
    expect(text).toContain('新标退市 1')
  })

  it('财务指标同步汇总报告期数,不展开 periods 数组', () => {
    const text = summarizeJobRun('sync_fundamentals', run({
      periods: [
        { report_period: '2025-06-30', fetched: 5000, upserted: 5000, requests: 8, coverage: {} },
        { report_period: '2025-09-30', fetched: 5100, upserted: 5100, requests: 8, coverage: {} },
      ],
      period_count: 2, requests: 16, upserted: 10100,
    }))
    expectNoJson(text)
    expect(text).toContain('报告期 2 个')
    expect(text).toContain('入库 10100 行')
    expect(text).not.toContain('report_period')
  })

  it('研究计划保留策略区分「无需清理」与实际清理', () => {
    expect(summarizeJobRun('prune_research_plans', run({
      chains: 42, candidates: 0, deleted: 0, protected_kept: 0,
    }))).toContain('无需清理')

    const text = summarizeJobRun('prune_research_plans', run({
      chains: 42, candidates: 9, deleted: 7, protected_kept: 2,
    }))
    expect(text).toContain('清理 7 版')
    expect(text).toContain('信号引用保留 2 版')
  })

  it('盘后流水线汇总行情覆盖情况', () => {
    const text = summarizeJobRun('evening_pipeline', run({
      skipped: false, requested: 5532, succeeded: 5530,
      failed: ['sh.600000', 'sz.000001'], empty: [], empty_ratio: 0,
    }))
    expectNoJson(text)
    expect(text).toContain('请求 5532 只')
    expect(text).toContain('成功 5530 只')
    expect(text).toContain('失败 2 只')
    // 代码列表本身不该铺在摘要里
    expect(text).not.toContain('sh.600000')
  })

  it('任务内部守卫跳过时给出原因,而不是 skipped:true', () => {
    const text = summarizeJobRun('evening_pipeline', run({
      skipped: true, succeeded: 0, failed: [], empty: [],
    }))
    expect(text).toBe('已跳过(非交易日或数据源无数据)')
  })

  it('状态优先于结果:执行中与失败', () => {
    expect(summarizeJobRun('sync_valuations', run(null, {
      status: 'running', finished_at: null,
    }))).toBe('执行中…')

    expect(summarizeJobRun('sync_valuations', run(null, {
      status: 'failed', error: '远端超时',
    }))).toBe('失败: 远端超时')

    // failed 但没有 error 文本时不能显示 undefined
    expect(summarizeJobRun('sync_valuations', run(null, {
      status: 'failed',
    }))).toBe('失败: 未知错误')
  })

  it('无结果的任务显示已完成', () => {
    expect(summarizeJobRun('sync_index_members', run(null))).toBe('已完成')
    expect(summarizeJobRun('intraday_snapshot', run(undefined))).toBe('已完成')
  })

  it('未登记的任务也不吐 JSON(兜底摘要)', () => {
    // 成分股同步返回嵌套 dict,且未在 FORMATTERS 里登记
    const text = summarizeJobRun('sync_index_members', run({
      hs300: { index: 'hs300', remote: 300, added: 0, removed: 0 },
      zz500: { index: 'zz500', remote: 500, added: 0, removed: 0 },
    }))
    expectNoJson(text)
  })

  it('兜底摘要拼数值字段而非序列化', () => {
    const text = summarizeJobRun('some_new_job', run({ inserted: 5, skipped_rows: 2 }))
    expectNoJson(text)
    expect(text).toContain('inserted 5')
  })

  it('非对象结果安全降级', () => {
    expect(summarizeJobRun('x', run(42))).toBe('已完成')
    expect(summarizeJobRun('x', run([1, 2, 3]))).toBe('已完成')
    expect(summarizeJobRun('x', run('清理 3 条'))).toBe('清理 3 条')
  })

  it('缺字段时省略对应片段,不出现 NaN/undefined', () => {
    const text = summarizeJobRun('sync_valuations', run({ upserted: 10 }))
    expect(text).toBe('入库 10 行')
    expect(text).not.toMatch(/NaN|undefined|null/)
  })
})

describe('rawResult', () => {
  it('原始 JSON 供 title 排障', () => {
    expect(rawResult(run({ days: 365 }))).toContain('"days": 365')
  })

  it('无结果时为空串(不渲染 title)', () => {
    expect(rawResult(run(null))).toBe('')
  })

  it('循环引用不抛', () => {
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect(rawResult(run(cyclic))).toBe('')
  })
})
