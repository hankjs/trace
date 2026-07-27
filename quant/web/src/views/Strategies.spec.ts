import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CatalogEntry, Strategy, StrategyLimits } from '../api'

const limits: StrategyLimits = { max_total: 50, max_enabled: 10 }

const booleanTemplate: CatalogEntry = {
  key: 'test_boolean',
  name: '布尔参数测试模板',
  description: '用于锁定不同参数类型的表单行为。',
  kind: 'single',
  kind_name: '单只股票',
  params: [
    {
      key: 'use_filter',
      name: '启用过滤',
      value_type: 'boolean',
      default: false,
    },
  ],
}

function editableStrategy(): Strategy {
  return {
    id: 10,
    name: '我的布尔策略',
    template: booleanTemplate.key,
    template_name: booleanTemplate.name,
    kind: 'single',
    kind_name: '单只股票',
    params: { use_filter: true },
    effective_params: { use_filter: true },
    params_valid: true,
    enabled: true,
    is_system: false,
    editable: true,
    backtest_count: 0,
  }
}

beforeEach(() => {
  vi.resetModules()
})

describe('strategy parameter form', () => {
  it('renders boolean parameters and stores only overrides from template defaults', async () => {
    const strategy = editableStrategy()
    const { api } = await import('../api')
    vi.spyOn(api, 'strategies').mockResolvedValue({ items: [strategy], limits })
    vi.spyOn(api, 'strategyTemplates').mockResolvedValue({ items: [booleanTemplate] })
    const update = vi.spyOn(api, 'updateStrategy').mockResolvedValue(strategy)
    const Component = (await import('./Strategies.vue')).default

    const wrapper = mount(Component)
    await flushPromises()

    const checkbox = wrapper.get<HTMLInputElement>('input[type="checkbox"]')
    expect(checkbox.element.checked).toBe(true)

    await checkbox.setValue(false)
    const save = wrapper.findAll('button').find((button) => button.text() === '保存参数')
    expect(save).toBeDefined()
    await save!.trigger('click')
    await flushPromises()

    expect(update).toHaveBeenCalledWith(strategy.id, { params: {} })
  })

  it('separates overlay metadata from native parameter fields', async () => {
    const template: CatalogEntry = {
      key: 'breakout',
      name: '价格突破策略',
      description: '测试模板参数分区。',
      kind: 'single',
      kind_name: '单只股票',
      params: [
        { key: 'entry', name: '入场观察天数', value_type: 'integer', default: 20 },
        {
          key: 'risk_overlay',
          name: '统一风险覆盖层',
          value_type: 'overlay',
          default: { enabled: false, type: 'fixed_pct', value: 0.08, atr_period: 14 },
        },
        {
          key: 'take_profit',
          name: '可选止盈覆盖层',
          value_type: 'overlay',
          default: { enabled: false, type: 'fixed_pct', value: 0.2, atr_period: 14 },
        },
      ],
    }
    const strategy: Strategy = {
      ...editableStrategy(),
      template: template.key,
      template_name: template.name,
      params: {},
      effective_params: {
        entry: 20,
        risk_overlay: { enabled: false, type: 'fixed_pct', value: 0.08, atr_period: 14 },
        take_profit: { enabled: false, type: 'fixed_pct', value: 0.2, atr_period: 14 },
      },
    }
    const { api } = await import('../api')
    vi.spyOn(api, 'strategies').mockResolvedValue({ items: [strategy], limits })
    vi.spyOn(api, 'strategyTemplates').mockResolvedValue({ items: [template] })
    const Component = (await import('./Strategies.vue')).default

    const wrapper = mount(Component)
    await flushPromises()

    expect(wrapper.text()).toContain('模板原生参数')
    expect(wrapper.text()).toContain('统一风险覆盖层')
    expect(wrapper.text()).toContain('止盈覆盖层')
    expect(wrapper.findAll('input[type="number"]')).toHaveLength(1)
    expect(wrapper.text()).not.toContain('[object Object]')

    const takeProfitSwitch = wrapper.findAll<HTMLInputElement>('input[type="checkbox"]')[1]
    await takeProfitSwitch.setValue(true)
    expect(wrapper.findAll('input[type="number"]')).toHaveLength(2)
  })
})
