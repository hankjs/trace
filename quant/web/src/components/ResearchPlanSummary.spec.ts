import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { ResearchPlan } from '../api'
import ResearchPlanSummary from './ResearchPlanSummary.vue'

const plan: ResearchPlan = {
  id: 7,
  type: 'single',
  status: 'needs_review',
  status_reason: '开盘价越过进场观察区间',
  data_date: '2026-07-24',
  next_simulated_trade_date: '2026-07-27',
  signal_close_price: 12.34,
  strategy_id: 3,
  strategy_name: '价格突破研究',
  template: 'breakout',
  strategy_version: 'v2',
  params_snapshot: { window: 20 },
  adjustment: '前复权',
  entry: {
    mode: 'range',
    summary: '观察是否仍处于客观突破区间。',
    data_date: '2026-07-24',
    range: { source: 'entry', name: '进场观察区间', lower: 12, upper: 12.5, data_date: '2026-07-24' },
  },
  risk_rules: [],
  take_profit_rules: [],
  native_exit_rules: [],
  evidence: { status: 'unverified', exact_match: false },
}

describe('ResearchPlanSummary', () => {
  it('keeps the required reading order and shows traceability', () => {
    const text = mount(ResearchPlanSummary, { props: { plan } }).text()
    const headings = ['数据与信号', '进场观察', '风险与退出', '历史回测对照', '产品边界']
    const offsets = headings.map((heading) => text.indexOf(heading))

    expect(offsets.every((offset) => offset >= 0)).toBe(true)
    expect(offsets).toEqual([...offsets].sort((a, b) => a - b))
    expect(text).toContain('需要重新评估')
    expect(text).toContain('开盘价越过进场观察区间')
    expect(text).toContain('前复权')
    expect(text).toContain('不是建议成交价')
    expect(text).toContain('尚无匹配回测')
  })

  it('shows fee assumptions for exact backtest evidence', () => {
    const wrapper = mount(ResearchPlanSummary, {
      props: {
        plan: {
          ...plan,
          evidence: {
            status: 'verified',
            exact_match: true,
            backtest_id: 7,
            start: '2024-01-01',
            end: '2025-12-31',
            metrics: { total_return: 0.12, max_drawdown: -0.08, win_rate: 0.55, trade_count: 20 },
            costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
          },
        },
      },
    })

    expect(wrapper.text()).toContain('费用假设：佣金 0.03%，印花税 0.05%，滑点 0.01%')
  })

  it('does not present a holding snapshot as a new signal', () => {
    const wrapper = mount(ResearchPlanSummary, {
      props: {
        plan: {
          ...plan,
          signal_type: 'hold',
          signal_close_price: null,
          evidence: {
            status: 'unverified', exact_match: false,
            costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
          },
        },
      },
    })

    expect(wrapper.text()).toContain('持仓快照未生成新信号')
    expect(wrapper.text()).toContain('待对照费用口径')
  })
})
