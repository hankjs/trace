import { describe, expect, it } from 'vitest'
import {
  SPEC_SNIPPETS,
  buildSnippetAst,
  mergeSuggestedFields,
  orderFastSlow,
  snippetsForTarget,
} from './specSnippets'

describe('specSnippets', () => {
  it('ships at least 8 snippets covering entry/exit/score', () => {
    expect(SPEC_SNIPPETS.length).toBeGreaterThanOrEqual(8)
    const targets = new Set(SPEC_SNIPPETS.flatMap((s) => s.targets))
    expect(targets.has('entry')).toBe(true)
    expect(targets.has('exit')).toBe(true)
    expect(targets.has('score')).toBe(true)
    for (const s of SPEC_SNIPPETS) {
      expect(s.disclaimer).toMatch(/未验证/)
      expect(s.id).toMatch(/^[a-z][a-z0-9_]*$/)
    }
  })

  it('clamps params to min/max and rounds ints', () => {
    const snip = SPEC_SNIPPETS.find((s) => s.id === 'entry_breakout_n')!
    const high = snip.build({ N: 9999 })
    expect((high as { right: { window: number } }).right.window).toBe(500)
    const low = snip.build({ N: 1 })
    expect((low as { right: { window: number } }).right.window).toBe(2)
  })

  it('orders fast/slow so fast < slow', () => {
    expect(orderFastSlow(60, 10)).toEqual({ fast: 10, slow: 60 })
    expect(orderFastSlow(10, 60)).toEqual({ fast: 10, slow: 60 })
    const ast = buildSnippetAst('entry_ma_cross_up', { fast: 60, slow: 10 }) as {
      left: { window: number }
      right: { window: number }
    }
    expect(ast.left.window).toBe(10)
    expect(ast.right.window).toBe(60)
  })

  it('filters by target slot', () => {
    const exits = snippetsForTarget('exit', 'single')
    expect(exits.every((s) => s.targets.includes('exit'))).toBe(true)
    expect(exits.some((s) => s.id === 'exit_channel_low')).toBe(true)
  })

  it('merges suggested fields without dropping existing', () => {
    const merged = mergeSuggestedFields(
      [{ field: 'close', availability: 'daily_close', required: false }],
      ['close', 'high'],
    )
    expect(merged.find((f) => f.field === 'close')?.required).toBe(true)
    expect(merged.some((f) => f.field === 'high')).toBe(true)
  })

  it('builds non-empty AST for every first-batch snippet', () => {
    for (const s of SPEC_SNIPPETS) {
      const ast = s.build({})
      expect(ast.op).toBeTruthy()
    }
  })
})
