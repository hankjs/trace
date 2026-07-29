<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { FolderOpen, Plus, Save, Search, Trash2 } from 'lucide-vue-next'
import {
  api,
  type CatalogEntry,
  type FilterLogic,
  type ScreenerCondition,
  type ScreenerGroup,
  type ScreenerItem,
  type ScreenerMatchReason,
  type PoolRef,
  type StructuredScreenerRequest,
} from '../api'
import { categoryLabels, operatorLabels, useCatalog } from '../catalog'
import LoadingRows from '../components/LoadingRows.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import PoolSelect from '../components/PoolSelect.vue'
import QuSelect from '../components/QuSelect.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import { isKnownPoolId } from '../pools'
import { fmtBigAmount, fmtPct, fmtPrice, pnlClass } from '../format'

interface SavedScheme {
  name: string
  logic: FilterLogic
  /** v2 起存 pool_id;null 表示用后端默认池(全部A股) */
  pool_id: number | null
  groups: ScreenerGroup[]
}

/**
 * v1 方案里 universe 存的是 'pool'/'hs300'/'all' 等字符串,与 pool_id 语义不兼容,
 * 直接当 pool_id 用会筛错范围。故 bump 到 _v2:v1 方案不迁移(旧字符串无法可靠映射到
 * 新建的池 id),v2 首次加载为空,并在页面提示旧方案需重建。详见 logs/decisions-web.md。
 */
const STORAGE_KEY = 'quant_screener_schemes_v2'
const LEGACY_STORAGE_KEY = 'quant_screener_schemes_v1'
const { catalog, load: loadCatalog } = useCatalog()
const groups = ref<ScreenerGroup[]>([])
const rootLogic = ref<FilterLogic>('and')
const poolId = ref<number | null>(null)
const items = ref<ScreenerItem[]>([])
const combinedCount = ref(0)
const candidateCount = ref(0)
const conditionCounts = ref<Record<string, number>>({})
const fieldCoverage = ref<Record<string, number>>({})
const resultDate = ref('')
/** 后端回显的实际使用池,用于结果区标注 */
const resultPool = ref<PoolRef | null>(null)
const valuationMaxAgeDays = ref(7)
const searched = ref(false)
const loading = ref(false)
const error = ref('')
const schemeName = ref('')
const selectedScheme = ref('')
const savedSchemes = ref<SavedScheme[]>(readSchemes())
/** 存在 v1 遗留方案时提示用户需重建(v1 的 universe 字符串无法映射到 pool_id) */
const legacySchemeCount = ref(countLegacySchemes())
let sequence = 0

const presets: { name: string; conditions: [string, string, string | boolean, string?][] }[] = [
  { name: '成交活跃', conditions: [['amount_avg20', 'gte', '1'], ['vol_ratio5', 'gte', '1.2']] },
  { name: '趋势稳健', conditions: [['mom20', 'gte', '0'], ['ma_bull', 'eq', true]] },
  { name: '质量价值', conditions: [['pe_ttm', 'between', '0', '25'], ['roe', 'gte', '10']] },
  { name: '财务稳健', conditions: [['roe', 'gte', '12'], ['debt_ratio', 'lte', '60']] },
]

const groupedFields = computed(() => {
  const map = new Map<string, CatalogEntry[]>()
  for (const field of catalog.value.filter_fields) {
    const category = field.category ?? 'basic'
    const values = map.get(category) ?? []
    values.push(field)
    map.set(category, values)
  }
  return [...map.entries()].map(([key, fields]) => ({
    key,
    name: categoryLabels[key] ?? key,
    fields,
  }))
})

