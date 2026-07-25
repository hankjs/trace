<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api, type LeaderboardItem } from '../api'
import { loadCatalog, metricName, templateName } from '../catalog'
import { fmtPct } from '../format'

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
  // 单标的聚合口径带 _mean/_median 后缀,组合口径为原名;按 mean → 原名 → median 兜底
  const m = it.metrics ?? {}
  for (const k of [`${key}_mean`, key, `${key}_median`]) {
    const v = m[k]
    if (typeof v === 'number' && !Number.isNaN(v)) return v
  }
  return undefined
}

function metricText(v: number | undefined, pct = true): string {
  if (v === undefined) return '--'
  return pct ? fmtPct(v) : v.toFixed(2)
}

function metricClass(v: number | undefined): string {
  if (v === undefined || v === 0) return 'text-text-secondary'
  return v > 0 ? 'text-up' : 'text-down'
}

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

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <div v-else class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border text-left text-xs text-text-tertiary">
            <th class="px-4 py-2 font-medium">#</th>
            <th class="px-4 py-2 font-medium">策略</th>
            <th class="px-4 py-2 font-medium">范围</th>
            <th class="px-4 py-2 font-medium">区间</th>
            <th class="px-4 py-2 text-right font-medium">{{ metricName('total_return') }}</th>
            <th class="px-4 py-2 text-right font-medium">{{ metricName('annual_return') }}</th>
            <th class="px-4 py-2 text-right font-medium">{{ metricName('max_drawdown') }}</th>
            <th class="px-4 py-2 text-right font-medium">{{ metricName('sharpe') }}</th>
            <th class="px-4 py-2 text-right font-medium">{{ metricName('win_rate') }}</th>
            <th class="px-4 py-2 font-medium">评估时间</th>
            <th class="px-4 py-2 font-medium"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="(it, i) in sorted" :key="rowKey(it)">
            <tr
              class="cursor-pointer border-b border-border-subtle hover:bg-hover"
              @click="toggleExpand(it)"
            >
              <td class="px-4 py-2 text-text-tertiary">{{ i + 1 }}</td>
              <td class="px-4 py-2">
                <div class="flex items-center gap-1.5">
                  <span class="font-medium">{{ it.strategy }}</span>
                  <span v-if="!it.is_system" class="rounded bg-active px-1.5 py-0.5 text-[11px] text-accent">自定义</span>
                </div>
                <div class="text-[11px] text-text-tertiary">{{ templateName(it.template) }}</div>
              </td>
              <td class="px-4 py-2">
                <span class="rounded bg-active px-1.5 py-0.5 text-xs text-text-secondary">
                  {{ scopeLabel(it.scope) }}
                </span>
              </td>
              <td class="px-4 py-2 whitespace-nowrap text-text-secondary">{{ it.start }} ~ {{ it.end }}</td>
              <td class="px-4 py-2 text-right" :class="metricClass(metricNum(it, 'total_return'))">
                {{ metricText(metricNum(it, 'total_return')) }}
              </td>
              <td class="px-4 py-2 text-right font-medium" :class="metricClass(metricNum(it, 'annual_return'))">
                {{ metricText(metricNum(it, 'annual_return')) }}
              </td>
              <td class="px-4 py-2 text-right text-down">
                {{ metricText(metricNum(it, 'max_drawdown')) }}
              </td>
              <td class="px-4 py-2 text-right">{{ metricText(metricNum(it, 'sharpe'), false) }}</td>
              <td class="px-4 py-2 text-right">{{ metricText(metricNum(it, 'win_rate')) }}</td>
              <td class="px-4 py-2 whitespace-nowrap text-xs text-text-tertiary">{{ it.run_at || '--' }}</td>
              <td class="px-4 py-2 text-right text-xs text-text-tertiary">
                {{ expanded === rowKey(it) ? '收起 ▲' : '明细 ▼' }}
              </td>
            </tr>
            <tr v-if="expanded === rowKey(it)" class="border-b border-border-subtle bg-hover/50">
              <td colspan="11" class="px-4 py-3">
                <div class="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-6">
                  <div
                    v-for="[k, v] in metricEntries(it)"
                    :key="k"
                    class="rounded-md border border-border bg-surface-raised p-2"
                  >
                    <div class="text-xs text-text-tertiary">{{ k }}</div>
                    <div class="mt-0.5 text-sm font-medium">{{ v }}</div>
                  </div>
                </div>
              </td>
            </tr>
          </template>
        </tbody>
      </table>
      <p v-if="!items.length" class="px-4 py-6 text-center text-sm text-text-tertiary">
        暂无策略评估数据(定时任务评估后生成)
      </p>
    </div>
  </div>
</template>
