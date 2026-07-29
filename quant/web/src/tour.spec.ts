import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { TourStep } from './tour'

const STEPS: TourStep[] = [
  { target: 'a', title: '第一步', content: '内容一' },
  { target: 'b', title: '第二步', content: '内容二' },
]

async function loadTour() {
  return await import('./tour')
}

beforeEach(() => {
  vi.resetModules()
})

describe('tour store', () => {
  it('startTour 空数组直接忽略', async () => {
    const { startTour, useTour } = await loadTour()
    const onFinish = vi.fn()

    startTour([], { onFinish })

    expect(useTour().active.value).toBe(false)
    expect(onFinish).not.toHaveBeenCalled()
  })

  it('startTour 激活并定位到第一步', async () => {
    const { startTour, useTour } = await loadTour()

    startTour(STEPS)

    const { active, index, total, currentStep } = useTour()
    expect(active.value).toBe(true)
    expect(index.value).toBe(0)
    expect(total.value).toBe(2)
    expect(currentStep.value?.target).toBe('a')
  })

  it('next 推进,走完最后一步触发 onFinish(true)', async () => {
    const { next, startTour, useTour } = await loadTour()
    const onFinish = vi.fn()
    startTour(STEPS, { onFinish })

    next()
    expect(useTour().index.value).toBe(1)
    expect(onFinish).not.toHaveBeenCalled()

    next()
    expect(onFinish).toHaveBeenCalledWith(true)
    expect(useTour().active.value).toBe(false)
    expect(useTour().currentStep.value).toBeUndefined()
  })

  it('prev 回退但不能越过第一步', async () => {
    const { next, prev, startTour, useTour } = await loadTour()
    startTour(STEPS)

    prev()
    expect(useTour().index.value).toBe(0)

    next()
    prev()
    expect(useTour().index.value).toBe(0)
  })

  it('skip 触发 onFinish(false)', async () => {
    const { skip, startTour, useTour } = await loadTour()
    const onFinish = vi.fn()
    startTour(STEPS, { onFinish })

    skip()

    expect(onFinish).toHaveBeenCalledWith(false)
    expect(useTour().active.value).toBe(false)
  })

  it('结束后 next/prev/skip 不再生效', async () => {
    const { next, prev, skip, startTour, useTour } = await loadTour()
    const onFinish = vi.fn()
    startTour(STEPS, { onFinish })
    skip()
    onFinish.mockClear()

    next()
    prev()
    skip()

    expect(onFinish).not.toHaveBeenCalled()
    expect(useTour().active.value).toBe(false)
  })

  it('重复 startTour 静默终止旧 tour(不触发其回调)并开始新 tour', async () => {
    const { startTour, useTour } = await loadTour()
    const oldFinish = vi.fn()
    const newFinish = vi.fn()
    startTour(STEPS, { onFinish: oldFinish })

    startTour([{ target: 'c', title: '新引导', content: '内容' }], { onFinish: newFinish })

    expect(oldFinish).not.toHaveBeenCalled()
    const { active, index, total, currentStep } = useTour()
    expect(active.value).toBe(true)
    expect(index.value).toBe(0)
    expect(total.value).toBe(1)
    expect(currentStep.value?.target).toBe('c')
  })
})
