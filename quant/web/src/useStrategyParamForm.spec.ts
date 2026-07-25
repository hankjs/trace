import { describe, expect, it } from 'vitest'
import type { CatalogParameter } from './api'
import { useStrategyParamForm } from './useStrategyParamForm'

const parameters: CatalogParameter[] = [
  { key: 'period', name: '周期', value_type: 'integer', default: 20, minimum: 2 },
  { key: 'enabled', name: '启用过滤', value_type: 'boolean', default: false },
  { key: 'label', name: '标签', value_type: 'string', default: '默认' },
]

describe('useStrategyParamForm', () => {
  it('fills defaults and returns only explicit overrides', () => {
    const form = useStrategyParamForm(parameters)
    form.reset({ enabled: true })

    expect(form.snapshot()).toEqual({ period: 20, enabled: true, label: '默认' })
    expect(form.overrides.value).toEqual({ enabled: true })
    expect(form.differsFrom({ period: 20, enabled: false, label: '默认' })).toBe(true)
  })

  it('validates integer bounds and required strings', () => {
    const form = useStrategyParamForm(parameters)
    form.reset()
    form.values.period = 1.5
    form.values.label = ''

    expect(form.validate()).toBe(false)
    expect(form.errors).toEqual({
      period: '周期必须为整数',
      label: '请填写标签',
    })
  })
})
