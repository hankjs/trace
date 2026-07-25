/**
 * 股票池组共享状态:池列表在多个页面复用,集中缓存一次。
 *
 * 默认选中口径为「全部A股」(kind='all'),对齐后端默认池——旧版硬编码的
 * 沪深300+中证500 不再是默认。
 */

import { computed, readonly, ref, shallowRef } from 'vue'
import { api, hasSurvivorshipBias, type Pool } from './api'

// 池相关判定与类型统一从本模块出口,页面不必再回到 api.ts 取
export { hasSurvivorshipBias, isPresetPool } from './api'
export type { Pool, PoolKind, PoolMember, PoolRef } from './api'

const pools = shallowRef<Pool[]>([])
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
let inflight: Promise<Pool[]> | null = null

/** 默认池:优先 kind='all',否则退回第一个可用池 */
export function defaultPool(items: Pool[] = pools.value): Pool | null {
  return items.find((pool) => pool.kind === 'all') ?? items[0] ?? null
}

export function defaultPoolId(items: Pool[] = pools.value): number | null {
  return defaultPool(items)?.id ?? null
}

/** 拉取池列表;并发调用共享同一请求,已加载时直接返回缓存 */
export async function loadPools(force = false): Promise<Pool[]> {
  if (loaded.value && !force) return pools.value
  if (inflight) return inflight
  loading.value = true
  error.value = ''
  inflight = (async () => {
    try {
      const response = await api.pools()
      pools.value = response.items ?? []
      loaded.value = true
      return pools.value
    } catch (caught) {
      error.value = (caught as Error).message
      throw caught
    } finally {
      loading.value = false
      inflight = null
    }
  })()
  return inflight
}

/** 池增删改后调用,强制下次读取走网络 */
export function invalidatePools() {
  loaded.value = false
}

export function poolById(id: number | null | undefined): Pool | null {
  if (id === null || id === undefined) return null
  return pools.value.find((pool) => pool.id === id) ?? null
}

/** 已保存的 pool_id 是否仍然有效(池可能已被删除) */
export function isKnownPoolId(id: unknown): id is number {
  return typeof id === 'number' && pools.value.some((pool) => pool.id === id)
}

export function usePools() {
  return {
    pools: readonly(pools),
    loading: readonly(loading),
    loaded: readonly(loaded),
    error: readonly(error),
    customPools: computed(() => pools.value.filter((pool) => pool.kind === 'static')),
    presetPools: computed(() => pools.value.filter((pool) => pool.kind !== 'static')),
    load: loadPools,
    invalidate: invalidatePools,
    poolById,
    defaultPoolId,
    hasSurvivorshipBias,
  }
}
