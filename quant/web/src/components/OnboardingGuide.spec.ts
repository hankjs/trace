import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { Router } from 'vue-router'

vi.mock('../tour', () => ({ startTour: vi.fn() }))

const STORAGE_KEY = 'quant_onboarding_v1'

function makeRouter(): Router {
  const page = { template: '<div />' }
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', name: 'dashboard', component: page },
      { path: '/watchlist', name: 'watchlist', component: page },
      { path: '/selection', name: 'selection', component: page },
      { path: '/signals', name: 'signals', component: page },
      { path: '/strategies/manage', name: 'strategies-manage', component: page },
      { path: '/strategies/backtest', name: 'strategies-backtest', component: page },
      { path: '/portfolio', name: 'portfolio', component: page },
      { path: '/catalog', name: 'catalog', component: page },
    ],
  })
}

beforeEach(() => {
  vi.resetModules()
  localStorage.clear()
  document.body.innerHTML = ''
})

async function mountGuide() {
  const { api } = await import('../api')
  vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 0, items: [] })
  vi.spyOn(api, 'strategies').mockResolvedValue({ count: 0, items: [], limits: {} as never })
  vi.spyOn(api, 'trades').mockResolvedValue({ count: 0, items: [] })
  const router = makeRouter()
  await router.push('/')
  await router.isReady()
  const Component = (await import('./OnboardingGuide.vue')).default
  const wrapper = mount(Component, { global: { plugins: [router] } })
  await flushPromises()
  return { wrapper, router }
}

function panel() {
  return document.querySelector<HTMLElement>('#onboarding-guide')
}

function taskItem(title: string) {
  return Array.from(panel()?.querySelectorAll<HTMLElement>('ol li') ?? [])
    .find((item) => item.textContent?.includes(title))
}

function itemButton(item: Element | undefined, text: string) {
  return Array.from(item?.querySelectorAll('button') ?? [])
    .find((button) => button.textContent?.includes(text))
}

function goButtons() {
  return Array.from(panel()?.querySelectorAll('ol li button') ?? [])
    .filter((button) => button.textContent?.includes('前往'))
}

describe('OnboardingGuide', () => {
  it('首次访问自动展开面板并渲染 8 项任务', async () => {
    await mountGuide()

    const el = panel()
    expect(el).not.toBeNull()
    expect(el?.textContent).toContain('新手上路')
    expect(el?.querySelectorAll('ol li').length).toBe(8)
    expect(el?.textContent).toContain('打开「今日研究」确认数据日期')
    // tour 类任务不再因到过页面而自动完成,8 项全部显示「前往」
    expect(goButtons().length).toBe(8)
  })

  it('已交互过的用户不自动展开,点击浮动按钮打开', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ done: {}, seen: true }))
    const { wrapper } = await mountGuide()

    expect(panel()).toBeNull()

    await wrapper.get('button[title="新手上路"]').trigger('click')
    await flushPromises()

    expect(panel()).not.toBeNull()
  })

  it('点击「前往」跳转目标页、关闭面板并自动开播该任务的引导', async () => {
    const { startTour } = await import('../tour')
    const startTourMock = vi.mocked(startTour)
    const { ONBOARDING_TOURS } = await import('../onboardingTours')
    const { router } = await mountGuide()
    const push = vi.spyOn(router, 'push')

    itemButton(taskItem('添加第一只自选股'), '前往')?.click()
    await flushPromises()

    expect(push).toHaveBeenCalledWith({ name: 'watchlist' })
    expect(panel()).toBeNull()
    // startTaskTour 在跳转后延迟开播(等页面首屏)
    await vi.waitFor(() => expect(startTourMock).toHaveBeenCalledTimes(1))
    expect(startTourMock.mock.calls[0][0]).toBe(ONBOARDING_TOURS.add_watch)
  })

  it('已完成项渲染勾选与「重看」「重置」按钮,重看同样开播引导', async () => {
    const { startTour } = await import('../tour')
    const startTourMock = vi.mocked(startTour)
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { add_watch: '2026-01-15T08:00:00.000Z' },
      seen: false,
    }))
    const { router } = await mountGuide()
    const push = vi.spyOn(router, 'push')

    const watchItem = taskItem('添加第一只自选股')
    expect(watchItem).toBeDefined()
    expect(watchItem?.querySelector('svg')).not.toBeNull()
    // 不再显示完成日期
    expect(watchItem?.textContent).not.toMatch(/\d{2}\/\d{2}/)
    expect(itemButton(watchItem, '前往')).toBeUndefined()
    expect(itemButton(watchItem, '重置')).toBeDefined()
    // 8 项中仅 add_watch 已完成,剩 7 个「前往」
    expect(goButtons().length).toBe(7)

    itemButton(watchItem, '重看')?.click()
    await flushPromises()
    expect(push).toHaveBeenCalledWith({ name: 'watchlist' })
    await vi.waitFor(() => expect(startTourMock).toHaveBeenCalledTimes(1))
  })

  it('单条任务的「重置」只清除该任务并恢复「前往」', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { add_watch: '2026-01-15T08:00:00.000Z', run_backtest: '2026-01-16T08:00:00.000Z' },
      seen: false,
    }))
    await mountGuide()

    itemButton(taskItem('添加第一只自选股'), '重置')?.click()
    await flushPromises()

    // add_watch 回到未完成,run_backtest 不受影响
    expect(itemButton(taskItem('添加第一只自选股'), '前往')).toBeDefined()
    expect(itemButton(taskItem('跑一次历史回测'), '前往')).toBeUndefined()
    expect(goButtons().length).toBe(7)
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')
    expect(stored.done).toEqual({ run_backtest: '2026-01-16T08:00:00.000Z' })
  })

  it('重置进度清空已完成任务', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { add_watch: '2026-01-15T08:00:00.000Z' },
      seen: true,
    }))
    const { wrapper } = await mountGuide()
    await wrapper.get('button[title="新手上路"]').trigger('click')
    await flushPromises()

    const resetButton = Array.from(panel()?.querySelectorAll('button') ?? [])
      .find((button) => button.textContent?.includes('重置进度'))
    resetButton?.click()
    await flushPromises()

    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')
    expect(stored.done).toEqual({})
  })

  it('「永远隐藏」关闭面板与浮动按钮并持久化,挂载时 hidden=true 则不渲染入口', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ done: {}, seen: false }))
    const { wrapper } = await mountGuide()

    const hideButton = Array.from(panel()?.querySelectorAll('button') ?? [])
      .find((button) => button.textContent?.includes('永远隐藏'))
    hideButton?.click()
    await flushPromises()

    expect(panel()).toBeNull()
    expect(wrapper.find('button[title="新手上路"]').exists()).toBe(false)
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}').hidden).toBe(true)

    // 已隐藏的浏览器重新挂载:不渲染触发按钮,也不自动展开面板
    vi.resetModules()
    document.body.innerHTML = ''
    const remounted = await mountGuide()
    expect(remounted.wrapper.find('button[title="新手上路"]').exists()).toBe(false)
    expect(panel()).toBeNull()
  })
})