const activeConditions = computed(() => groups.value.flatMap((group) => group.conditions).filter((condition) => condition.enabled))
/** 筛选字段下拉选项:按目录分类打平,group 字段驱动 QuSelect 渲染分组标题(替代 optgroup) */
const fieldOptions = computed(() => groupedFields.value.flatMap((category) =>
  category.fields.map((field) => ({
    value: field.key,
    label: `${field.name}${field.available === false ? '（数据待接入）' : ''}`,
    disabled: field.available === false,
    group: category.name,
  })),
))
const boolValueOptions = [
  { value: true, label: '是' },
  { value: false, label: '否' },
]
const schemeOptions = computed(() => [
  { value: '', label: '选择方案' },
  ...savedSchemes.value.map((scheme) => ({ value: scheme.name, label: scheme.name })),
])
function operatorOptions(condition: ScreenerCondition) {
  return operatorsOf(condition).map((operator) => ({ value: operator, label: operatorLabels[operator] ?? operator }))
}
const resultFields = computed(() => [...new Set(activeConditions.value.map((condition) => condition.field))].slice(0, 4))
const missingDataFields = computed(() => [...new Set(
  activeConditions.value
    .filter((condition) => searched.value && fieldCoverage.value[condition.field] === 0)
    .map((condition) => fieldOf(condition.field)?.name ?? condition.field)
)])
const partialDataFields = computed(() => [...new Set(
  activeConditions.value
    .filter((condition) => {
      const coverage = fieldCoverage.value[condition.field]
      return searched.value && typeof coverage === 'number' && coverage < candidateCount.value
    })
    .map((condition) => fieldOf(condition.field)?.name ?? condition.field)
)])
const usesValuation = computed(() => activeConditions.value.some(
  (condition) => fieldOf(condition.field)?.category === 'valuation'
))
const resultColumns = computed<QuTableColumn<ScreenerItem>[]>(() => [
  { key: 'stock', label: '股票' },
  { key: 'industry', label: '行业', cellClass: 'text-text-secondary', value: (item) => item.industry || '--' },
  { key: 'close', label: '最新价', align: 'right', cellClass: (item) => pnlClass(numericValue(item, 'pct_chg')) },
  ...resultFields.value.map((field) => ({
    key: `field:${field}`,
    label: fieldOf(field)?.name ?? field,
    align: 'right' as const,
    cellClass: 'tabular-nums',
    value: (item: ScreenerItem) => valueOf(item, field),
    format: (_value: unknown, item: ScreenerItem) => formatFieldValue(item, field),
  })),
  { key: 'reasons', label: '命中原因', cellClass: 'max-w-sm' },
])

function nextId(prefix: string): string {
  sequence += 1
  return `${prefix}-${Date.now()}-${sequence}`
}

function fieldOf(key: string): CatalogEntry | undefined {
  return catalog.value.filter_fields.find((field) => field.key === key)
}

function defaultOperator(meta: CatalogEntry | undefined): string {
  if ((meta?.data_type === 'number' || meta?.data_type === 'integer') && meta.operators?.includes('gte')) return 'gte'
  if (meta?.operators?.includes('eq')) return 'eq'
  return meta?.operators?.[0] ?? 'gte'
}

function newCondition(field = catalog.value.filter_fields[0]?.key ?? 'pct_chg'): ScreenerCondition {
  const meta = fieldOf(field)
  return {
    id: nextId('condition'),
    field,
    operator: defaultOperator(meta),
    value: meta?.data_type === 'boolean' ? true : '',
    enabled: true,
  }
}

function newGroup(): ScreenerGroup {
  return {
    id: nextId('group'),
    logic: 'and',
    conditions: [newCondition()],
  }
}

function resetBuilder() {
  groups.value = [newGroup()]
  rootLogic.value = 'and'
}

function addGroup() {
  groups.value.push(newGroup())
}

function removeGroup(index: number) {
  groups.value.splice(index, 1)
  if (!groups.value.length) groups.value.push(newGroup())
}

function addCondition(group: ScreenerGroup) {
  group.conditions.push(newCondition())
}

function removeCondition(group: ScreenerGroup, index: number) {
  group.conditions.splice(index, 1)
  if (!group.conditions.length) group.conditions.push(newCondition())
}

function updateField(condition: ScreenerCondition) {
  const meta = fieldOf(condition.field)
  condition.operator = defaultOperator(meta)
  condition.value = meta?.data_type === 'boolean' ? true : ''
  condition.value2 = ''
}

function operatorsOf(condition: ScreenerCondition): string[] {
  return fieldOf(condition.field)?.operators ?? ['gte', 'lte', 'eq']
}

function scaleValue(value: string | number | boolean | null | undefined, meta: CatalogEntry | undefined) {
  if (meta?.data_type === 'boolean') return value === true || value === 'true'
  if (meta?.data_type === 'number' || meta?.data_type === 'integer') {
    const parsed = Number(value)
    return Number.isNaN(parsed) ? value ?? null : parsed * (meta.input_scale ?? 1)
  }
  return value ?? null
}

