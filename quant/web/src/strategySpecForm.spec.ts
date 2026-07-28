import { describe, expect, it } from 'vitest'
import { buildStrategySpec, defaultStrategySpecForm, strategySpecToForm } from './strategySpecForm'

describe('strategySpecForm', () => {
  it('round-trips the supported breakout and native exit fields', () => {
    const form = defaultStrategySpecForm()
    form.breakoutWindow = 55
    form.volumeWindow = 13
    form.volumeRatio = 2.2
    form.exitWindow = 7
    form.riskEnabled = true
    form.riskType = 'atr_multiple'
    form.riskValue = 2.5

    const restored = strategySpecToForm(buildStrategySpec(form))

    expect(restored).toMatchObject({
      breakoutWindow: 55,
      volumeWindow: 13,
      volumeRatio: 2.2,
      exitWindow: 7,
      riskEnabled: true,
      riskType: 'atr_multiple',
      riskValue: 2.5,
    })
  })

  it('uses controlled AST nodes and next-open execution', () => {
    const spec = buildStrategySpec(defaultStrategySpecForm())

    expect((spec.entry.condition as { args: unknown[] }).args).toHaveLength(2)
    expect((spec.native_exit.condition as { args: unknown[] }).args).toHaveLength(1)
    expect(spec.execution).toMatchObject({
      signal_time: 'close',
      execution_time: 'next_open',
      cost_model: 'a_share_daily_v1',
    })
    const operators = JSON.stringify(spec).match(/\"op\":\"([^\"]+)\"/g) ?? []
    expect(operators.join(' ')).not.toMatch(/eval|python|javascript|shell/i)
  })
})
