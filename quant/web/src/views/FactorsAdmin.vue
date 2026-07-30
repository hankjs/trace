<script setup lang="ts">
/**
 * 因子库管理(admin 专属):动态因子定义、预览、回填与选股评分配置。
 *
 * - 左侧列表按 category 分组,展示 key / name / enabled / 系统 徽章
 * - 右侧为因子详情/编辑:元数据表单 + SpecExpressionEditor + 校验并保存
 * - 工具栏:预览当前表达式、回填当前/全部因子
 * - 第二 tab:选股配置(评分权重、vol_confirm、hard_filters、top_n)
 */
import { computed, onMounted, ref, watch } from 'vue'
import {
  CheckCircle2,
  Database,
  LayoutList,
  Play,
  Plus,
  RefreshCw,
  Save,
  Settings2,
  Trash2,
  TriangleAlert,
  X,
} from 'lucide-vue-next'
import {
  api,
  normalizeStockCode,
  type ExpressionValidationResult,
  type FactorDef,
  type HardFilter,
  type StrategyAstNode,
} from '../api'
import FactorPreviewPanel from '../components/FactorPreviewPanel.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import QuDatePicker from '../components/QuDatePicker.vue'
import QuSelect from '../components/QuSelect.vue'
import SpecExpressionEditor from '../components/SpecExpressionEditor.vue'
import { confirmDialog } from '../confirmDialog'
import {
  defaultFactorExpression,
  factorByKey,
  type SelectionConfig,
  useFactors,
  useSelectionConfig,
} from '../factors'
import { localDateISO } from '../format'
import { trackTask } from '../tasks'
import { useAsyncAction } from '../useAsyncAction'

interface FactorForm {
  key: string
  name: string
  description: string
  category: string
  unit: string
  direction: string
  limits: string
  value_type: string
  input_scale: number | null
  enabled: boolean
  expression: StrategyAstNode
}

interface ScoreWeightRow {
  key: string
  weight: number
}

const { factors, loading: factorsLoading, load, invalidate, numberFactors, enabledFactors } = useFactors()
const { config: selectionConfig, loading: selectionLoading, load: loadSelection } = useSelectionConfig()

const selectedKey = ref<string | null>(null)
const creating = ref(false)
const activeTab = ref<'factors' | 'selection'>('factors')
const form = ref<FactorForm>(defaultForm())
const validation = ref<ExpressionValidationResult | null>(null)
const validating = ref(false)
const validationError = ref('')
const { busy, error, notice, clear, fail, run: runAction } = useAsyncAction()
const selectionAction = useAsyncAction()

const previewOpen = ref(false)
const backfillOpen = ref(false)
const backfillScope = ref<'current' | 'all'>('current')
const backfillStart = ref('')
const backfillEnd = ref(localDateISO())
const backfillCodes = ref('')

const selected = computed<FactorDef | null>(() => factorByKey(selectedKey.value))
const readonlyFactor = computed(() => selected.value?.is_system === true)

const groupedFactors = computed(() => {
  const map = new Map<string, FactorDef[]>()
  for (const item of factors.value) {
    const category = item.category || '未分类'
    const list = map.get(category) ?? []
    list.push(item)
    map.set(category, list)
  }
  return [...map.entries()].map(([category, items]) => ({
    category,
    items: items.sort((a, b) => a.key.localeCompare(b.key)),
  }))
})

const factorOptions = computed(() =>
  numberFactors.value.map((item) => ({
    value: item.key,
    label: `${item.name} (${item.key})`,
  })),
)

const directionOptions = [
  { value: 'asc', label: '升序(越大越好)' },
  { value: 'desc', label: '降序(越小越好)' },
]

const valueTypeOptions = [
  { value: 'number', label: '数值' },
  { value: 'boolean', label: '布尔' },
  { value: 'string', label: '字符串' },
]

const filterTypeOptions = [
  { value: 'exclude_st', label: '排除 ST' },
  { value: 'exclude_suspended', label: '排除停牌' },
  { value: 'min_bars', label: '最小上市天数' },
  { value: 'factor_gte', label: '因子 ≥' },
  { value: 'factor_lte', label: '因子 ≤' },
  { value: 'factor_gt', label: '因子 >' },
  { value: 'factor_lt', label: '因子 <' },
  { value: 'row_flag', label: '行标志' },
]

