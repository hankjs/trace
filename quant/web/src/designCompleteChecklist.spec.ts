import { describe, expect, it } from 'vitest'
import {
  designCompleteReady,
  evaluateDesignCompleteChecklist,
} from './designCompleteChecklist'
import type { StrategySpec } from './api'

function baseSpec(overrides: Partial<StrategySpec> = {}): StrategySpec {
  return {
    schema_version: 1,
    kind: 'single',
    metadata: {
      canonical_id: 'T',
      sources: [{ book: 'x', candidate_id: 'y' }],
      evidence_status: 'unverified',
      hypothesis: '这是一条足够长的研究假说用于通过长度检查。',
    },
    universe: {
      pool_id: 2,
      exclude_st: true,
      min_listing_days: 60,
      min_amount_avg20: 0,
    },
    data_requirements: [{ field: 'close', availability: 'daily_close', required: true }],
    entry: {
      condition: { op: 'literal', value: true },
      reason_code: 'always',
    },
    positioning: { type: 'binary', target: 1 },
    holding: {
      allow_add: false,
      allow_reduce: false,
      cooldown_days: 0,
      risk_reentry: 'native_reset',
    },
    native_exit: {
      condition: { op: 'literal', value: false },
      reason_code: 'never',
    },
    overlays: {
      risk: { enabled: false, type: 'fixed_pct', value: 0.08, atr_period: 14, trailing: false },
      take_profit: { enabled: false, type: 'fixed_pct', value: 0.2, atr_period: 14, trailing: false },
    },
    portfolio_constraints: {
      long_only: true,
      max_positions: 500,
      max_single_weight: 1,
      max_total_weight: 1,
    },
    execution: {
      signal_time: 'close',
      execution_time: 'next_open',
      buy_limit_policy: 'reject',
      sell_limit_policy: 'retry',
      suspension_policy: 'reject_entry_retry_exit',
      missing_bar_policy: 'reject_entry_retry_exit',
      cost_model: 'a_share_daily_v1',
      max_entry_premium: 0,
    },
    validation: {
      baseline_ids: ['buy_and_hold', 'equal_weight'],
      locked_oos: true,
      rejection_criteria: ['capacity_failure'],
      parameter_scans: [],
    },
    ...overrides,
  } as StrategySpec
}

describe('designCompleteChecklist', () => {
  it('passes full valid shell', () => {
    const checks = evaluateDesignCompleteChecklist(baseSpec(), true)
    expect(designCompleteReady(checks)).toBe(true)
  })

  it('fails short hypothesis and unlocked oos', () => {
    const short = baseSpec()
    short.metadata.hypothesis = '太短了'
    const checks = evaluateDesignCompleteChecklist(short, true)
    expect(checks.find((c) => c.id === 'HYP_LEN')?.ok).toBe(false)

    const unlocked = baseSpec()
    unlocked.validation.locked_oos = false
    expect(
      evaluateDesignCompleteChecklist(unlocked, true).find((c) => c.id === 'LOCKED_OOS')?.ok,
    ).toBe(false)
  })

  it('fails unknown rejection and baseline', () => {
    const bad = baseSpec()
    bad.validation.rejection_criteria = ['foo']
    expect(
      evaluateDesignCompleteChecklist(bad, true).find((c) => c.id === 'REJECT_KNOWN')?.ok,
    ).toBe(false)
    bad.validation.baseline_ids = ['magic']
    bad.validation.rejection_criteria = ['capacity_failure']
    expect(
      evaluateDesignCompleteChecklist(bad, true).find((c) => c.id === 'BASELINE_KNOWN')?.ok,
    ).toBe(false)
  })

  it('fails when capability not supported', () => {
    const checks = evaluateDesignCompleteChecklist(baseSpec(), false)
    expect(checks.find((c) => c.id === 'CAPABILITY')?.ok).toBe(false)
  })
})
