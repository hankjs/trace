/**
 * 股票池组共享状态:池列表在多个页面复用,集中缓存一次。
 *
 * 默认选中口径为「全部A股」(kind='all'),对齐后端默认池——旧版硬编码的
 * 沪深300+中证500 不再是默认。
 */

import { computed } from 'vue'
import { api, hasSurvivorshipBias, type Pool } from './api'
import { createCachedCollectionStore } from './createCachedCollectionStore'

// 池相关判定与类型统一从本模块出口,页面不必再回到 api.ts 取
export { hasSurvivorshipBias, isPresetPool } from './api'
export type { Pool, PoolKind, PoolMember, PoolRef } from './api'

const collection = createCachedCollectionStore({
  request: () => api.pools(),
  itemsFrom: (response) => response.items ?? [],
})
const pools = collection.items

/** 默认池:优先 kind='all',否则退回第一个可用池 */
export function defaultPool(items: readonly Pool[] = pools.value): Pool | null {
  return items.find((pool) => pool.kind === 'all') ?? items[0] ?? null
}

export function defaultPoolId(items: readonly Pool[] = pools.value): number | null {
  return defaultPool(items)?.id ?? null
}

/** 拉取池列表;并发调用共享同一请求,已加载时直接返回缓存 */
export const loadPools = collection.load

/** 池增删改后调用,强制下次读取走网络 */
export const invalidatePools = collection.invalidate

export const poolById = collection.byId

/** 已保存的 pool_id 是否仍然有效(池可能已被删除) */
export const isKnownPoolId = collection.isKnownId

export function usePools() {
  return {
    pools,
    loading: collection.loading,
    loaded: collection.loaded,
    error: collection.error,
    customPools: computed(() => pools.value.filter((pool) => pool.kind === 'static')),
    presetPools: computed(() => pools.value.filter((pool) => pool.kind !== 'static')),
    load: loadPools,
    invalidate: invalidatePools,
    poolById,
    defaultPoolId,
    hasSurvivorshipBias,
  }
}
