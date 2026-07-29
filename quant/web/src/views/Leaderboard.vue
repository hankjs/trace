<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ChevronDown, ChevronRight } from 'lucide-vue-next'
import { api, type LeaderboardItem } from '../api'
import { loadCatalog, metricName, templateName } from '../catalog'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import { aggregateMetric, fmtPct } from '../format'

const items = ref<LeaderboardItem[]>([])
const runAt = ref('')
const loading = ref(true)
const error = ref('')
const expanded = ref<string | null>(null)

/** scope 友好文案,未知值原样展示 */
const scopeLabels: Record<string, string> = {
  pool: '组合(股票池)',
  pool_top50: '组合(Top50)',
  single: '单标的样本',
}

const sorted = computed(() =>
  [...items.value].sort(
    (a, b) => (metricNum(b, 'annual_return') ?? -Infinity) - (metricNum(a, 'annual_return') ?? -Infinity)
  )
)

function scopeLabel(scope: string): string {
  return scopeLabels[scope] ?? scope
}

function metricNum(it: LeaderboardItem, key: string): number | undefined {
  return aggregateMetric(it.metrics, key)
}

function metricText(v: number | undefined, pct = true): string {
  if (v === undefined) return '--'
  return pct ? fmtPct(v) : v.toFixed(2)
}

function metricClass(v: number | undefined): string {
  if (v === undefined || v === 0) return 'text-text-secondary'
  return v > 0 ? 'text-up' : 'text-down'
}

const columns = computed<QuTableColumn<LeaderboardItem>[]>(() => [
  { key: 'rank', label: '#', value: (_item, index) => index + 1, cellClass: 'text-text-tertiary' },
  { key: 'strategy', label: '策略' },
  { key: 'scope', label: '范围' },
  { key: 'period', label: '区间', cellClass: 'whitespace-nowrap text-text-secondary' },
  { key: 'total-return', label: metricName('total_return'), align: 'right', cellClass: (item) => metricClass(metricNum(item, 'total_return')) },
  { key: 'annual-return', label: metricName('annual_return'), align: 'right', cellClass: (item) => `font-medium ${metricClass(metricNum(item, 'annual_return'))}` },
  { key: 'max-drawdown', label: metricName('max_drawdown'), align: 'right', cellClass: 'text-down' },
  { key: 'sharpe', label: metricName('sharpe'), align: 'right' },
  { key: 'win-rate', label: metricName('win_rate'), align: 'right' },
  { key: 'run-at', label: '评估时间', cellClass: 'whitespace-nowrap text-xs text-text-tertiary' },
  { key: 'actions', label: '', align: 'right', cellClass: 'text-xs text-text-tertiary' },
])

function rowKey(it: LeaderboardItem): string {
  return `${it.strategy_id}|${it.scope}|${it.start}|${it.end}`
}

function toggleExpand(it: LeaderboardItem) {
  const k = rowKey(it)
  expanded.value = expanded.value === k ? null : k
}

/** 展开明细:metrics 全量字段,数值保留 4 位 */
function metricEntries(it: LeaderboardItem): [string, string][] {
  return Object.entries(it.metrics ?? {}).map(([k, v]) => [
    metricName(k),
    typeof v === 'number' ? String(+v.toFixed(4)) : String(v),
  ])
}

onMounted(async () => {
  await loadCatalog()
  try {
    const r = await api.leaderboard()
    items.value = r.items ?? []
    runAt.value = r.run_at ?? ''
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-end justify-between gap-3">
      <h2 class="text-base font-semibold">策略比较</h2>
      <p v-if="runAt" class="text-xs text-text-tertiary">最近评估:{{ runAt }}</p>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="5" />

    <div v-else class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
      <QuTable
        :data="sorted"
        :columns="columns"
        :row-key="rowKey"
        body-row-class="cursor-pointer border-b border-border-subtle hover:bg-hover"
        @row-click="toggleExpand"
      >
        <template #cell-strategy="{ row: item }">
          <div class="flex items-center gap-1.5">
            <span class="font-medium">{{ item.strategy }}</span>
            <span v-if="!item.is_system" class="rounded bg-active px-1.5 py-0.5 text-[11px] text-accent">自定义</span>
          </div>
          <div class="text-[11px] text-text-tertiary">{{ templateName(item.template) }}</div>
        </template>
        <template #cell-scope="{ row: item }">
          <span class="rounded bg-active px-1.5 py-0.5 text-xs text-text-secondary">{{ scopeLabel(item.scope) }}</span>
        </template>
        <template #cell-period="{ row: item }">{{ item.start }} ~ {{ item.end }}</template>
        <template #cell-total-return="{ row: item }">{{ metricText(metricNum(item, 'total_return')) }}</template>
        <template #cell-annual-return="{ row: item }">{{ metricText(metricNum(item, 'annual_return')) }}</template>
        <template #cell-max-drawdown="{ row: item }">{{ metricText(metricNum(item, 'max_drawdown')) }}</template>
        <template #cell-sharpe="{ row: item }">{{ metricText(metricNum(item, 'sharpe'), false) }}</template>
        <template #cell-win-rate="{ row: item }">{{ metricText(metricNum(item, 'win_rate')) }}</template>
        <template #cell-run-at="{ row: item }">{{ item.run_at || '--' }}</template>
        <template #cell-actions="{ row: item }">
          <span class="btn btn-ghost btn-sm">
            {{ expanded === rowKey(item) ? '收起' : '明细' }}
            <ChevronDown v-if="expanded === rowKey(item)" :size="13" />
            <ChevronRight v-else :size="13" />
          </span>
        </template>
        <template #after-row="{ row: item, colspan }">
          <tr v-if="expanded === rowKey(item)" class="border-b border-border-subtle bg-hover/50">
            <td :colspan="colspan" class="px-4 py-3">
              <div class="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-6">
                <div
                  v-for="[key, value] in metricEntries(item)"
                  :key="key"
                  class="rounded-md border border-border bg-surface-raised p-2"
                >
                  <div class="text-xs text-text-tertiary">{{ key }}</div>
                  <div class="mt-0.5 text-sm font-medium">{{ value }}</div>
                </div>
              </div>
            </td>
          </tr>
        </template>
      </QuTable>
      <p v-if="!items.length" class="px-4 py-6 text-center text-sm text-text-tertiary">
        暂无策略评估数据(定时任务评估后生成)
      </p>
    </div>
  </div>
</template>
