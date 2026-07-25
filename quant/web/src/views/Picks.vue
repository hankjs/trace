<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ChevronDown, ChevronUp } from 'lucide-vue-next'
import { api, type PickItem } from '../api'
import { catalogEntry, factorName, loadCatalog } from '../catalog'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import { fmtBigAmount, fmtPct, localDateISO } from '../format'

const date = ref('')
const items = ref<PickItem[]>([])
const dropped = ref<(PickItem | string)[]>([])
const loading = ref(true)
const error = ref('')
const expanded = ref<string | null>(null)

const factorKeys = ['mom20', 'mom60', 'rsi14', 'atr_pct', 'vol_ratio5', 'ma20_slope', 'amount_avg20']

/** 新进标记:后端以 change='new' 表示 */
function isNew(p: PickItem): boolean {
  return p.change === 'new'
}

function droppedCode(d: PickItem | string): string {
  return typeof d === 'string' ? d : d.code
}

function droppedName(d: PickItem | string): string {
  return typeof d === 'string' ? '' : (d.name ?? '')
}

function factorText(p: PickItem, key: string): string {
  const v = p.factors?.[key]
  if (v === null || v === undefined || Number.isNaN(v)) return '--'
  const entry = catalogEntry('factors', key)
  if (entry?.input_scale === 100000000 || key === 'amount_avg20') return fmtBigAmount(v)
  if (entry?.unit === '0-100' || entry?.unit === '倍' || key === 'rsi14' || key === 'vol_ratio5') return v.toFixed(2)
  return fmtPct(v)
}

function toggleExpand(code: string) {
  expanded.value = expanded.value === code ? null : code
}

const emptyText = computed(() =>
  date.value >= localDateISO()
    ? '今日还未生成选股池(交易日 17:00 生成)'
    : '该日期无选股池数据'
)

async function load() {
  loading.value = true
  error.value = ''
  expanded.value = null
  try {
    const requestedDate = date.value
    const r = await api.picks(requestedDate || undefined)
    if (!requestedDate && r.date) date.value = r.date
    items.value = r.items ?? []
    dropped.value = r.dropped ?? []
  } catch (e) {
    items.value = []
    dropped.value = []
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}

onMounted(async () => {
  await loadCatalog()
  await load()
})
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-end justify-between gap-3">
      <div>
        <h2 class="text-base font-semibold">系统候选</h2>
        <p class="mt-1 text-xs text-text-tertiary">按每日量化评分生成，展开可查看各项指标。</p>
      </div>
      <form class="flex items-end gap-3" @submit.prevent="load">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">日期</span>
          <input v-model="date" type="date" class="rounded-md border border-border bg-surface-raised px-2 py-1.5" />
        </label>
        <button type="submit" class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover">
          查询
        </button>
      </form>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="5" />

    <template v-else>
      <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-xs text-text-tertiary">
              <th class="px-4 py-2 font-medium">排名</th>
              <th class="px-4 py-2 font-medium">股票</th>
              <th class="px-4 py-2 text-right font-medium">评分</th>
              <th class="px-4 py-2 text-right font-medium">{{ factorName('mom20') }}</th>
              <th class="px-4 py-2 text-right font-medium">{{ factorName('mom60') }}</th>
              <th class="px-4 py-2 text-right font-medium">{{ factorName('rsi14') }}</th>
              <th class="px-4 py-2 text-right font-medium">{{ factorName('vol_ratio5') }}</th>
              <th class="px-4 py-2 font-medium"></th>
            </tr>
          </thead>
          <tbody>
            <template v-for="p in items" :key="p.code">
              <tr
                class="border-b border-border-subtle hover:bg-hover"
              >
                <td class="px-4 py-2 font-medium">{{ p.rank }}</td>
                <td class="px-4 py-2">
                  <router-link :to="`/stock/${p.code}`" class="text-accent hover:underline" @click.stop>
                    <span class="font-medium text-text-primary">{{ p.name || '名称待同步' }}</span>
                    <span class="ml-2 text-xs text-text-tertiary">{{ p.code }}</span>
                  </router-link>
                  <span
                    v-if="isNew(p)"
                    class="ml-1.5 rounded bg-down/10 px-1.5 py-0.5 text-xs font-medium text-down"
                  >新</span>
                </td>
                <td class="px-4 py-2 text-right font-medium">{{ p.score?.toFixed(4) ?? '--' }}</td>
                <td class="px-4 py-2 text-right" :class="(p.factors?.mom20 ?? 0) >= 0 ? 'text-up' : 'text-down'">
                  {{ factorText(p, 'mom20') }}
                </td>
                <td class="px-4 py-2 text-right" :class="(p.factors?.mom60 ?? 0) >= 0 ? 'text-up' : 'text-down'">
                  {{ factorText(p, 'mom60') }}
                </td>
                <td class="px-4 py-2 text-right">{{ factorText(p, 'rsi14') }}</td>
                <td class="px-4 py-2 text-right">{{ factorText(p, 'vol_ratio5') }}</td>
                <td class="px-4 py-2 text-right text-xs text-text-tertiary">
                  <button
                    type="button"
                    class="inline-flex items-center gap-1 rounded px-1.5 py-1 hover:bg-active hover:text-text-primary"
                    :aria-expanded="expanded === p.code"
                    :aria-controls="`pick-factors-${p.code}`"
                    @click="toggleExpand(p.code)"
                  >
                    {{ expanded === p.code ? '收起' : '查看指标' }}
                    <ChevronUp v-if="expanded === p.code" :size="14" />
                    <ChevronDown v-else :size="14" />
                  </button>
                </td>
              </tr>
              <tr v-if="expanded === p.code" :id="`pick-factors-${p.code}`" class="border-b border-border-subtle bg-hover/50">
                <td colspan="8" class="px-4 py-3">
                  <div class="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-7">
                    <div v-for="key in factorKeys" :key="key" class="p-2">
                      <div class="text-xs text-text-tertiary">{{ factorName(key) }}</div>
                      <div class="mt-0.5 text-sm font-medium">{{ factorText(p, key) }}</div>
                    </div>
                  </div>
                </td>
              </tr>
            </template>
          </tbody>
        </table>
        <p v-if="!items.length" class="px-4 py-6 text-center text-sm text-text-tertiary">{{ emptyText }}</p>
      </div>

      <section v-if="dropped.length">
        <h3 class="mb-2 text-base font-semibold text-text-secondary">调出({{ dropped.length }})</h3>
        <div class="flex flex-wrap gap-2">
          <router-link
            v-for="d in dropped"
            :key="droppedCode(d)"
            :to="`/stock/${droppedCode(d)}`"
            class="rounded-md border border-border px-2.5 py-1 text-xs text-text-tertiary line-through hover:bg-hover"
          >
            {{ droppedName(d) || droppedCode(d) }}
          </router-link>
        </div>
      </section>
    </template>
  </div>
</template>
