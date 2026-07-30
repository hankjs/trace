<script setup lang="ts">
/** 因子表达式预览面板:选股票、设天数,查看逐日数值与计算树。 */
import { computed, ref, watch } from 'vue'
import { Play, X } from 'lucide-vue-next'
import { api, type FactorPreviewResult, type ReasonNode, type StrategyAstNode } from '../api'
import InlineFeedback from './InlineFeedback.vue'
import LoadingRows from './LoadingRows.vue'
import QuTable from './QuTable.vue'
import ReasonTree from './ReasonTree.vue'
import StockSearchInput from './StockSearchInput.vue'
import type { QuTableColumn } from './quTable'
import { useAsyncAction } from '../useAsyncAction'

const props = defineProps<{
  expression?: StrategyAstNode | Record<string, unknown>
  factorKey?: string
}>()

const emit = defineEmits<{
  close: []
}>()

const code = ref('')
const days = ref(60)
const result = ref<FactorPreviewResult | null>(null)
const loading = ref(false)
const error = ref('')
const { busy, run } = useAsyncAction()

const effectiveBusy = computed(() => busy.value || loading.value)

const columns = computed<QuTableColumn<{ date: string; value: number | null }>[]>(() => [
  { key: 'date', label: '日期' },
  {
    key: 'value',
    label: '因子值',
    align: 'right',
    cellClass: 'tabular-nums',
    format: (value) => (value === null || value === undefined ? '—' : Number(value).toFixed(4)),
  },
])

const rows = computed(() => {
  if (!result.value) return []
  return result.value.dates.map((date, index) => ({
    date,
    value: result.value!.values[index] ?? null,
  }))
})

watch(
  () => props.factorKey,
  () => {
    result.value = null
  },
)

async function preview() {
  if (!code.value) {
    error.value = '请选择或输入股票代码'
    return
  }
  if (!props.expression && !props.factorKey) {
    error.value = '缺少表达式或因子 key'
    return
  }
  error.value = ''
  loading.value = true
  await run(async () => {
    const payload: {
      expression?: StrategyAstNode | Record<string, unknown>
      factor_key?: string
      code: string
      days?: number
    } = { code: code.value, days: days.value }
    if (props.factorKey) {
      payload.factor_key = props.factorKey
    } else if (props.expression) {
      payload.expression = props.expression
    }
    result.value = await api.previewFactor(payload)
    return result.value
  }, { success: '预览数据已更新。' })
  loading.value = false
}

function reasonTreeRoot(): ReasonNode | null {
  return result.value?.reason_tree ?? null
}

function onClose() {
  emit('close')
}
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
    <div class="flex max-h-[90vh] w-full max-w-4xl flex-col rounded-lg bg-surface-raised shadow-panel">
      <div class="flex items-center justify-between border-b border-border px-4 py-3">
        <div>
          <h3 class="text-base font-semibold">因子预览</h3>
          <p class="text-xs text-text-tertiary">查看表达式在单只股票上的逐日计算结果与推理树。</p>
        </div>
        <button type="button" class="icon-button" title="关闭" @click="onClose">
          <X :size="17" />
        </button>
      </div>

      <div class="space-y-4 overflow-y-auto p-4">
        <div class="flex flex-wrap items-end gap-3">
          <StockSearchInput v-model="code" label="股票" placeholder="输入名称或代码" />
          <label class="block text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">天数</span>
            <input
              v-model.number="days"
              type="number"
              min="5"
              max="500"
              class="h-9 w-24 rounded-md border border-border bg-surface-raised px-2.5 text-sm"
            />
          </label>
          <button
            type="button"
            class="btn btn-primary"
            :disabled="effectiveBusy"
            @click="preview"
          >
            <Play :size="14" />
            预览
          </button>
        </div>

        <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>

        <LoadingRows v-if="loading" :rows="4" />

        <template v-else-if="result">
          <section class="rounded-md border border-border bg-surface-raised">
            <div class="border-b border-border-subtle px-3 py-2 text-sm font-medium">
              {{ result.code }} · 共 {{ result.dates.length }} 个交易日
            </div>
            <div class="max-h-72 overflow-auto">
              <QuTable
                :data="rows"
                :columns="columns"
                row-key="date"
                header-cell-class="px-3 py-2 text-xs"
                body-cell-class="px-3 py-1.5 text-xs"
              />
            </div>
          </section>

          <section class="rounded-md border border-border bg-surface-raised p-3">
            <div class="mb-2 text-sm font-medium">推理树</div>
            <ReasonTree v-if="reasonTreeRoot()" :node="reasonTreeRoot()!" />
            <p v-else class="text-xs text-text-tertiary">无推理树。</p>
          </section>
        </template>

        <p v-else class="rounded-md border border-dashed border-border px-4 py-8 text-center text-sm text-text-tertiary">
          选择股票并点击预览,即可查看因子逐日计算结果。
        </p>
      </div>
    </div>
  </div>
</template>