const scoreWeightRows = ref<ScoreWeightRow[]>([])
const volConfirm = ref<SelectionConfig['vol_confirm']>({ factor: '', cap: 0.2, weight: 0 })
const hardFilters = ref<HardFilter[]>([])
const topN = ref(20)
const selectionName = ref('默认选股配置')

const weightSum = computed(() =>
  scoreWeightRows.value.reduce((sum, row) => sum + (Number.isFinite(row.weight) ? row.weight : 0), 0),
)

function defaultForm(): FactorForm {
  return {
    key: '',
    name: '',
    description: '',
    category: '',
    unit: '',
    direction: 'desc',
    limits: '',
    value_type: 'number',
    input_scale: null,
    enabled: true,
    expression: defaultFactorExpression(),
  }
}

function formFromFactor(factor: FactorDef): FactorForm {
  return {
    key: factor.key,
    name: factor.name,
    description: factor.description ?? '',
    category: factor.category ?? '',
    unit: factor.unit ?? '',
    direction: factor.direction ?? 'desc',
    limits: factor.limits ?? '',
    value_type: factor.value_type ?? 'number',
    input_scale: factor.input_scale ?? null,
    enabled: factor.enabled,
    expression: (factor.expression as StrategyAstNode) ?? defaultFactorExpression(),
  }
}

function syncSelectionLocal(config: SelectionConfig) {
  selectionName.value = config.name || '默认选股配置'
  scoreWeightRows.value = Object.entries(config.score_weights ?? {}).map(([key, weight]) => ({
    key,
    weight: Number.isFinite(weight) ? weight : 0,
  }))
  volConfirm.value = { ...config.vol_confirm }
  hardFilters.value = (config.hard_filters ?? []).map((item) => ({ ...item }))
  topN.value = config.top_n ?? 20
}

watch([selected, creating], ([factor, isCreating]) => {
  validation.value = null
  validationError.value = ''
  if (isCreating) {
    form.value = defaultForm()
  } else if (factor) {
    form.value = formFromFactor(factor)
  }
}, { immediate: true })

watch(factors, (items) => {
  if (selectedKey.value === null && items.length) {
    selectedKey.value = items[0].key
  }
}, { immediate: true })

watch(selectedKey, () => {
  clear()
})

watch(selectionConfig, (config) => {
  if (config) syncSelectionLocal(config as SelectionConfig)
}, { immediate: true })

function startCreate() {
  creating.value = true
  selectedKey.value = null
  activeTab.value = 'factors'
  validation.value = null
  validationError.value = ''
  clear()
}

async function refreshFactors(selectKey?: string) {
  invalidate()
  const items = await load(true)
  if (selectKey !== undefined) {
    selectedKey.value = selectKey
    creating.value = false
  } else if (!items.some((item) => item.key === selectedKey.value)) {
    selectedKey.value = items[0]?.key ?? null
  }
}

async function validateExpression() {
  validating.value = true
  validationError.value = ''
  try {
    validation.value = await api.validateFactorExpression(form.value.expression)
    if (!validation.value.valid) {
      throw new Error(validation.value.capability.issues.map((issue) => issue.message).join('；') || '表达式未通过校验')
    }
  } catch (caught) {
    validationError.value = (caught as Error).message
    validation.value = null
  } finally {
    validating.value = false
  }
}

function buildCreateBody() {
  return {
    key: form.value.key.trim(),
    name: form.value.name.trim(),
    description: orUndefined(form.value.description),
    category: orUndefined(form.value.category),
    unit: orUndefined(form.value.unit),
    direction: orUndefined(form.value.direction),
    limits: orUndefined(form.value.limits),
    value_type: orUndefined(form.value.value_type),
    input_scale: form.value.input_scale,
    expression: form.value.expression,
    enabled: form.value.enabled,
  }
}

function buildUpdateBody() {
  return {
    name: form.value.name.trim(),
    description: orUndefined(form.value.description),
    category: orUndefined(form.value.category),
    unit: orUndefined(form.value.unit),
    direction: orUndefined(form.value.direction),
    limits: orUndefined(form.value.limits),
    value_type: orUndefined(form.value.value_type),
    input_scale: form.value.input_scale,
    expression: form.value.expression,
    enabled: form.value.enabled,
  }
}

