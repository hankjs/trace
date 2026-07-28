import { describe, expect, it } from 'vitest'
import {
  expandParamColumns,
  formatParamPatch,
  pickBestTrial,
  sortTrials,
  summarizeTrials,
  type CompareTrial,
} from './experimentCompare'

function trial(
  index: number,
  outcome: string,
  metrics: Record<string, number | null>,
  patch: Record<string, unknown> = {},
): CompareTrial {
  return {
    id: index,
    trial_index: index,
    outcome,
    param_patch: patch,
    metrics_summary: metrics,
  }
}

describe('experimentCompare', () => {
  const rows: CompareTrial[] = [
    trial(1, 'ok', { sharpe: 0.5, annual_return: 0.1, total_return: 0.2, max_drawdown: -0.2 }, { '$.w': 10 }),
    trial(2, 'ok', { sharpe: 1.2, annual_return: 0.3, total_return: 0.5, max_drawdown: -0.15 }, { '$.w': 20 }),
    trial(3, 'error', { sharpe: null, annual_return: null, total_return: null, max_drawdown: null }, { '$.w': 30 }),
    trial(4, 'ok', { sharpe: 0.9, annual_return: 0.2, total_return: 0.4, max_drawdown: -0.1 }, { '$.w': 40 }),
    trial(5, 'no_trades', { sharpe: 0, annual_return: 0, total_return: 0, max_drawdown: 0 }, { '$.w': 50 }),
  ]

  it('sorts by sharpe descending with nulls at bottom', () => {
    const sorted = sortTrials(rows, { key: 'sharpe', dir: 'desc' })
    expect(sorted.map((t) => t.trial_index)).toEqual([2, 4, 1, 5, 3])
  })

  it('picks best ok trial by objective and excludes non-ok', () => {
    const best = pickBestTrial(rows, 'sharpe')
    expect(best?.trial_index).toBe(2)
    const byAnn = pickBestTrial(rows, 'annual_return')
    expect(byAnn?.trial_index).toBe(2)
    // error row with fake high sharpe still excluded
    const withFake = [
      ...rows,
      trial(9, 'error', { sharpe: 99 }, { '$.w': 99 }),
    ]
    expect(pickBestTrial(withFake, 'sharpe')?.trial_index).toBe(2)
  })

  it('expands 1–2 param keys into columns', () => {
    const one = expandParamColumns(rows)
    expect(one.mode).toBe('columns')
    expect(one.keys).toEqual(['$.w'])

    const multi = expandParamColumns([
      trial(1, 'ok', {}, { a: 1, b: 2, c: 3 }),
      trial(2, 'ok', {}, { a: 1 }),
    ])
    expect(multi.mode).toBe('summary')
  })

  it('summary card matches table counts and best', () => {
    const card = summarizeTrials(rows, 'sharpe')
    expect(card.total).toBe(5)
    expect(card.ok).toBe(3)
    expect(card.error).toBe(1)
    expect(card.no_trades).toBe(1)
    expect(card.best_trial_index).toBe(2)
    expect(card.best_value).toBe(1.2)
    expect(card.min).toBe(0.5)
    expect(card.max).toBe(1.2)
    expect(card.median).toBe(0.9)
  })

  it('formats param patch for humans', () => {
    expect(formatParamPatch({})).toBe('(基准)')
    expect(formatParamPatch({ '$.x': 1 })).toContain('$.x=1')
  })
})
