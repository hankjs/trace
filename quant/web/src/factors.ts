/**
 * 动态因子库:FactorDef / 选股配置 / 表达式校验 / 预览 / 回填
 *
 * 列表按 createCachedCollectionStore 缓存,与 strategies.ts 同一套模式;
 * 选股配置单独一个模块级 loader,因为不是简单集合缓存。
 */

import { computed, readonly, ref, shallowRef } from 'vue'
import { api, type QuantTask, type StrategyAstNode } from './api'
import { createCachedCollectionStore } from './createCachedCollectionStore'

export type FactorDirection = 'asc' | 'desc' | string
export type FactorValueType = 'number' | 'boolean' | 'string' | string

export interface FactorDef {
  id: number
  key: string
  name: string
  description: string | null
  category: string | null
  unit: string | null
  direction: FactorDirection | null
  limits: string | null
  value_type: FactorValueType | null
  input_scale: number | null
  expression: StrategyAstNode | Record<string, unknown>
  expression_hash: string | null
  min_bars: number | null
  enabled: boolean
  is_system: boolean
  created_at: string | null
  updated_at: string | null
}

export type HardFilter =
  | { type: 'exclude_st' }
  | { type: 'exclude_suspended' }
  | { type: 'min_bars'; value: number }
  | { type: 'factor_gte' | 'factor_lte' | 'factor_gt' | 'factor_lt'; factor: string; value: number }
  | { type: 'row_flag'; field: string; value: boolean }

export interface VolConfirmConfig {
  factor: string
  cap: number
  weight: number
}

export interface SelectionConfig {
  id: number
  name: string
  is_active: boolean
  score_weights: Record<string, number>
  vol_confirm: VolConfirmConfig
  hard_filters: HardFilter[]
  top_n: number
  updated_at: string | null
}

export interface FactorValidationIssue {
  status: string
  path: string
  code: string
  message: string
}

export interface FactorValidationCapability {
  status: string
  issues: FactorValidationIssue[]
}

export interface ExpressionValidationResult {
  valid: boolean
  expression_hash: string | null
  canonical_json: string | null
  result_type: string | null
  min_bars: number | null
  used_fields: string[] | null
  capability: FactorValidationCapability
}

export interface ReasonNode {
  op: string
  value?: number | null
  field?: string | null
  literal?: number | boolean | null
  window?: number | null
  shift?: number | null
  periods?: number | null
  ascending?: boolean | null
  n?: number | null
  children?: ReasonNode[]
}

export interface FactorPreviewResult {
  code: string
  dates: string[]
  values: (number | null)[]
  reason_tree: ReasonNode
}

export interface FactorBackfillRequest {
  factor_key: string | null
  start: string
  end: string
  codes?: string[]
}

const selectionConfig = shallowRef<SelectionConfig | null>(null)
const selectionConfigLoading = ref(false)
const selectionConfigError = ref('')

const collection = createCachedCollectionStore({
  request: () => api.listFactors(),
  itemsFrom: (response) => response.items ?? [],
})

export const factors = collection.items
export const loadFactors = collection.load
export const invalidateFactors = collection.invalidate
export const factorById = collection.byId
export const resetFactors = collection.reset

export function factorByKey(
  key: string | null | undefined,
  items: readonly FactorDef[] = factors.value,
): FactorDef | null {
  if (!key) return null
  return items.find((item) => item.key === key) ?? null
}

export function useFactors() {
  return {
    factors,
    loading: collection.loading,
    loaded: collection.loaded,
    error: collection.error,
    load: loadFactors,
    invalidate: invalidateFactors,
    byId: factorById,
    byKey: factorByKey,
    reset: resetFactors,
    systemFactors: computed(() => factors.value.filter((item) => item.is_system)),
    customFactors: computed(() => factors.value.filter((item) => !item.is_system)),
    enabledFactors: computed(() => factors.value.filter((item) => item.enabled)),
    numberFactors: computed(() => factors.value.filter((item) => item.value_type === 'number')),
  }
}

export async function loadSelectionConfig(): Promise<SelectionConfig | null> {
  if (selectionConfigLoading.value) return selectionConfig.value
  selectionConfigLoading.value = true
  selectionConfigError.value = ''
  try {
    const config = await api.getSelectionConfig()
    selectionConfig.value = config
    return config
  } catch (caught) {
    selectionConfigError.value = (caught as Error).message
    throw caught
  } finally {
    selectionConfigLoading.value = false
  }
}

export function useSelectionConfig() {
  return {
    config: readonly(selectionConfig),
    loading: readonly(selectionConfigLoading),
    error: readonly(selectionConfigError),
    load: loadSelectionConfig,
  }
}

export function defaultFactorExpression(): StrategyAstNode {
  return { op: 'field', name: 'close' }
}

export function emptySelectionConfig(): SelectionConfig {
  return {
    id: 0,
    name: '默认选股配置',
    is_active: false,
    score_weights: {},
    vol_confirm: { factor: '', cap: 0.2, weight: 0 },
    hard_filters: [{ type: 'exclude_st' }],
    top_n: 20,
    updated_at: null,
  }
}

export type { QuantTask, StrategyAstNode }