function orUndefined(value: string): string | undefined {
  return value.trim() || undefined
}

async function createFactor() {
  if (!form.value.key.trim()) {
    fail('请填写因子 key')
    return
  }
  if (!form.value.name.trim()) {
    fail('请填写因子名称')
    return
  }
  await runAction(async () => {
    await validateExpression()
    const factor = await api.createFactor(buildCreateBody())
    await refreshFactors(factor.key)
    return factor
  }, { success: (factor) => `已创建因子「${factor.name}」。` })
}

async function saveFactor() {
  if (!selected.value || readonlyFactor.value) return
  if (!form.value.name.trim()) {
    fail('请填写因子名称')
    return
  }
  await runAction(async () => {
    await validateExpression()
    await api.updateFactor(selected.value!.key, buildUpdateBody())
    await refreshFactors(selected.value!.key)
  }, { success: '已保存因子。' })
}

async function toggleEnabled() {
  const factor = selected.value
  if (!factor || readonlyFactor.value) return
  await runAction(async () => {
    await api.updateFactor(factor.key, { enabled: !factor.enabled })
    await refreshFactors(factor.key)
  }, {
    success: factor.enabled
      ? `已停用「${factor.name}」。`
      : `已启用「${factor.name}」。`,
  })
}

async function deleteFactor() {
  const factor = selected.value
  if (!factor || readonlyFactor.value) return
  const confirmed = await confirmDialog(`确认删除因子「${factor.name}」？系统因子不能删除,只能禁用。`, {
    title: '删除因子',
    tone: 'danger',
    confirmText: '删除',
  })
  if (!confirmed) return
  await runAction(async () => {
    await api.deleteFactor(factor.key)
    selectedKey.value = null
    await refreshFactors()
  }, { success: `已删除「${factor.name}」。` })
}

function openPreview() {
  previewOpen.value = true
}

function openBackfill() {
  backfillOpen.value = true
  const end = localDateISO()
  backfillEnd.value = end
  const d = new Date()
  d.setMonth(d.getMonth() - 3)
  backfillStart.value = localDateISO(d)
  backfillCodes.value = ''
}

function parseBackfillCodes(): string[] {
  const raw = backfillCodes.value
  if (!raw.trim()) return []
  const codes: string[] = []
  for (const part of raw.split(/[\s,;，；]+/)) {
    const normalized = normalizeStockCode(part)
    if (normalized) codes.push(normalized)
  }
  return [...new Set(codes)]
}

async function submitBackfill() {
  const factorKey = backfillScope.value === 'current' ? (selected.value?.key ?? null) : null
  if (backfillScope.value === 'current' && !factorKey) {
    fail('请先选择一个因子')
    return
  }
  await runAction(async () => {
    const result = await api.backfillFactors({
      factor_key: factorKey,
      start: backfillStart.value,
      end: backfillEnd.value,
      codes: parseBackfillCodes(),
    })
    trackTask(result.task)
    return result.task
  }, { success: '回填任务已提交,可在任务中心查看进度。' })
  backfillOpen.value = false
}

function newFilter(type: HardFilter['type']): HardFilter {
  switch (type) {
    case 'min_bars':
      return { type, value: 60 }
    case 'factor_gte':
    case 'factor_lte':
    case 'factor_gt':
    case 'factor_lt':
      return { type, factor: '', value: 0 }
    case 'row_flag':
      return { type, field: '', value: false }
    default:
      return { type } as HardFilter
  }
}

function addHardFilter() {
  hardFilters.value.push(newFilter('exclude_st'))
}

function removeHardFilter(index: number) {
  hardFilters.value.splice(index, 1)
}

function onFilterTypeChange(index: number, type: unknown) {
  hardFilters.value.splice(index, 1, newFilter(String(type) as HardFilter['type']))
}

function filterFactorValue(filter: HardFilter): string {
  return filter.type.startsWith('factor_') ? (filter as { factor: string }).factor : ''
}

function setFilterFactorValue(filter: HardFilter, value: unknown) {
  if (filter.type.startsWith('factor_')) {
    ;(filter as { factor: string }).factor = String(value)
  }
}

function filterValue(filter: HardFilter): number {
  if (filter.type === 'min_bars' || filter.type.startsWith('factor_')) {
    return (filter as { value: number }).value
  }
  return 0
}