function buildRequest(): StructuredScreenerRequest {
  const request: StructuredScreenerRequest = {
    logic: rootLogic.value,
    limit: 300,
    groups: groups.value.map((group) => ({
      id: group.id,
      logic: group.logic,
      conditions: group.conditions.map((condition) => {
        const meta = fieldOf(condition.field)
        return {
          ...condition,
          value: scaleValue(condition.value, meta),
          value2: scaleValue(condition.value2, meta) as string | number | null,
        }
      }),
    })),
  }
  // 未选池时不传 pool_id,由后端落到默认池
  if (poolId.value !== null) request.pool_id = poolId.value
  return request
}

function validate(): string {
  if (!activeConditions.value.length) return '请至少启用一个筛选条件'
  for (const condition of activeConditions.value) {
    if (condition.operator === 'is_null' || condition.operator === 'not_null') continue
    if (condition.value === '' || condition.value === null) return `请填写“${fieldOf(condition.field)?.name ?? condition.field}”的条件值`
    if (condition.operator === 'between' && (condition.value2 === '' || condition.value2 === null || condition.value2 === undefined)) {
      return `请填写“${fieldOf(condition.field)?.name ?? condition.field}”的区间上限`
    }
  }
  return ''
}

async function search() {
  const validation = validate()
  if (validation) {
    error.value = validation
    return
  }
  loading.value = true
  error.value = ''
  searched.value = true
  try {
    const result = await api.structuredScreener(buildRequest())
    items.value = result.items ?? []
    combinedCount.value = result.combined_count ?? result.count ?? result.total ?? items.value.length
    candidateCount.value = result.candidate_count ?? 0
    if (result.independent_counts) conditionCounts.value = result.independent_counts
    else if (Array.isArray(result.condition_counts)) {
      conditionCounts.value = Object.fromEntries(result.condition_counts.map((entry) => [entry.id, entry.matched]))
    } else conditionCounts.value = result.condition_counts ?? {}
    fieldCoverage.value = result.field_coverage ?? {}
    resultDate.value = result.date ?? ''
    resultPool.value = result.pool ?? null
    valuationMaxAgeDays.value = result.data_policy?.valuation_max_age_days ?? 7
  } catch (caught) {
    items.value = []
    combinedCount.value = 0
    candidateCount.value = 0
    conditionCounts.value = {}
    fieldCoverage.value = {}
    resultPool.value = null
    error.value = (caught as Error).message
  } finally {
    loading.value = false
  }
}

function applyPreset(preset: typeof presets[number]) {
  groups.value = [{
    id: nextId('group'),
    logic: 'and',
    conditions: preset.conditions.map(([field, operator, value, value2]) => ({
      id: nextId('condition'),
      field,
      operator,
      value,
      value2: value2 ?? '',
      enabled: true,
    })),
  }]
  rootLogic.value = 'and'
  error.value = ''
}

function readSchemes(): SavedScheme[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]')
    if (!Array.isArray(parsed)) return []
    // 只接受 pool_id 为数字或 null 的方案,防御手工改坏的 localStorage
    return parsed.filter((scheme): scheme is SavedScheme =>
      !!scheme && typeof scheme.name === 'string'
      && (scheme.pool_id === null || typeof scheme.pool_id === 'number')
    )
  } catch {
    return []
  }
}

function countLegacySchemes(): number {
  try {
    const parsed = JSON.parse(localStorage.getItem(LEGACY_STORAGE_KEY) ?? '[]')
    return Array.isArray(parsed) ? parsed.length : 0
  } catch {
    return 0
  }
}

/** 用户确认后清掉 v1 遗留数据,不再提示 */
function discardLegacySchemes() {
  localStorage.removeItem(LEGACY_STORAGE_KEY)
  legacySchemeCount.value = 0
}

function cloneGroups(value: ScreenerGroup[]): ScreenerGroup[] {
  return JSON.parse(JSON.stringify(value)) as ScreenerGroup[]
}

