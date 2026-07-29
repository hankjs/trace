import { describe, expect, it } from 'vitest'
import { aggregateMetric, fmtCoverage, fmtPct } from './format'

describe('aggregateMetric', () => {
  it('uses mean, raw, then median and ignores invalid numbers', () => {
    expect(aggregateMetric({ return_mean: 1, return: 2, return_median: 3 }, 'return')).toBe(1)
    expect(aggregateMetric({ return: 2, return_median: 3 }, 'return')).toBe(2)
    expect(aggregateMetric({ return_mean: Number.NaN, return_median: 3 }, 'return')).toBe(3)
    expect(aggregateMetric({}, 'return')).toBeUndefined()
  })
})

describe('fmtCoverage vs fmtPct', () => {
  it('fmtPct is signed (returns / chg); fmtCoverage is unsigned ratio', () => {
    expect(fmtPct(0.1105)).toBe('+11.05%')
    expect(fmtCoverage(0.1105)).toBe('11.1%')
    expect(fmtCoverage(1.0002)).toBe('100.0%')
    expect(fmtCoverage(0)).toBe('0.0%')
    expect(fmtCoverage(undefined)).toBe('—')
  })
})
