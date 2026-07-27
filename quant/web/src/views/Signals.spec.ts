import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SignalItem } from '../api'

beforeEach(() => {
  vi.resetModules()
})

describe('Signals research plan disclosure', () => {
  it('uses a focusable button and expands the plan inline', async () => {
    const signal: SignalItem = {
      id: 1,
      code: 'sh.600519',
      name: '贵州茅台',
      date: '2026-07-24',
      strategy_id: 2,
      strategy_name: '突破研究',
      side: 'buy',
      price: 1500,
      reason: {},
      reason_text: '收盘后满足突破条件。',
      plan_summary: {
        id: 7,
        type: 'single',
        status: 'active',
        data_date: '2026-07-24',
        signal_close_price: 1500,
      },
    }
    const { api } = await import('../api')
    vi.spyOn(api, 'catalog').mockResolvedValue({})
    vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 0, items: [] })
    vi.spyOn(api, 'signals').mockResolvedValue({ count: 1, items: [signal] })
    const Component = (await import('./Signals.vue')).default
    const wrapper = mount(Component, {
      global: {
        stubs: {
          RouterLink: { template: '<a><slot /></a>' },
          StrategySelect: { template: '<label>策略<select /></label>' },
          StockSearchInput: { template: '<label>股票<input /></label>' },
        },
      },
    })
    await flushPromises()

    const toggle = wrapper.get<HTMLButtonElement>('button[aria-controls="signal-plan-1"]')
    expect(toggle.attributes('aria-expanded')).toBe('false')
    await toggle.trigger('click')
    expect(toggle.attributes('aria-expanded')).toBe('true')
    expect(wrapper.get('#signal-plan-1').text()).toContain('数据与信号')
    expect(wrapper.get('#signal-plan-1').text()).toContain('产品边界')
  })
})
