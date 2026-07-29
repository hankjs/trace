import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./tour', () => ({ startTour: vi.fn() }))

const STORAGE_KEY = 'quant_onboarding_v1'

async function loadOnboarding() {
  return await import('./onboarding')
}

beforeEach(() => {
  vi.resetModules()
  localStorage.clear()
})

describe('onboarding store', () => {
  it('localStorage 无记录时进度为空且未交互', async () => {
    const { useOnboarding } = await loadOnboarding()
    const { done, hasInteracted, doneCount, allDone } = useOnboarding()

    expect(done.value).toEqual({})
    expect(hasInteracted.value).toBe(false)
    expect(doneCount.value).toBe(0)
    expect(allDone.value).toBe(false)
  })

  it('防御脏数据:非法 JSON 与错误结构都视为无进度', async () => {
    localStorage.setItem(STORAGE_KEY, '{not json')
    let mod = await loadOnboarding()
    expect(mod.useOnboarding().done.value).toEqual({})

    vi.resetModules()
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ done: 'oops', seen: 'yes' }))
    mod = await loadOnboarding()
    expect(mod.useOnboarding().done.value).toEqual({})
    expect(mod.useOnboarding().hasInteracted.value).toBe(false)

    vi.resetModules()
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { visit_dashboard: '2026-01-01T00:00:00.000Z', unknown_task: 'x', add_watch: 42 },
      seen: true,
    }))
    mod = await loadOnboarding()
    // 只保留已知任务且值为字符串的条目
    expect(mod.useOnboarding().done.value).toEqual({ visit_dashboard: '2026-01-01T00:00:00.000Z' })
    expect(mod.useOnboarding().hasInteracted.value).toBe(true)
  })

  it('completeByRoute 不再完成已升级为 tour 类的任务', async () => {
    const { completeByRoute, useOnboarding } = await loadOnboarding()

    completeByRoute({ name: 'dashboard', query: {} })
    completeByRoute({ name: 'selection', query: { tab: 'picks' } })
    completeByRoute({ name: 'selection', query: { tab: 'screener' } })
    completeByRoute({ name: 'signals', query: {} })

    const { doneCount, isDone } = useOnboarding()
    expect(isDone('visit_dashboard')).toBe(false)
    expect(isDone('view_picks')).toBe(false)
    expect(isDone('run_screener')).toBe(false)
    expect(isDone('view_signals')).toBe(false)
    expect(doneCount.value).toBe(0)
  })

  it('completeByRoute 不影响探测类与事件类任务', async () => {
    const { completeByRoute, useOnboarding } = await loadOnboarding()

    completeByRoute({ name: 'watchlist', query: {} })
    completeByRoute({ name: 'strategies-backtest', query: {} })

    expect(useOnboarding().isDone('add_watch')).toBe(false)
    expect(useOnboarding().isDone('run_backtest')).toBe(false)
  })

  it('startTaskTour 跳转后开播引导,tour 类任务看完或跳过即完成', async () => {
    vi.useFakeTimers()
    const { startTour } = await import('./tour')
    const startTourMock = vi.mocked(startTour)
    const { ONBOARDING_TOURS } = await import('./onboardingTours')
    const { startTaskTour, useOnboarding } = await loadOnboarding()
    const push = vi.fn().mockResolvedValue(undefined)

    const pending = startTaskTour('visit_dashboard', push)
    await vi.runAllTimersAsync()
    await pending

    expect(push).toHaveBeenCalledWith({ name: 'dashboard' })
    expect(startTourMock).toHaveBeenCalledTimes(1)
    const [steps, opts] = startTourMock.mock.calls[0]
    expect(steps).toBe(ONBOARDING_TOURS.visit_dashboard)
    expect(useOnboarding().isDone('visit_dashboard')).toBe(false)
    // 主动跳过(completed=false)同样视为完成
    opts?.onFinish?.(false)
    expect(useOnboarding().isDone('visit_dashboard')).toBe(true)
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')
    expect(typeof stored.done.visit_dashboard).toBe('string')
    vi.useRealTimers()
  })

  it('startTaskTour 对探测类任务开播引导但不直接完成', async () => {
    vi.useFakeTimers()
    const { startTour } = await import('./tour')
    const startTourMock = vi.mocked(startTour)
    const { startTaskTour, useOnboarding } = await loadOnboarding()
    const push = vi.fn().mockResolvedValue(undefined)

    const pending = startTaskTour('add_watch', push)
    await vi.runAllTimersAsync()
    await pending

    expect(push).toHaveBeenCalledWith({ name: 'watchlist' })
    expect(startTourMock).toHaveBeenCalledTimes(1)
    const [, opts] = startTourMock.mock.calls[0]
    // 引导看完不代表真的加过自选,完成仍靠 probe
    opts?.onFinish?.(true)
    expect(useOnboarding().isDone('add_watch')).toBe(false)
    vi.useRealTimers()
  })

  it('refreshProbes 数据非空即完成对应任务', async () => {
    const { api } = await import('./api')
    vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 1, items: [{ code: 'sh.600519', name: '贵州茅台', industry: '白酒' }] })
    vi.spyOn(api, 'strategies').mockResolvedValue({
      count: 2,
      items: [
        { id: 1, is_system: true },
        { id: 2, is_system: false },
      ] as never,
      limits: {} as never,
    })
    vi.spyOn(api, 'trades').mockResolvedValue({ count: 0, items: [] })
    const { refreshProbes, useOnboarding } = await loadOnboarding()

    await refreshProbes()

    const { isDone } = useOnboarding()
    expect(isDone('add_watch')).toBe(true)
    expect(isDone('duplicate_strategy')).toBe(true)
    expect(isDone('add_trade')).toBe(false)
  })

  it('refreshProbes 接口报错时静默跳过', async () => {
    const { api } = await import('./api')
    vi.spyOn(api, 'watchlist').mockRejectedValue(new Error('未登录'))
    vi.spyOn(api, 'strategies').mockRejectedValue(new Error('网络错误'))
    vi.spyOn(api, 'trades').mockRejectedValue(new Error('网络错误'))
    const { refreshProbes, useOnboarding } = await loadOnboarding()

    await expect(refreshProbes()).resolves.toBeUndefined()
    expect(useOnboarding().doneCount.value).toBe(0)
  })

  it('refreshProbes 只探测未完成任务', async () => {
    const { api } = await import('./api')
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { add_watch: '2026-01-01T00:00:00.000Z' },
      seen: true,
    }))
    const watchlist = vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 1, items: [{ code: 'sh.600519', name: '贵州茅台', industry: '白酒' }] })
    vi.spyOn(api, 'strategies').mockResolvedValue({ count: 0, items: [], limits: {} as never })
    vi.spyOn(api, 'trades').mockResolvedValue({ count: 0, items: [] })
    const { refreshProbes } = await loadOnboarding()

    await refreshProbes()

    expect(watchlist).not.toHaveBeenCalled()
  })

  it('markEventDone 只接受事件类任务', async () => {
    const { markEventDone, useOnboarding } = await loadOnboarding()

    markEventDone('run_backtest')
    expect(useOnboarding().isDone('run_backtest')).toBe(true)

    markEventDone('add_watch')
    expect(useOnboarding().isDone('add_watch')).toBe(false)

    markEventDone('not_a_task')
    expect(useOnboarding().doneCount.value).toBe(1)
  })

  it('resetProgress 清空进度但保留交互标记', async () => {
    const { completeByRoute, markInteracted, resetProgress, useOnboarding } = await loadOnboarding()

    completeByRoute({ name: 'dashboard', query: {} })
    markInteracted()
    resetProgress()

    const { done, hasInteracted } = useOnboarding()
    expect(done.value).toEqual({})
    expect(hasInteracted.value).toBe(true)
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')).toEqual({ done: {}, seen: true, hidden: false })
  })

  it('resetTask 只清除单条任务,其它进度保留', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      done: { add_watch: '2026-01-01T00:00:00.000Z', run_backtest: '2026-01-02T00:00:00.000Z' },
      seen: true,
    }))
    const { resetTask, useOnboarding } = await loadOnboarding()

    resetTask('add_watch')

    const { done, isDone, doneCount } = useOnboarding()
    expect(isDone('add_watch')).toBe(false)
    expect(isDone('run_backtest')).toBe(true)
    expect(doneCount.value).toBe(1)
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')
    expect(stored.done).toEqual({ run_backtest: '2026-01-02T00:00:00.000Z' })

    // 重置未完成的任务是无操作
    resetTask('add_trade')
    expect(done.value).toEqual({ run_backtest: '2026-01-02T00:00:00.000Z' })
  })

  it('hideForever 持久化隐藏标记,showGuide 恢复显示', async () => {
    const { hideForever, useOnboarding } = await loadOnboarding()
    const { dismissed, panelOpen } = useOnboarding()

    expect(dismissed.value).toBe(false)
    panelOpen.value = true
    hideForever()

    expect(dismissed.value).toBe(true)
    expect(panelOpen.value).toBe(false)
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}').hidden).toBe(true)

    // 重新加载模块后隐藏标记仍在
    vi.resetModules()
    const reloaded = await loadOnboarding()
    expect(reloaded.useOnboarding().dismissed.value).toBe(true)

    reloaded.showGuide()
    expect(reloaded.useOnboarding().dismissed.value).toBe(false)
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}').hidden).toBe(false)
  })

  it('doneCount 与 allDone 随进度变化', async () => {
    const { ONBOARDING_TASKS, markEventDone, useOnboarding } = await loadOnboarding()
    const { doneCount, allDone } = useOnboarding()

    expect(doneCount.value).toBe(0)
    markEventDone('run_backtest')
    expect(doneCount.value).toBe(1)
    expect(allDone.value).toBe(false)

    for (const task of ONBOARDING_TASKS) {
      if (task.completion.type === 'event') {
        markEventDone(task.id)
      } else {
        // 引导/探测类任务直接写入完成态模拟各自的完成路径
        useOnboarding().done.value = { ...useOnboarding().done.value, [task.id]: '2026-01-01T00:00:00.000Z' }
      }
    }
    expect(doneCount.value).toBe(ONBOARDING_TASKS.length)
    expect(allDone.value).toBe(true)
  })
})
