<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { api, type SignalItem, type WatchItem } from '../api'
import { fmtPrice } from '../format'

const items = ref<SignalItem[]>([])
const watchMap = ref<Record<string, WatchItem>>({})
const strategies = ref<string[]>([])
const loading = ref(true)
const error = ref('')

const fDate = ref('')
const fCode = ref('')
const fStrategy = ref('')
const fSide = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    const r = await api.signals({
      date: fDate.value || undefined,
      code: fCode.value.trim() || undefined,
      strategy: fStrategy.value || undefined,
      side: fSide.value || undefined,
      limit: 200,
    })
    items.value = r.items
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}

function nameOf(code: string): string {
  return watchMap.value[code]?.name ?? ''
}

function sideLabel(side: SignalItem['side']): string {
  if (side === 'buy') return '买入'
  if (side === 'sell') return '卖出'
  return '观察'
}

function sideClass(side: SignalItem['side']): string {
  if (side === 'buy') return 'text-up'
  if (side === 'sell') return 'text-down'
  return 'text-text-secondary'
}

function reasonText(reason: Record<string, unknown>): string {
  return Object.entries(reason)
    .map(([k, v]) => {
      const s = typeof v === 'number' ? +v.toFixed(4)
        : v !== null && typeof v === 'object' ? JSON.stringify(v)
        : String(v)
      return `${k}: ${s}`
    })
    .join(', ')
}

onMounted(async () => {
  try {
    const [watch, strat] = await Promise.all([api.watchlist(), api.strategies()])
    watchMap.value = Object.fromEntries(watch.items.map((i) => [i.code, i]))
    strategies.value = strat.strategies
  } catch {
    /* 名称/策略列表加载失败不阻塞信号查询 */
  }
  await load()
})
</script>

<template>
  <div class="space-y-4">
    <h2 class="text-lg font-semibold">信号</h2>

    <form class="flex flex-wrap items-end gap-3" @submit.prevent="load">
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">日期</span>
        <input v-model="fDate" type="date" class="rounded-md border border-border bg-surface-raised px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">股票代码</span>
        <input v-model="fCode" type="text" placeholder="sh.600519" class="w-32 rounded-md border border-border bg-surface-raised px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">策略</span>
        <select v-model="fStrategy" class="rounded-md border border-border bg-surface-raised px-2 py-1.5">
          <option value="">全部</option>
          <option v-for="s in strategies" :key="s" :value="s">{{ s }}</option>
        </select>
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">方向</span>
        <select v-model="fSide" class="rounded-md border border-border bg-surface-raised px-2 py-1.5">
          <option value="">全部</option>
          <option value="buy">买入</option>
          <option value="sell">卖出</option>
          <option value="watch">观察</option>
        </select>
      </label>
      <button type="submit" class="rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover">
        查询
      </button>
    </form>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <div v-else class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border text-left text-xs text-text-tertiary">
            <th class="px-4 py-2 font-medium">日期</th>
            <th class="px-4 py-2 font-medium">代码</th>
            <th class="px-4 py-2 font-medium">名称</th>
            <th class="px-4 py-2 font-medium">策略</th>
            <th class="px-4 py-2 font-medium">方向</th>
            <th class="px-4 py-2 text-right font-medium">价格</th>
            <th class="px-4 py-2 font-medium">原因</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="sig in items"
            :key="sig.id"
            class="border-b border-border-subtle last:border-0 hover:bg-hover"
          >
            <td class="px-4 py-2 whitespace-nowrap">{{ sig.date }}</td>
            <td class="px-4 py-2">
              <router-link :to="`/stock/${sig.code}`" class="text-accent hover:underline">
                {{ sig.code }}
              </router-link>
            </td>
            <td class="px-4 py-2">{{ nameOf(sig.code) }}</td>
            <td class="px-4 py-2">{{ sig.strategy }}</td>
            <td class="px-4 py-2">
              <span :class="sideClass(sig.side)" class="font-medium">
                {{ sideLabel(sig.side) }}
              </span>
            </td>
            <td class="px-4 py-2 text-right">{{ fmtPrice(sig.price) }}</td>
            <td class="max-w-md truncate px-4 py-2 text-text-secondary" :title="reasonText(sig.reason)">
              {{ reasonText(sig.reason) }}
            </td>
          </tr>
        </tbody>
      </table>
      <p v-if="!items.length" class="px-4 py-6 text-center text-sm text-text-tertiary">无匹配信号</p>
    </div>
  </div>
</template>