function saveScheme() {
  const name = schemeName.value.trim()
  if (!name) {
    error.value = '请先填写方案名称'
    return
  }
  const scheme: SavedScheme = { name, logic: rootLogic.value, pool_id: poolId.value, groups: cloneGroups(groups.value) }
  const index = savedSchemes.value.findIndex((item) => item.name === name)
  if (index >= 0) savedSchemes.value.splice(index, 1, scheme)
  else savedSchemes.value.push(scheme)
  localStorage.setItem(STORAGE_KEY, JSON.stringify(savedSchemes.value))
  selectedScheme.value = name
  error.value = ''
}

function loadScheme() {
  const scheme = savedSchemes.value.find((item) => item.name === selectedScheme.value)
  if (!scheme) return
  groups.value = cloneGroups(scheme.groups)
  rootLogic.value = scheme.logic
  // 方案里的池可能已被删除,失效则回落默认池(由 PoolSelect 补齐)
  poolId.value = isKnownPoolId(scheme.pool_id) ? scheme.pool_id : null
  schemeName.value = scheme.name
  error.value = scheme.pool_id !== null && !isKnownPoolId(scheme.pool_id)
    ? '该方案保存的股票池已不存在，已回退到默认股票池。'
    : ''
}

function deleteScheme() {
  if (!selectedScheme.value) return
  savedSchemes.value = savedSchemes.value.filter((item) => item.name !== selectedScheme.value)
  localStorage.setItem(STORAGE_KEY, JSON.stringify(savedSchemes.value))
  selectedScheme.value = ''
  schemeName.value = ''
}

function conditionSummary(condition: ScreenerCondition): string {
  const meta = fieldOf(condition.field)
  if (condition.operator === 'is_null' || condition.operator === 'not_null') {
    return `${meta?.name ?? condition.field} ${operatorLabels[condition.operator] ?? condition.operator}`
  }
  const unit = meta?.unit ? ` ${meta.unit}` : ''
  const value = condition.value
  const range = condition.operator === 'between' ? ` ${value}${unit} 到 ${condition.value2}${unit}` : ` ${String(value)}${unit}`
  return `${meta?.name ?? condition.field} ${operatorLabels[condition.operator] ?? condition.operator}${range}`
}

function countFor(condition: ScreenerCondition): number | undefined {
  const count = conditionCounts.value[condition.id] ?? conditionCounts.value[condition.field]
  return typeof count === 'number' ? count : undefined
}

function coverageFor(condition: ScreenerCondition): number | undefined {
  const coverage = fieldCoverage.value[condition.field]
  return typeof coverage === 'number' ? coverage : undefined
}

function valueOf(item: ScreenerItem, field: string): unknown {
  return item.values?.[field] ?? item[field]
}

function numericValue(item: ScreenerItem, field: string): number | undefined {
  const value = valueOf(item, field)
  return typeof value === 'number' ? value : undefined
}

function formatFieldValue(item: ScreenerItem, field: string): string {
  const value = valueOf(item, field)
  if (value === null || value === undefined || value === '') return '--'
  const meta = fieldOf(field)
  if (meta?.data_type === 'boolean') return value ? '是' : '否'
  if (typeof value !== 'number') return String(value)
  if (meta?.input_scale === 0.01) return fmtPct(value)
  if (meta?.input_scale === 100000000) return fmtBigAmount(value)
  if (meta?.unit === '倍') return value.toFixed(2)
  return Number.isInteger(value) ? String(value) : value.toFixed(2)
}

function reasonActual(reason: ScreenerMatchReason): string {
  const meta = fieldOf(reason.field)
  const value = reason.actual
  if (value === null || value === undefined) return '暂无数据'
  if (meta?.data_type === 'boolean') return value ? '是' : '否'
  if (typeof value !== 'number') return String(value)
  if (meta?.input_scale === 0.01) return fmtPct(value)
  if (meta?.input_scale === 100000000) return fmtBigAmount(value)
  return Number.isInteger(value) ? String(value) : value.toFixed(2)
}

function matchReasons(item: ScreenerItem): string[] {
  if (item.match_reasons?.length) {
    return item.match_reasons
      .filter((reason) => typeof reason === 'string' || reason.matched)
      .map((reason) => typeof reason === 'string' ? reason : `${reason.field_name ?? fieldOf(reason.field)?.name ?? reason.field}：${reasonActual(reason)}`)
  }
  const matches = item.matched_conditions ?? []
  return matches.map((match) => {
    const condition = activeConditions.value.find((itemCondition) => itemCondition.id === match || itemCondition.field === match)
    return condition ? conditionSummary(condition) : match
  })
}

