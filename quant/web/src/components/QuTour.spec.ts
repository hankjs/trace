import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TourStep } from '../tour'

const RECT = { top: 100, left: 100, right: 200, bottom: 140, width: 100, height: 40, x: 100, y: 100 }

const wrappers: VueWrapper[] = []

function addTarget(name: string) {
  const el = document.createElement('button')
  el.setAttribute('data-tour', name)
  el.textContent = `目标 ${name}`
  // jsdom 的 getBoundingClientRect 全为 0,需要自行 stub
  el.getBoundingClientRect = () => ({ ...RECT, toJSON: () => ({}) }) as DOMRect
  document.body.appendChild(el)
  return el
}

async function mountTour(steps: TourStep[], onFinish = vi.fn()) {
  const tour = await import('../tour')
  const Component = (await import('./QuTour.vue')).default
  const wrapper = mount(Component)
  wrappers.push(wrapper)
  tour.startTour(steps, { onFinish })
  await flushPromises()
  return { tour, onFinish }
}

function bubble() {
  return document.querySelector<HTMLElement>('[data-testid="tour-bubble"]')
}

function highlight() {
  return document.querySelector<HTMLElement>('[data-testid="tour-highlight"]')
}

function bubbleButton(text: string) {
  return Array.from(bubble()?.querySelectorAll('button') ?? [])
    .find((button) => button.textContent?.includes(text))
}

beforeEach(() => {
  vi.resetModules()
  document.body.innerHTML = ''
})

afterEach(() => {
  vi.useRealTimers()
  while (wrappers.length) wrappers.pop()?.unmount()
  document.body.innerHTML = ''
})

describe('QuTour', () => {
  it('tour 未激活时不渲染任何内容', async () => {
    const Component = (await import('./QuTour.vue')).default
    wrappers.push(mount(Component))
    await flushPromises()

    expect(bubble()).toBeNull()
    expect(highlight()).toBeNull()
  })

  it('激活后渲染高亮环与气泡,展示标题、内容与步数', async () => {
    addTarget('demo')
    await mountTour([
      { target: 'demo', title: '第一步标题', content: '第一步内容' },
      { target: 'demo', title: '第二步标题', content: '第二步内容' },
    ])

    expect(highlight()).not.toBeNull()
    expect(bubble()?.textContent).toContain('第一步标题')
    expect(bubble()?.textContent).toContain('第一步内容')
    expect(bubble()?.textContent).toContain('第 1/2 步')
    // 第一步不能回退
    expect(bubbleButton('上一步')?.disabled).toBe(true)
  })

  it('下一步推进到最后一步,点完成触发 onFinish(true)', async () => {
    addTarget('demo')
    const { onFinish } = await mountTour([
      { target: 'demo', title: '第一步', content: '内容' },
      { target: 'demo', title: '第二步', content: '内容' },
    ])

    bubbleButton('下一步')?.click()
    await flushPromises()
    expect(bubble()?.textContent).toContain('第 2/2 步')

    bubbleButton('上一步')?.click()
    await flushPromises()
    expect(bubble()?.textContent).toContain('第 1/2 步')

    bubbleButton('下一步')?.click()
    await flushPromises()
    bubbleButton('完成')?.click()
    await flushPromises()

    expect(onFinish).toHaveBeenCalledWith(true)
    expect(bubble()).toBeNull()
    expect(highlight()).toBeNull()
  })

  it('Esc 与跳过按钮都触发 onFinish(false)', async () => {
    addTarget('demo')
    const { onFinish } = await mountTour([{ target: 'demo', title: '唯一一步', content: '内容' }])

    bubble()?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await flushPromises()

    expect(onFinish).toHaveBeenCalledWith(false)
    expect(bubble()).toBeNull()

    const second = vi.fn()
    const tour = await import('../tour')
    tour.startTour([{ target: 'demo', title: '再来', content: '内容' }], { onFinish: second })
    await flushPromises()
    bubbleButton('跳过')?.click()
    await flushPromises()
    expect(second).toHaveBeenCalledWith(false)
    expect(bubble()).toBeNull()
  })

  it('advanceOn: target 时隐藏下一步按钮,点击目标元素范围内才推进', async () => {
    const target = addTarget('demo')
    addTarget('demo2')
    const { tour } = await mountTour([
      { target: 'demo', title: '操作步', content: '内容', advanceOn: 'target' },
      { target: 'demo2', title: '结束步', content: '内容' },
    ])

    expect(bubbleButton('下一步')).toBeUndefined()
    expect(bubble()?.textContent).toContain('点击页面上高亮的元素继续')

    // 范围外的点击不推进
    document.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: 500, clientY: 500 }))
    await flushPromises()
    expect(tour.useTour().index.value).toBe(0)

    // 落在目标 rect(100~200, 100~140)内的点击推进
    target.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: 150, clientY: 120 }))
    await flushPromises()
    expect(tour.useTour().index.value).toBe(1)
    expect(bubble()?.textContent).toContain('结束步')
  })

  it('目标元素不存在时最多等待约 3 秒后自动跳过该步', async () => {
    vi.useFakeTimers()
    addTarget('demo')
    const tour = await import('../tour')
    const Component = (await import('./QuTour.vue')).default
    wrappers.push(mount(Component))
    const onFinish = vi.fn()
    tour.startTour([
      { target: 'missing', title: '找不到的步', content: '内容' },
      { target: 'demo', title: '存在的步', content: '内容' },
    ], { onFinish })
    // advanceTimersByTimeAsync 顺带冲刷微任务(假时钟下 flushPromises 会挂起)
    await vi.advanceTimersByTimeAsync(0)

    // 等待期间不渲染高亮环,仍停留在第一步
    expect(highlight()).toBeNull()
    expect(tour.useTour().index.value).toBe(0)

    await vi.advanceTimersByTimeAsync(3100)

    expect(tour.useTour().index.value).toBe(1)
    expect(bubble()?.textContent).toContain('存在的步')
    expect(highlight()).not.toBeNull()
    expect(onFinish).not.toHaveBeenCalled()
  })

  it('唯一一步等不到目标时,超时结束 tour 并视为看完', async () => {
    vi.useFakeTimers()
    const tour = await import('../tour')
    const Component = (await import('./QuTour.vue')).default
    wrappers.push(mount(Component))
    const onFinish = vi.fn()
    tour.startTour([{ target: 'missing', title: '找不到', content: '内容' }], { onFinish })
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(3100)

    expect(onFinish).toHaveBeenCalledWith(true)
    expect(bubble()).toBeNull()
  })
})
