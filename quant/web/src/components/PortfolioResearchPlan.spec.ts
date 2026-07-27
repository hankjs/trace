import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { ResearchPlanSummary } from '../api'
import PortfolioResearchPlan from './PortfolioResearchPlan.vue'

describe('PortfolioResearchPlan', () => {
  it('shows target-weight changes, cash and structured reasons', () => {
    const plan: ResearchPlanSummary = {
      id: 9,
      type: 'portfolio_rebalance',
      status: 'active',
      data_date: '2026-07-24',
      rebalance: {
        pool_name: '沪深 300',
        frequency: '每周',
        plan_date: '2026-07-24',
        next_simulated_trade_date: '2026-07-27',
        cash_weight: 0.1,
        changes: [{
          code: 'sh.600519',
          name: '贵州茅台',
          change_type: 'increase',
          previous_weight: 0.05,
          target_weight: 0.1,
          reasons: ['综合评分进入前十', '通过风险过滤'],
          score_details: {
            mom20: {
              name: '20日动量', value: 0.12, weight: 0.4,
              contribution: 0.048,
            },
          },
          risk_rules: [{
            id: 'risk-overlay',
            source: 'risk_overlay',
            name: '风险覆盖层',
            summary: '基于逐股模拟入场价和固定比例计算。',
            data_date: '2026-07-24',
            price_reference: {
              source: 'risk_overlay',
              name: '风险失效线',
              value: 9.18,
              data_date: '2026-07-24',
            },
          }],
        }],
      },
    }

    const text = mount(PortfolioResearchPlan, { props: { plan } }).text()
    expect(text).toContain('调仓研究计划')
    expect(text).toContain('调仓目标权重')
    expect(text).toContain('增仓')
    expect(text).toContain('综合评分进入前十')
    expect(text).toContain('评分分解：20日动量 0.0480')
    expect(text).toContain('风险覆盖层')
    expect(text).toContain('9.18')
    expect(text).toContain('现金权重')
    expect(text).toContain('外部交易应用')
  })
})
