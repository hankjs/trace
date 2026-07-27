import { describe, expect, it } from 'vitest'
import {
  DEFAULT_RISK_OVERLAY,
  DEFAULT_TAKE_PROFIT,
  overlayParamSnapshot,
  overlaySummary,
} from './researchPlans'

describe('research plan overlay parameters', () => {
  it('does not persist untouched disabled overlays', () => {
    expect(overlayParamSnapshot(
      { ...DEFAULT_RISK_OVERLAY },
      { ...DEFAULT_TAKE_PROFIT }
    )).toEqual({})
  })

  it('persists explicit disable when an existing overlay is turned off', () => {
    const result = overlayParamSnapshot(
      { ...DEFAULT_RISK_OVERLAY },
      { ...DEFAULT_TAKE_PROFIT },
      { risk_overlay: { ...DEFAULT_RISK_OVERLAY, enabled: true } }
    )
    expect(result.risk_overlay).toEqual(DEFAULT_RISK_OVERLAY)
  })

  it('describes daily confirmation and next-day simulated fills', () => {
    const text = overlaySummary({ enabled: true, type: 'fixed_pct', value: 0.08, atr_period: 14 }, 'risk')
    expect(text).toContain('8.00%')
    expect(text).toContain('T 日收盘确认')
    expect(text).toContain('T+1 开盘模拟')
  })
})
