import { describe, expect, it } from 'vitest'
import { normalizeResearchPlanResponse } from './api'

describe('research plan API normalization', () => {
  it('maps the persisted snapshot contract into the frontend reading model', () => {
    const plan = normalizeResearchPlanResponse({
      plan_id: 21,
      plan_type: 'single',
      status: 'reevaluate',
      status_name: '需要重新评估',
      status_reason: { code: 'insufficient_data', text: '历史窗口不足。' },
      data_date: '2026-07-24',
      next_simulated_execution_date: '2026-07-27',
      signal_close_price: 10.5,
      entry_observation: {
        kind: 'range',
        explanation: '过去 20 个交易日最高价形成客观观察线。',
        calculation_status: 'calculated',
        data_date: '2026-07-24',
        line: 10.4,
        lower: 10.4,
        upper: 10.6,
      },
      strategy: { id: 3, name: '突破研究', template: 'breakout', kind: 'single', version: 'rp1-test' },
      params_snapshot: {
        effective_params: {
          entry: 20,
          exit: 10,
          risk_overlay: { enabled: false, type: 'fixed_pct', value: 0.08, atr_period: 14 },
          take_profit: { enabled: false, type: 'fixed_pct', value: 0.2, atr_period: 14 },
        },
      },
      price_adjustment: 'forward',
      risk_rules: [{
        source: 'native',
        name: '模板原生风险',
        condition: '收盘价跌破过去 10 个交易日最低价',
        reference_line: 9.8,
        data_date: '2026-07-24',
      }],
      take_profit: { enabled: false, calculation_status: 'disabled' },
      native_exit: [{
        source: 'native', name: '策略退出条件', condition: '收盘价跌破区间低点',
        reference_line: 9.8, data_date: '2026-07-24',
      }],
      backtest_evidence: { status: 'unverified', reason: '未找到同配置回测。' },
      portfolio_changes: [],
    })

    expect(plan.id).toBe(21)
    expect(plan.status).toBe('needs_review')
    expect(plan.status_reason).toBe('历史窗口不足。')
    expect(plan.entry?.range?.lower).toBe(10.4)
    expect(plan.risk_rules?.[0].price_reference?.value).toBe(9.8)
    expect(plan.evidence?.exact_match).toBe(false)
    expect('adjustment' in plan && plan.adjustment).toBe('前复权')
  })

  it('maps portfolio change verbs and weights without inventing price ranges', () => {
    const plan = normalizeResearchPlanResponse({
      plan_id: 31,
      plan_type: 'portfolio_rebalance',
      status: 'current',
      data_date: '2026-07-24',
      next_simulated_execution_date: '2026-07-27',
      entry_observation: { kind: 'portfolio_rebalance', explanation: '按排名调仓。' },
      portfolio_summary: { pool_name: '沪深 300', frequency: '每周', cash_weight: 0.1 },
      portfolio_changes: [{
        code: 'sh.600519', name: '贵州茅台', previous_weight: 0.05,
        target_weight: 0.1, change_type: 'increased', reasons: ['排名上升'],
        score_details: { factors: {
          mom20: { name: '20日动量', value: 0.12, weight: 0.4, contribution: 0.048 },
        } },
        risk_snapshot: { rules: [{
          source: 'risk_overlay', name: '风险覆盖层',
          reference_line: 9.18, data_date: '2026-07-24',
          calculation_status: 'calculated', explanation: '固定比例风险线。',
        }] },
      }],
    })

    expect(plan.status).toBe('active')
    expect(plan.rebalance?.changes[0].change_type).toBe('increase')
    expect(plan.rebalance?.cash_weight).toBe(0.1)
    expect(plan.rebalance?.changes[0].score_details?.mom20.contribution).toBe(0.048)
    expect(plan.rebalance?.changes[0].risk_rules?.[0].price_reference?.value).toBe(9.18)
    expect(plan.entry?.range).toBeNull()
  })
})
