import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { WatchItem } from '../api'

const watched: WatchItem = { code: 'sh.600519', name: '贵州茅台', industry: '白酒' }

const stubs = {
  'router-link': { template: '<a><slot /></a>' },
  StockSearchInput: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template: '<input :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />',
  },
}

beforeEach(() => {
  vi.resetModules()
})

describe('watchlist view', () => {
  it('renders watchlist items and removes one', async () => {
    const { api } = await import('../api')
    vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 1, items: [watched] })
    const removeWatch = vi.spyOn(api, 'removeWatch').mockResolvedValue({ code: watched.code, is_watch: false })
    const Component = (await import('./Watchlist.vue')).default

    const wrapper = mount(Component, { global: { stubs } })
    await flushPromises()

    expect(wrapper.text()).toContain('贵州茅台')

    await wrapper.get('button[title^="移出自选"]').trigger('click')
    await flushPromises()

    expect(removeWatch).toHaveBeenCalledWith('sh.600519')
    expect(wrapper.text()).toContain('还没有自选股票')
    expect(wrapper.text()).toContain('已把 贵州茅台 移出自选')
  })

  it('adds the selected stock to the watchlist', async () => {
    const { api } = await import('../api')
    vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 0, items: [] })
    const addWatch = vi.spyOn(api, 'addWatch').mockResolvedValue(watched)
    const Component = (await import('./Watchlist.vue')).default

    const wrapper = mount(Component, { global: { stubs } })
    await flushPromises()

    await wrapper.get('form input').setValue('sh.600519')
    await wrapper.get('form').trigger('submit')
    await flushPromises()

    expect(addWatch).toHaveBeenCalledWith('sh.600519')
    expect(wrapper.text()).toContain('已把 贵州茅台 加入自选')
  })
})
