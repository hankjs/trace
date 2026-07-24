<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api, type PickItem } from '../api'
import { fmtBigAmount, fmtPct } from '../format'

const date = ref(new Date().toISOString().slice(0, 10))
const items = ref<PickItem[]>([])
const dropped = ref<(PickItem | string)[]>([])
const loading = ref(true)
const error = ref('')
const expanded = ref<string | null>(null)

const factorLabels: [string, string][] = [
  ['mom20', '20日动量'],
  ['mom60', '60日动量'],
  ['rsi14', 'RSI14'],
  ['atr_pct', 'ATR%'],
  ['vol_ratio5', '量比5日'],
  ['ma20_slope', 'MA20斜率'],
  ['amount_avg20', '20日日均成交额'],
]

/** 新进标记:兼容 change='new' / is_new 两种契约 */
function isNew(p: PickItem): boolean {
  return p.change === 'new' || p.is_new === true
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
  if (key === 'amount_avg20') return fmtBigAmount(v)
  if (key === 'rsi14' || key === 'vol_ratio5') return v.toFixed(2)
  return fmtPct(v)
}

function toggleExpand(code: string) {
  expanded.value = expanded.value === code ? null : code
}

const emptyText = computed(() =>
  date.value >= new Date().toISOString().slice(0, 10)
    ? '今日还未生成选股池(交易日 17:00 生成)'
    : '该日期无选股池数据'
)

async function load() {
  loading.value = true
  error.value = ''
  expanded.value = null
  try {
    const r = await api.picks(date.value || undefined)
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

onMounted(load)
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-end justify-between gap-3">
      <h2 class="text-lg font-semibold">选股池</h2>
      <form class="flex items-end gap-3" @submit.prevent="load">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">日期</span>
          <input v-model="date" type="date" class="rounded-md border border-border bg-surface-raised px-2 py-1.5" />
        </label>
        <button type="submit" class="rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover">
          查询
        </button>
      </form>
    </div>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <template v-else>
      <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-xs text-text-tertiary">
              <th class="px-4 py-2 font-medium">排名</th>
              <th class="px-4 py-2 font-medium">代码</th>
              <th class="px-4 py-2 font-medium">名称</th>
              <th class="px-4 py-2 text-right font-medium">评分</th>
              <th class="px-4 py-2 text-right font-medium">20日动量</th>
              <th class="px-4 py-2 text-right font-medium">60日动量</th>
              <th class="px-4 py-2 text-right font-medium">RSI14</th>
              <th class="px-4 py-2 text-right font-medium">量比</th>
              <th class="px-4 py-2 font-medium"></th>
            </tr>
          </thead>
          <tbody>
            <template v-for="p in items" :key="p.code">
              <tr
                class="cursor-pointer border-b border-border-subtle hover:bg-hover"
                @click="toggleExpand(p.code)"
              >
                <td class="px-4 py-2 font-medium">{{ p.rank }}</td>
                <td class="px-4 py-2">
                  <router-link :to="`/stock/${p.code}`" class="text-accent hover:underline" @click.stop>
                    {{ p.code }}
                  </router-link>
                </td>
                <td class="px-4 py-2">
                  {{ p.name }}
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
                  {{ expanded === p.code ? '收起 ▲' : '因子 ▼' }}
                </td>
              </tr>
              <tr v-if="expanded === p.code" class="border-b border-border-subtle bg-hover/50">
                <td colspan="9" class="px-4 py-3">
                  <div class="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-7">
                    <div v-for="[key, label] in factorLabels" :key="key" class="rounded-md border border-border bg-surface-raised p-2">
                      <div class="text-xs text-text-tertiary">{{ label }}</div>
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
