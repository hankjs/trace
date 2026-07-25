import { describe, expect, it } from 'vitest'
import { aggregateMetric } from './format'

describe('aggregateMetric', () => {
  it('uses mean, raw, then median and ignores invalid numbers', () => {
    expect(aggregateMetric({ return_mean: 1, return: 2, return_median: 3 }, 'return')).toBe(1)
    expect(aggregateMetric({ return: 2, return_median: 3 }, 'return')).toBe(2)
    expect(aggregateMetric({ return_mean: Number.NaN, return_median: 3 }, 'return')).toBe(3)
    expect(aggregateMetric({}, 'return')).toBeUndefined()
  })
})
