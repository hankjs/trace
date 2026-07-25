import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Pool, Strategy, StrategyLimits } from './api'

const limits: StrategyLimits = { max_total: 50, max_enabled: 10 }

function makeStrategy(
  id: number,
  isSystem: boolean,
  kind: Strategy['kind'] = 'single'
): Strategy {
  return {
    id,
    name: `策略 ${id}`,
    template: kind === 'single' ? 'ma_cross' : 'momentum_rotation',
    template_name: kind === 'single' ? '双均线趋势策略' : '强势股票轮动策略',
    kind,
    kind_name: kind === 'single' ? '单只股票' : '股票组合',
    params: {},
    effective_params: {},
    params_valid: true,
    enabled: true,
    is_system: isSystem,
    editable: !isSystem,
  }
}

function makePool(id: number, kind: Pool['kind']): Pool {
  return {
    id,
    kind,
    name: `股票池 ${id}`,
    min_list_days: kind === 'all' ? 60 : 0,
    is_system: kind !== 'static',
  }
}

beforeEach(() => {
  vi.resetModules()
})

describe('strategy collection store', () => {
  it('merges concurrent loads and reuses the loaded cache', async () => {
    const { api } = await import('./api')
    let resolveRequest!: (value: { items: Strategy[]; limits: StrategyLimits }) => void
    const response = new Promise<{ items: Strategy[]; limits: StrategyLimits }>((resolve) => {
      resolveRequest = resolve
    })
    const request = vi.spyOn(api, 'strategies').mockReturnValue(response)
    const store = await import('./strategies')
    const items = [makeStrategy(1, true)]

    const first = store.loadStrategies()
    const second = store.loadStrategies()
    expect(request).toHaveBeenCalledTimes(1)

    resolveRequest({ items, limits })
    await expect(first).resolves.toEqual(items)
    await expect(second).resolves.toEqual(items)
    await expect(store.loadStrategies()).resolves.toEqual(items)
    expect(request).toHaveBeenCalledTimes(1)
  })

  it('reloads after invalidation and keeps the public strategy as default', async () => {
    const { api } = await import('./api')
    const firstItems = [makeStrategy(1, false), makeStrategy(5, true)]
    const nextItems = [makeStrategy(9, false), makeStrategy(3, true, 'portfolio')]
    const request = vi.spyOn(api, 'strategies')
      .mockResolvedValueOnce({ items: firstItems, limits })
      .mockResolvedValueOnce({ items: nextItems, limits })
    const store = await import('./strategies')

    await store.loadStrategies()
    expect(store.defaultStrategy(undefined, firstItems)?.id).toBe(5)
    store.invalidateStrategies()
    await expect(store.loadStrategies()).resolves.toEqual(nextItems)

    expect(request).toHaveBeenCalledTimes(2)
    expect(store.defaultStrategyId('portfolio')).toBe(3)
  })
})

describe('pool collection store', () => {
  it('merges concurrent loads and reuses the loaded cache', async () => {
    const { api } = await import('./api')
    let resolveRequest!: (value: { items: Pool[] }) => void
    const response = new Promise<{ items: Pool[] }>((resolve) => {
      resolveRequest = resolve
    })
    const request = vi.spyOn(api, 'pools').mockReturnValue(response)
    const store = await import('./pools')
    const items = [makePool(2, 'all')]

    const first = store.loadPools()
    const second = store.loadPools()
    expect(request).toHaveBeenCalledTimes(1)

    resolveRequest({ items })
    await expect(first).resolves.toEqual(items)
    await expect(second).resolves.toEqual(items)
    await expect(store.loadPools()).resolves.toEqual(items)
    expect(request).toHaveBeenCalledTimes(1)
  })

  it('reloads after invalidation and prefers the all-market pool', async () => {
    const { api } = await import('./api')
    const firstItems = [makePool(1, 'index'), makePool(2, 'all')]
    const nextItems = [makePool(8, 'static'), makePool(7, 'all')]
    const request = vi.spyOn(api, 'pools')
      .mockResolvedValueOnce({ items: firstItems })
      .mockResolvedValueOnce({ items: nextItems })
    const store = await import('./pools')

    await store.loadPools()
    expect(store.defaultPoolId(firstItems)).toBe(2)
    store.invalidatePools()
    await expect(store.loadPools()).resolves.toEqual(nextItems)

    expect(request).toHaveBeenCalledTimes(2)
    expect(store.defaultPoolId()).toBe(7)
  })
})