function setFilterValue(filter: HardFilter, value: number) {
  if (filter.type === 'min_bars' || filter.type.startsWith('factor_')) {
    ;(filter as { value: number }).value = value
  }
}

function filterField(filter: HardFilter): string {
  return filter.type === 'row_flag' ? (filter as { field: string }).field : ''
}

function setFilterField(filter: HardFilter, value: string) {
  if (filter.type === 'row_flag') {
    ;(filter as { field: string }).field = value
  }
}

function filterBoolean(filter: HardFilter): boolean {
  return filter.type === 'row_flag' ? (filter as { value: boolean }).value : false
}

function setFilterBoolean(filter: HardFilter, value: boolean) {
  if (filter.type === 'row_flag') {
    ;(filter as { value: boolean }).value = value
  }
}

function addScoreWeight() {
  scoreWeightRows.value.push({ key: '', weight: 0 })
}

function removeScoreWeight(index: number) {
  scoreWeightRows.value.splice(index, 1)
}

function buildScoreWeights(): Record<string, number> {
  const weights: Record<string, number> = {}
  for (const row of scoreWeightRows.value) {
    if (!row.key) continue
    weights[row.key] = Number.isFinite(row.weight) ? row.weight : 0
  }
  return weights
}

async function saveSelectionConfig() {
  await selectionAction.run(async () => {
    const body = {
      name: selectionName.value.trim() || undefined,
      score_weights: buildScoreWeights(),
      vol_confirm: { ...volConfirm.value },
      hard_filters: hardFilters.value.map((item) => ({ ...item })),
      top_n: topN.value,
    }
    const updated = await api.putSelectionConfig(body)
    syncSelectionLocal(updated)
    return updated
  }, { success: '选股配置已保存。' })
}

onMounted(async () => {
  await load()
  await loadSelection()
})
</script>

