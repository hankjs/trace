import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const STOCKS = [
  { code: 'sh.600519', name: '贵州茅台', industry: '白酒', is_watch: false },
  { code: 'sz.000001', name: '平安银行', industry: '银行', is_watch: false },
  { code: 'sh.600036', name: '招商银行', industry: '银行', is_watch: true },
]

async function mountPicker(props: { modelValue: string[]; multiple?: boolean }) {
  const { api } = await import('../api')
  vi.spyOn(api, 'stockList').mockResolvedValue({ count: STOCKS.length, items: STOCKS })
  const StockPicker = (await import('./StockPicker.vue')).default
  const wrapper = mount(StockPicker, { props })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.resetModules()
})

describe('StockPicker', () => {
  it('multi-select toggles rows and shows selected chips', async () => {
    const wrapper = await mountPicker({ modelValue: [] })
    await wrapper.get('button[aria-haspopup="listbox"]').trigger('click')
    const rows = wrapper.findAll('button[role="option"]')
    expect(rows.length).toBe(3)

    await rows[0].trigger('click')
    expect(wrapper.emitted('update:modelValue')![0]).toEqual([['sh.600519']])

    // 已选行再次点击 -> 移除;已选区同步展示
    await wrapper.setProps({ modelValue: ['sh.600519'] })
    expect(wrapper.text()).toContain('贵州茅台 · sh.600519')
    expect(rows[0].attributes('aria-selected')).toBe('true')
    await rows[0].trigger('click')
    expect(wrapper.emitted('update:modelValue')![1]).toEqual([[]])
  })

  it('filters rows by name or code', async () => {
    const wrapper = await mountPicker({ modelValue: [] })
    await wrapper.get('button[aria-haspopup="listbox"]').trigger('click')
    await wrapper.get('input[placeholder="输入名称或代码过滤"]').setValue('银行')
    const rows = wrapper.findAll('button[role="option"]')
    expect(rows.length).toBe(2)
    expect(wrapper.text()).toContain('匹配 2 只')
  })

  it('single-select picks one and closes the panel', async () => {
    const wrapper = await mountPicker({ modelValue: [], multiple: false })
    await wrapper.get('button[aria-haspopup="listbox"]').trigger('click')
    const row = wrapper
      .findAll('button[role="option"]')
      .find((b) => b.text().includes('平安银行'))!
    await row.trigger('click')
    expect(wrapper.emitted('update:modelValue')![0]).toEqual([['sz.000001']])
    expect(wrapper.find('button[role="option"]').exists()).toBe(false)
  })
})