function dataBasis(item: ScreenerItem): string {
  const parts: string[] = []
  const valuationDate = item.values?.valuation_data_date
  const reportPeriod = item.values?.report_period
  if (valuationDate) parts.push(`估值 ${valuationDate}`)
  if (reportPeriod) parts.push(`财报期 ${reportPeriod}`)
  return parts.join(' · ')
}

function stockName(item: ScreenerItem): string {
  return item.name || '名称待同步'
}

onMounted(async () => {
  await loadCatalog()
  resetBuilder()
})
</script>

<template>
  <div class="space-y-5">
    <section data-tour="screener-pool" aria-labelledby="preset-heading">
      <div class="flex flex-wrap items-start justify-between gap-3">
        <PoolSelect v-model="poolId" label="研究范围（股票池）" />
        <div class="flex flex-wrap items-center gap-2">
        <h2 id="preset-heading" class="mr-1 text-sm font-medium">快速方案</h2>
        <button
          v-for="preset in presets"
          :key="preset.name"
          type="button"
          class="rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text-primary"
          @click="applyPreset(preset)"
        >
          {{ preset.name }}
        </button>
        </div>
      </div>
    </section>

    <section data-tour="screener-builder" class="rounded-md border border-border bg-surface-raised" aria-labelledby="builder-heading">
      <div class="flex flex-wrap items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div>
          <h2 id="builder-heading" class="text-sm font-semibold">筛选条件</h2>
          <p class="mt-0.5 text-xs text-text-tertiary">停用的条件不会参与计算，其他条件保持不变。</p>
        </div>
        <div v-if="groups.length > 1" class="flex items-center gap-2 text-xs">
          <span class="text-text-tertiary">组与组之间</span>
          <div class="flex rounded-md border border-border p-0.5">
            <button type="button" class="rounded px-2 py-1" :class="rootLogic === 'and' ? 'bg-active font-medium text-accent' : 'text-text-secondary'" @click="rootLogic = 'and'">全部满足</button>
            <button type="button" class="rounded px-2 py-1" :class="rootLogic === 'or' ? 'bg-active font-medium text-accent' : 'text-text-secondary'" @click="rootLogic = 'or'">任意满足</button>
          </div>
        </div>
      </div>

      <div class="divide-y divide-border">
        <div v-for="(group, groupIndex) in groups" :key="group.id" class="px-4 py-4">
          <div class="mb-3 flex flex-wrap items-center gap-3">
            <span class="text-xs font-semibold text-text-secondary">条件组 {{ groupIndex + 1 }}</span>
            <div class="flex rounded-md border border-border p-0.5 text-xs">
              <button type="button" class="rounded px-2 py-1" :class="group.logic === 'and' ? 'bg-active font-medium text-accent' : 'text-text-secondary'" @click="group.logic = 'and'">全部满足</button>
              <button type="button" class="rounded px-2 py-1" :class="group.logic === 'or' ? 'bg-active font-medium text-accent' : 'text-text-secondary'" @click="group.logic = 'or'">任意满足</button>
            </div>
            <button
              v-if="groups.length > 1"
              type="button"
              class="icon-button ml-auto !h-7 !w-7"
              title="删除条件组"
              @click="removeGroup(groupIndex)"
            >
              <Trash2 :size="15" />
              <span class="sr-only">删除条件组</span>
            </button>
          </div>

          <div class="space-y-2">
            <div
              v-for="(condition, conditionIndex) in group.conditions"
              :key="condition.id"
              class="grid items-center gap-2 rounded-md bg-surface-muted px-3 py-2 sm:grid-cols-[auto_minmax(150px,1.4fr)_minmax(110px,0.8fr)_minmax(130px,1fr)_auto]"
              :class="condition.enabled ? '' : 'opacity-55'"
            >
              <label class="flex items-center" title="启用或停用此条件">
                <input v-model="condition.enabled" type="checkbox" class="h-4 w-4 accent-accent" />
                <span class="sr-only">启用条件</span>
              </label>

              <label>
                <span class="sr-only">筛选字段</span>
                <QuSelect v-model="condition.field" :options="fieldOptions" class="w-full rounded-md border border-border bg-surface-raised px-2 py-1.5 text-sm" @change="updateField(condition)" />
              </label>

              <label>
                <span class="sr-only">判断关系</span>
                <QuSelect v-model="condition.operator" :options="operatorOptions(condition)" class="w-full rounded-md border border-border bg-surface-raised px-2 py-1.5 text-sm" />
              </label>

              <div class="flex items-center gap-1.5">
                <span v-if="condition.operator === 'is_null' || condition.operator === 'not_null'" class="text-xs text-text-tertiary">无需填写数值</span>
                <template v-else-if="fieldOf(condition.field)?.data_type === 'boolean'">
                  <label class="w-full">
                    <span class="sr-only">条件值</span>
                    <QuSelect v-model="condition.value" :options="boolValueOptions" class="w-full rounded-md border border-border bg-surface-raised px-2 py-1.5 text-sm" />
                  </label>
                </template>
                <template v-else>
                  <label class="min-w-0 flex-1">
                    <span class="sr-only">条件值</span>
                    <input
                      v-model="condition.value"
                      :type="['number', 'integer'].includes(fieldOf(condition.field)?.data_type ?? '') ? 'number' : 'text'"
                      step="any"
                      class="w-full rounded-md border border-border px-2 py-1.5 text-sm"
                      :placeholder="fieldOf(condition.field)?.unit ? `数值（${fieldOf(condition.field)?.unit}）` : '数值'"
                    />
                  </label>
                  <template v-if="condition.operator === 'between'">
                    <span class="text-xs text-text-tertiary">到</span>
                    <label class="min-w-0 flex-1">
                      <span class="sr-only">区间上限</span>
                      <input v-model="condition.value2" type="number" step="any" class="w-full rounded-md border border-border px-2 py-1.5 text-sm" placeholder="上限" />
                    </label>
                  </template>
                </template>
              </div>

              <div class="flex items-center justify-end gap-2">
                <span v-if="countFor(condition) !== undefined || coverageFor(condition) !== undefined" class="whitespace-nowrap text-right text-[11px] leading-4 text-text-tertiary">
                  <span v-if="countFor(condition) !== undefined" class="block">单独命中 {{ countFor(condition) }} 只</span>
                  <span v-if="coverageFor(condition) !== undefined" class="block">数据覆盖 {{ coverageFor(condition) }} / {{ candidateCount }}</span>
                </span>
                <button type="button" class="icon-button !h-7 !w-7" title="删除条件" @click="removeCondition(group, conditionIndex)">
                  <Trash2 :size="14" />
                  <span class="sr-only">删除条件</span>
                </button>
              </div>
            </div>
          </div>

          <button type="button" class="mt-3 inline-flex items-center gap-1 text-xs text-accent hover:underline" @click="addCondition(group)">
            <Plus :size="14" /> 添加条件
          </button>
        </div>
      </div>

      <div class="flex flex-wrap items-end justify-between gap-3 border-t border-border px-4 py-3">
        <button type="button" class="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs text-text-secondary hover:bg-hover" @click="addGroup">
          <Plus :size="14" /> 添加条件组
        </button>
        <div class="flex flex-wrap items-end gap-2">
          <label>
            <span class="mb-1 block text-[11px] text-text-tertiary">方案名称</span>
            <input v-model="schemeName" class="w-32 rounded-md border border-border px-2 py-1.5 text-xs" placeholder="例如：我的低估值" />
          </label>
          <button type="button" class="icon-button border border-border" title="保存筛选方案" @click="saveScheme">
            <Save :size="15" />
            <span class="sr-only">保存筛选方案</span>
          </button>
          <template v-if="savedSchemes.length">
            <label>
              <span class="mb-1 block text-[11px] text-text-tertiary">已保存方案</span>
              <QuSelect v-model="selectedScheme" :options="schemeOptions" class="w-36 rounded-md border border-border bg-surface-raised px-2 py-1.5 text-xs" />
            </label>
            <button type="button" :disabled="!selectedScheme" class="icon-button border border-border disabled:opacity-40" title="载入筛选方案" @click="loadScheme">
              <FolderOpen :size="15" />
              <span class="sr-only">载入筛选方案</span>
            </button>
            <button type="button" :disabled="!selectedScheme" class="icon-button border border-border disabled:opacity-40" title="删除筛选方案" @click="deleteScheme">
              <Trash2 :size="15" />
              <span class="sr-only">删除筛选方案</span>
            </button>
          </template>
          <button type="button" data-tour="screener-run" :disabled="loading" class="inline-flex h-9 items-center gap-1.5 rounded-md bg-accent px-4 text-sm font-medium text-on-accent hover:bg-accent-hover disabled:opacity-50" @click="search">
            <Search :size="16" /> {{ loading ? '筛选中' : '开始筛选' }}
          </button>
        </div>
      </div>
    </section>

    <p
      v-if="legacySchemeCount"
      class="flex flex-wrap items-center gap-2 rounded-md border border-border bg-warning-soft px-4 py-2 text-xs text-text-secondary"
    >
      <span>
        检测到 {{ legacySchemeCount }} 个旧版筛选方案。旧方案保存的研究范围是已废弃的口径名称，无法安全对应到现在的股票池，需要重新保存一次。
      </span>
      <button type="button" class="rounded border border-border px-2 py-0.5 hover:bg-hover" @click="discardLegacySchemes">
        知道了，清除旧方案
      </button>
    </p>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="5" />

    <section v-else-if="searched && !error" data-tour="screener-result" aria-labelledby="result-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h2 id="result-heading" class="text-sm font-semibold">
          组合命中 <span class="text-base text-accent">{{ combinedCount }}</span> 只
          <span v-if="candidateCount" class="ml-1 font-normal text-text-tertiary">（研究范围 {{ candidateCount }} 只）</span>
        </h2>
        <span class="flex items-center gap-3 text-xs text-text-tertiary">
          <span v-if="resultPool">股票池 {{ resultPool.name }}</span>
          <span v-if="resultDate">数据日期 {{ resultDate }}</span>
        </span>
      </div>

      <div v-if="partialDataFields.length" class="mb-3 rounded-md border border-warning/30 bg-warning-soft px-3 py-2 text-xs leading-5 text-warning">
        {{ partialDataFields.join('、') }} 尚未覆盖全部研究范围。缺少数据的股票不会命中对应条件<template v-if="usesValuation">；估值超过 {{ valuationMaxAgeDays }} 天也按缺少数据处理</template>。
      </div>

      <div v-if="items.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
        <QuTable
          :data="items"
          :columns="resultColumns"
          row-key="code"
          class="min-w-[760px]"
          header-cell-class="px-4 py-2.5 font-medium"
          body-cell-class="px-4 py-3"
        >
          <template #cell-stock="{ row: item }">
            <router-link :to="`/stock/${item.code}`" class="font-medium text-text-primary hover:text-accent">{{ stockName(item) }}</router-link>
            <div class="mt-0.5 text-xs text-text-tertiary">{{ item.code }}</div>
          </template>
          <template #cell-close="{ row: item }">{{ fmtPrice(numericValue(item, 'close')) }}</template>
          <template #cell-reasons="{ row: item }">
            <div class="flex flex-wrap gap-1.5">
              <span v-for="reason in matchReasons(item)" :key="reason" class="rounded bg-info-soft px-1.5 py-0.5 text-xs text-text-secondary">{{ reason }}</span>
              <span v-if="!matchReasons(item).length" class="text-xs text-text-tertiary">符合当前组合</span>
            </div>
            <div v-if="dataBasis(item)" class="mt-1.5 text-[11px] text-text-tertiary">{{ dataBasis(item) }}</div>
          </template>
        </QuTable>
      </div>

      <div v-else class="rounded-md border border-dashed border-border px-5 py-10 text-center">
        <template v-if="missingDataFields.length">
          <p class="text-sm font-medium">所选指标尚无已同步数据</p>
          <p class="mt-1 text-xs text-text-tertiary">{{ missingDataFields.join('、') }} 的数据覆盖为 0，请先确认数据同步状态。</p>
        </template>
        <template v-else>
          <p class="text-sm font-medium">当前组合没有匹配股票</p>
          <p class="mt-1 text-xs text-text-tertiary">可停用限制较强的条件，或改为“任意满足”。</p>
        </template>
      </div>
    </section>

    <div v-else-if="!searched" class="rounded-md border border-dashed border-border px-5 py-8 text-center text-sm text-text-tertiary">
      设置并启用条件后开始筛选，结果会显示每条条件的作用。
    </div>
  </div>
</template>
