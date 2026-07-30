/**
 * 策略共享状态:策略列表在回测、信号、策略管理等页面复用,集中缓存一次。
 *
 * 与 pools.ts 同一套模式(缓存 + 并发去重 + invalidate)。默认选中优先公共策略
 * 里 id 最小的那条 —— 公共策略是种子数据,按 id 升序即算法内置顺序,对所有用户
 * 一致;落到用户自建策略上会让不同账号看到不同默认值。
 */

import { computed, readonly, shallowRef } from 'vue'
import { api, type Strategy, type StrategyKind, type StrategyLimits } from './api'
import { createCachedCollectionStore } from './createCachedCollectionStore'

// 策略相关判定与类型统一从本模块出口,页面不必再回到 api.ts 取
export { isPresetStrategy } from './api'
export type { Strategy, StrategyKind, StrategyLimits, StrategyParamValue } from './api'

/** 后端未下发 limits 时的兜底,只用于渲染配额文案 */
const DEFAULT_LIMITS: StrategyLimits = { max_total: 0, max_enabled: 0 }

const limits = shallowRef<StrategyLimits>(DEFAULT_LIMITS)
const collection = createCachedCollectionStore({
  request: () => api.strategies(),
  itemsFrom: (response) => response.items ?? [],
  onLoaded: (response) => {
    limits.value = response.limits ?? DEFAULT_LIMITS
  },
})
const strategies = collection.items

/** 默认策略:优先公共策略中 id 最小的,否则退回第一个可用策略 */
export function defaultStrategy(
  kind?: StrategyKind,
  items: readonly Strategy[] = strategies.value
): Strategy | null {
  const candidates = kind ? items.filter((strategy) => strategy.kind === kind) : items
  const preset = candidates
    .filter((strategy) => strategy.is_system)
    .reduce<Strategy | null>((best, strategy) => (!best || strategy.id < best.id ? strategy : best), null)
  return preset ?? candidates[0] ?? null
}

export function defaultStrategyId(
  kind?: StrategyKind,
  items: readonly Strategy[] = strategies.value
): number | null {
  return defaultStrategy(kind, items)?.id ?? null
}

/** 拉取策略列表;并发调用共享同一请求,已加载时直接返回缓存 */
export const loadStrategies = collection.load

/** 策略增删改后调用,强制下次读取走网络 */
export const invalidateStrategies = collection.invalidate

/** 登出或需要完全清空缓存时调用 */
export function resetStrategies() {
  limits.value = DEFAULT_LIMITS
  collection.reset()
}

export const strategyById = collection.byId

/** 已保存的 strategy_id 是否仍然有效(策略可能已被删除) */
export const isKnownStrategyId = collection.isKnownId

export function useStrategies() {
  return {
    strategies,
    limits: readonly(limits),
    loading: collection.loading,
    loaded: collection.loaded,
    error: collection.error,
    customStrategies: computed(() => strategies.value.filter((strategy) => !strategy.is_system)),
    presetStrategies: computed(() => strategies.value.filter((strategy) => strategy.is_system)),
    enabledCount: computed(() =>
      strategies.value.filter((strategy) => !strategy.is_system && strategy.enabled).length
    ),
    load: loadStrategies,
    invalidate: invalidateStrategies,
    reset: resetStrategies,
    strategyById,
    defaultStrategyId,
  }
}