<template>
  <div class="space-y-4 lg:flex lg:h-full lg:min-h-0 lg:flex-col lg:gap-4 lg:space-y-0">
    <div class="flex flex-wrap items-center gap-3">
      <div class="segmented">
        <button
          type="button"
          :aria-pressed="activeTab === 'factors'"
          @click="activeTab = 'factors'"
        >
          <LayoutList :size="14" class="inline" /> 因子管理
        </button>
        <button
          type="button"
          :aria-pressed="activeTab === 'selection'"
          @click="activeTab = 'selection'"
        >
          <Settings2 :size="14" class="inline" /> 选股配置
        </button>
      </div>
      <span class="ml-auto text-xs text-text-tertiary">因子数 {{ factors.length }} · 已启用 {{ enabledFactors.length }}</span>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice">{{ notice }}</InlineFeedback>

    <!-- 因子管理 tab -->
    <div
      v-if="activeTab === 'factors'"
      class="grid gap-5 lg:min-h-0 lg:flex-1 lg:grid-cols-[18rem_minmax(0,1fr)] lg:grid-rows-[minmax(0,1fr)]"
    >
      <section class="space-y-3 lg:min-h-0 lg:overflow-y-auto" aria-labelledby="factor-list-heading">
        <h3 id="factor-list-heading" class="text-sm font-semibold">因子列表</h3>
        <LoadingRows v-if="factorsLoading" :rows="3" />
        <template v-else>
          <div v-for="group in groupedFactors" :key="group.category" class="space-y-1.5">
            <span class="block text-xs text-text-tertiary">{{ group.category }}</span>
            <ul class="space-y-1.5">
              <li v-for="factor in group.items" :key="factor.key">
                <button
                  type="button"
                  class="w-full rounded-md border px-3 py-2 text-left text-sm transition-colors"
                  :class="factor.key === selectedKey && !creating
                    ? 'border-accent bg-active text-text-primary'
                    : 'border-border bg-surface-raised text-text-secondary hover:bg-hover'"
                  @click="selectedKey = factor.key; creating = false"
                >
                  <span class="flex items-center gap-1.5">
                    <span class="min-w-0 flex-1 truncate font-medium">{{ factor.name }}</span>
                    <span
                      v-if="factor.is_system"
                      class="shrink-0 rounded bg-surface-muted px-1 py-0 text-[10px] text-text-tertiary"
                    >系统</span>
                    <span
                      v-if="!factor.enabled"
                      class="shrink-0 rounded bg-surface-muted px-1 py-0 text-[10px] text-text-tertiary"
                    >已停用</span>
                  </span>
                  <span class="mt-0.5 block truncate text-xs text-text-tertiary">{{ factor.key }}</span>
                </button>
              </li>
            </ul>
          </div>
          <p v-if="!factors.length" class="rounded-md border border-dashed border-border px-3 py-4 text-center text-xs text-text-tertiary">
            暂无因子
          </p>
        </template>
        <button
          type="button"
          :disabled="busy"
          class="btn btn-primary w-full"
          @click="startCreate"
        >
          <Plus :size="15" />
          新建因子
        </button>
      </section>

      <section
        v-if="creating || selected"
        class="min-w-0 space-y-4 lg:min-h-0 lg:overflow-y-auto"
        aria-labelledby="factor-detail-heading"
      >
        <div class="flex flex-wrap items-end justify-between gap-3 border-b border-border-subtle pb-3">
          <div class="min-w-0">
            <h3 id="factor-detail-heading" class="text-base font-semibold">
              {{ creating ? '新建因子' : selected?.name }}
            </h3>
            <p v-if="!creating && selected" class="mt-0.5 text-xs text-text-tertiary">
              key: {{ selected.key }}
              <template v-if="selected.expression_hash"> · hash {{ selected.expression_hash.slice(0, 12) }}</template>
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <button
              v-if="!creating"
              type="button"
              class="btn btn-secondary"
              :disabled="busy"
              @click="openPreview"
            >
              <Play :size="14" />
              预览
            </button>
            <button
              v-if="!creating && !readonlyFactor"
              type="button"
              class="btn btn-secondary"
              :disabled="busy"
              @click="openBackfill"
            >
              <Database :size="14" />
              回填
            </button>
            <button
              v-if="creating"
              type="button"
              class="btn btn-secondary"
              @click="creating = false; selectedKey = factors[0]?.key ?? null"
            >
              取消
            </button>
          </div>
        </div>

        <div v-if="readonlyFactor" class="flex items-start gap-2 rounded-md border border-border bg-info-soft px-4 py-3 text-sm leading-6 text-text-secondary">
          <TriangleAlert :size="16" class="mt-1 shrink-0 text-text-tertiary" />
          <span>系统因子只读。如需类似因子,请新建自定义因子。</span>
        </div>

        <div class="grid gap-4 sm:grid-cols-2">
          <label class="block text-xs font-medium text-text-secondary"
          >
            因子 key
            <input
              v-model="form.key"
              :disabled="!creating"
              maxlength="64"
              placeholder="如 mom20"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            名称
            <input
              v-model="form.name"
              :disabled="readonlyFactor"
              maxlength="64"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            分类
            <input
              v-model="form.category"
              :disabled="readonlyFactor"
              maxlength="32"
              placeholder="如 momentum"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            单位
            <input
              v-model="form.unit"
              :disabled="readonlyFactor"
              maxlength="16"
              placeholder="如 % / 倍"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            方向
            <QuSelect
              v-model="form.direction"
              :options="directionOptions"
              :disabled="readonlyFactor"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            值类型
            <QuSelect
              v-model="form.value_type"
              :options="valueTypeOptions"
              :disabled="readonlyFactor"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            输入缩放
            <input
              v-model.number="form.input_scale"
              type="number"
              step="any"
              :disabled="readonlyFactor"
              placeholder="留空表示不缩放"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary sm:col-span-2"
          >
            限制说明
            <input
              v-model="form.limits"
              :disabled="readonlyFactor"
              maxlength="256"
              placeholder="如 上市不足 60 日无效"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary sm:col-span-2"
          >
            描述
            <textarea
              v-model="form.description"
              :disabled="readonlyFactor"
              rows="2"
              maxlength="512"
              class="mt-1 w-full rounded-md border border-border bg-surface-raised px-2.5 py-2 text-sm outline-none focus:ring-2 focus:ring-accent disabled:opacity-55"
            />
          </label>
        </div>

        <div class="flex items-center gap-2">
          <label class="flex items-center gap-2 text-sm text-text-secondary"
          >
            <input
              v-model="form.enabled"
              type="checkbox"
              :disabled="readonlyFactor"
              class="h-4 w-4 accent-accent"
            />
            启用该因子
          </label>
        </div>

        <section class="rounded-md border border-border bg-surface-raised" aria-labelledby="expression-heading">
          <div class="border-b border-border-subtle px-4 py-3">
            <h4 id="expression-heading" class="text-sm font-semibold">表达式</h4>
            <p class="text-xs text-text-tertiary">根节点必须为数值类型。</p>
          </div>
          <div class="p-4">
            <SpecExpressionEditor
              v-model="form.expression"
              expected-type="number"
              :disabled="readonlyFactor"
            />
          </div>
        </section>

        <section class="rounded-md border border-border bg-surface-raised" aria-labelledby="validation-heading">
          <div class="flex flex-wrap items-center justify-between gap-3 border-b border-border-subtle px-4 py-3">
            <div class="flex items-center gap-2">
              <CheckCircle2 v-if="validation?.valid" :size="17" class="text-down" />
              <TriangleAlert v-else :size="17" class="text-warning" />
              <div>
                <h4 id="validation-heading" class="text-sm font-semibold">校验与能力</h4>
                <p class="text-xs text-text-tertiary">
                  {{ validation ? '表达式已通过服务端校验' : '保存前必须通过服务端校验' }}
                </p>
              </div>
            </div>
            <button
              type="button"
              :disabled="busy || validating || readonlyFactor"
              class="btn btn-secondary"
              @click="validateExpression"
            >
              <RefreshCw :size="14" :class="validating ? 'animate-spin' : ''" />
              校验表达式
            </button>
          </div>
          <div class="p-4">
            <InlineFeedback v-if="validationError" tone="error">{{ validationError }}</InlineFeedback>
            <ul v-if="validation?.capability.issues.length" class="space-y-2">
              <li
                v-for="issue in validation.capability.issues"
                :key="`${issue.code}-${issue.path}`"
                class="rounded-md bg-surface-muted px-3 py-2 text-sm text-text-secondary"
              >
                <span class="font-medium text-text-primary">{{ issue.message }}</span>
                <code class="ml-2 text-xs text-text-tertiary">{{ issue.path }}</code>
              </li>
            </ul>
            <div v-else-if="validation?.valid" class="space-y-1 text-sm text-text-secondary">
              <p>表达式结构、字段依赖与操作符均通过校验。</p>
              <p class="text-xs text-text-tertiary">
                最小 K 线数:{{ validation.min_bars ?? '—' }}
                · 使用字段:{{ (validation.used_fields ?? []).join(', ') || '无' }}
                · 结果类型:{{ validation.result_type ?? '—' }}
              </p>
            </div>
            <p v-else class="text-sm text-text-tertiary">点击「校验表达式」查看能力评估。</p>
          </div>
        </section>

        <div class="flex flex-wrap items-center justify-between gap-3 border-t border-border-subtle pt-4">
          <div v-if="!creating && !readonlyFactor" class="flex items-center gap-2">
            <button
              type="button"
              :disabled="busy"
              class="btn btn-secondary"
              @click="toggleEnabled"
            >
              {{ selected?.enabled ? '停用因子' : '启用因子' }}
            </button>
            <button
              type="button"
              :disabled="busy"
              class="btn btn-danger"
              title="删除因子"
              @click="deleteFactor"
            >
              <Trash2 :size="14" />
              删除
            </button>
          </div>
          <span v-else />
          <button
            v-if="creating || !readonlyFactor"
            type="button"
            :disabled="busy || validating"
            class="btn btn-primary"
            @click="creating ? createFactor() : saveFactor()"
          >
            <Save :size="14" />
            {{ creating ? '校验并创建' : '校验并保存' }}
          </button>
        </div>
      </section>

      <section
        v-else
        class="rounded-md border border-dashed border-border px-5 py-12 text-center text-sm text-text-tertiary"
      >
        选择左侧因子查看详情,或新建一个自定义因子。
      </section>
    </div>

    <!-- 选股配置 tab -->
    <div v-else class="space-y-4">
      <InlineFeedback v-if="selectionAction.error.value" tone="error">{{ selectionAction.error.value }}</InlineFeedback>
      <InlineFeedback v-if="selectionAction.notice.value">{{ selectionAction.notice.value }}</InlineFeedback>

      <section class="rounded-md border border-border bg-surface-raised">
        <div class="border-b border-border-subtle px-4 py-3">
          <h3 class="text-sm font-semibold">评分权重</h3>
          <p class="text-xs text-text-tertiary">仅数值型因子可参与评分。权重和不需要等于 1,系统会按相对权重归一化。</p>
        </div>
        <div class="divide-y divide-border">
          <div
            v-for="(row, index) in scoreWeightRows"
            :key="index"
            class="flex flex-wrap items-center gap-3 px-4 py-3"
          >
            <QuSelect
              v-model="row.key"
              :options="factorOptions"
              placeholder="选择因子"
              class="h-9 w-56 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
            <label class="flex items-center gap-2 text-sm"
            >
              <span class="text-xs text-text-tertiary">权重</span>
              <input
                v-model.number="row.weight"
                type="number"
                step="any"
                class="h-9 w-24 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
              />
            </label>
            <button
              type="button"
              class="icon-button !h-8 !w-8"
              title="删除"
              @click="removeScoreWeight(index)"
            >
              <Trash2 :size="14" />
            </button>
          </div>
          <div v-if="!scoreWeightRows.length" class="px-4 py-4 text-sm text-text-tertiary">
            尚未添加评分因子。
          </div>
        </div>
        <div class="flex flex-wrap items-center justify-between gap-3 border-t border-border-subtle px-4 py-3">
          <span class="text-xs text-text-tertiary">权重合计:{{ weightSum.toFixed(4) }}</span>
          <button type="button" class="btn btn-secondary btn-sm" @click="addScoreWeight">
            <Plus :size="14" />
            添加因子
          </button>
        </div>
      </section>

      <section class="rounded-md border border-border bg-surface-raised">
        <div class="border-b border-border-subtle px-4 py-3">
          <h3 class="text-sm font-semibold">成交量确认</h3>
          <p class="text-xs text-text-tertiary">用于控制入选个股的成交量过滤与加权。</p>
        </div>
        <div class="grid gap-4 px-4 py-4 sm:grid-cols-3"
        >
          <label class="block text-xs font-medium text-text-secondary"
          >
            因子
            <QuSelect
              v-model="volConfirm.factor"
              :options="factorOptions"
              placeholder="选择因子"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            上限(cap)
            <input
              v-model.number="volConfirm.cap"
              type="number"
              step="any"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            权重
            <input
              v-model.number="volConfirm.weight"
              type="number"
              step="any"
              class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
        </div>
      </section>

      <section class="rounded-md border border-border bg-surface-raised">
        <div class="border-b border-border-subtle px-4 py-3">
          <h3 class="text-sm font-semibold">硬性过滤</h3>
        </div>
        <div class="divide-y divide-border"
        >
          <div
            v-for="(filter, index) in hardFilters"
            :key="index"
            class="flex flex-wrap items-end gap-3 px-4 py-3"
          >
            <label class="block text-xs font-medium text-text-secondary"
            >
              类型
              <QuSelect
                :model-value="filter.type"
                :options="filterTypeOptions"
                class="mt-1 h-9 w-40 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
                @change="onFilterTypeChange(index, $event)"
              />
            </label>

            <template v-if="filter.type === 'min_bars'"
            >
              <label class="block text-xs font-medium text-text-secondary"
              >
                最小天数
                <input
                  :value="filterValue(filter)"
                  type="number"
                  min="1"
                  class="mt-1 h-9 w-28 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
                  @input="setFilterValue(filter, Number(($event.target as HTMLInputElement).value))"
                />
              </label>
            </template>

            <template v-else-if="filter.type.startsWith('factor_')"
            >
              <QuSelect
                :model-value="filterFactorValue(filter)"
                :options="factorOptions"
                placeholder="选择因子"
                class="h-9 w-56 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
                @change="setFilterFactorValue(filter, $event)"
              />
              <label class="block text-xs font-medium text-text-secondary"
              >
                阈值
                <input
                  :value="filterValue(filter)"
                  type="number"
                  step="any"
                  class="mt-1 h-9 w-28 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
                  @input="setFilterValue(filter, Number(($event.target as HTMLInputElement).value))"
                />
              </label>
            </template>

            <template v-else-if="filter.type === 'row_flag'"
            >
              <label class="block text-xs font-medium text-text-secondary"
              >
                字段
                <input
                  :value="filterField(filter)"
                  class="mt-1 h-9 w-32 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
                  @input="setFilterField(filter, ($event.target as HTMLInputElement).value)"
                />
              </label>
              <label class="flex items-center gap-2 text-sm"
              >
                <input
                  :checked="filterBoolean(filter)"
                  type="checkbox"
                  class="h-4 w-4 accent-accent"
                  @change="setFilterBoolean(filter, ($event.target as HTMLInputElement).checked)"
                />
                为真
              </label>
            </template>

            <button
              type="button"
              class="icon-button !h-8 !w-8"
              title="删除"
              @click="removeHardFilter(index)"
            >
              <Trash2 :size="14" />
            </button>
          </div>
          <div v-if="!hardFilters.length" class="px-4 py-4 text-sm text-text-tertiary">
            尚未添加硬性过滤。
          </div>
        </div>
        <div class="border-t border-border-subtle px-4 py-3"
        >
          <button type="button" class="btn btn-secondary btn-sm" @click="addHardFilter">
            <Plus :size="14" />
            添加过滤
          </button>
        </div>
      </section>

      <section class="rounded-md border border-border bg-surface-raised">
        <div class="flex flex-wrap items-end gap-4 px-4 py-4"
        >
          <label class="block text-xs font-medium text-text-secondary"
          >
            配置名称
            <input
              v-model="selectionName"
              class="mt-1 h-9 w-64 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <label class="block text-xs font-medium text-text-secondary"
          >
            入选数量(top_n)
            <input
              v-model.number="topN"
              type="number"
              min="1"
              max="500"
              class="mt-1 h-9 w-28 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <button
            type="button"
            :disabled="selectionAction.busy.value"
            class="btn btn-primary ml-auto"
            @click="saveSelectionConfig"
          >
            <Save :size="14" />
            保存选股配置
          </button>
        </div>
      </section>

      <div v-if="selectionLoading" class="text-sm text-text-tertiary">加载配置中…</div>
    </div>

    <!-- 预览弹窗 -->
    <FactorPreviewPanel
      v-if="previewOpen && !creating"
      :expression="selected?.expression"
      :factor-key="selected?.key"
      @close="previewOpen = false"
    />

    <!-- 回填弹窗 -->
    <div v-if="backfillOpen" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    >
      <div class="w-full max-w-md rounded-lg bg-surface-raised p-5 shadow-panel"
      >
        <div class="mb-4 flex items-center justify-between"
        >
          <h3 class="text-base font-semibold">因子回填</h3>
          <button type="button" class="icon-button" @click="backfillOpen = false">
            <X :size="17" />
          </button>
        </div>

        <div class="space-y-4"
        >
          <div class="flex gap-3"
          >
            <label class="flex items-center gap-2 text-sm"
            >
              <input v-model="backfillScope" type="radio" value="current" />
              当前因子({{ selected?.name }})
            </label>
            <label class="flex items-center gap-2 text-sm"
            >
              <input v-model="backfillScope" type="radio" value="all" />
              全部启用因子
            </label>
          </div>

          <div class="grid grid-cols-2 gap-3"
          >
            <label class="block text-xs font-medium text-text-secondary"
            >
              开始日期
              <QuDatePicker v-model="backfillStart" :clearable="false" class="mt-1 h-9 w-full" />
            </label>
            <label class="block text-xs font-medium text-text-secondary"
            >
              结束日期
              <QuDatePicker v-model="backfillEnd" :clearable="false" class="mt-1 h-9 w-full" />
            </label>
          </div>

          <label class="block text-xs font-medium text-text-secondary"
          >
            股票代码(可选,逗号/空格/换行分隔)
            <textarea
              v-model="backfillCodes"
              rows="3"
              placeholder="sh.600000, sz.000001"
              class="mt-1 w-full rounded-md border border-border bg-surface-raised px-2.5 py-2 text-sm"
            />
          </label>

          <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>

          <div class="flex justify-end gap-2"
          >
            <button type="button" class="btn btn-secondary" @click="backfillOpen = false">取消</button>
            <button
              type="button"
              :disabled="busy"
              class="btn btn-primary"
              @click="submitBackfill"
            >
              提交回填
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
