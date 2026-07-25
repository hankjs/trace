<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { Search } from 'lucide-vue-next'
import { api, type SignalItem, type WatchItem } from '../api'
import { loadCatalog, reasonText, signalName, strategyName } from '../catalog'
import LoadingRows from '../components/LoadingRows.vue'
import PageHeader from '../components/PageHeader.vue'
import StockSearchInput from '../components/StockSearchInput.vue'
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
    const result = await api.signals({
      date: fDate.value || undefined,
      code: fCode.value.trim() || undefined,
      strategy: fStrategy.value || undefined,
      side: fSide.value || undefined,
      limit: 200,
    })
    items.value = result.items
  } catch (caught) {
    items.value = []
    error.value = (caught as Error).message
  } finally {
    loading.value = false
  }
}

function nameOf(signal: SignalItem): string {
  return signal.name || watchMap.value[signal.code]?.name || '名称待同步'
}

function sideClass(side: SignalItem['side']): string {
  if (side === 'buy') return 'bg-up/10 text-up'
  if (side === 'sell') return 'bg-down/10 text-down'
  return 'bg-active text-text-secondary'
}

function strategyLabel(signal: SignalItem): string {
  return signal.strategy_name || strategyName(signal.strategy)
}

function sideLabel(signal: SignalItem): string {
  return signal.side_name || signalName(signal.side)
}

onMounted(async () => {
  await loadCatalog()
  try {
    const [watch, strategyResult] = await Promise.all([api.watchlist(), api.strategies()])
    watchMap.value = Object.fromEntries(watch.items.map((item) => [item.code, item]))
    strategies.value = strategyResult.strategies
  } catch {
    strategies.value = []
  }
  await load()
})
</script>

<template>
  <div class="space-y-5">
    <PageHeader title="信号提醒" description="查看策略在日线数据上发生的状态变化，并阅读产生提示的原因。" />

    <form class="flex flex-wrap items-end gap-3 border-b border-border-subtle pb-4" @submit.prevent="load">
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">日期</span>
        <input v-model="fDate" type="date" class="rounded-md border border-border px-2 py-1.5" />
      </label>
      <StockSearchInput v-model="fCode" label="股票" />
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">策略</span>
        <select v-model="fStrategy" class="max-w-56 rounded-md border border-border px-2 py-1.5">
          <option value="">全部策略</option>
          <option v-for="strategy in strategies" :key="strategy" :value="strategy">{{ strategyName(strategy) }}</option>
        </select>
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">提示类型</span>
        <select v-model="fSide" class="rounded-md border border-border px-2 py-1.5">
          <option value="">全部类型</option>
          <option value="buy">满足入场规则</option>
          <option value="sell">满足退出规则</option>
          <option value="watch">继续观察</option>
        </select>
      </label>
      <button type="submit" :disabled="loading" class="inline-flex h-9 items-center gap-1.5 rounded-md bg-accent px-4 text-sm font-medium text-on-accent hover:bg-accent-hover disabled:opacity-50">
        <Search :size="16" /> 查询
      </button>
    </form>

    <p v-if="error" role="alert" class="rounded-md border border-up/30 bg-danger-soft px-4 py-2 text-sm text-up">{{ error }}</p>
    <LoadingRows v-if="loading" :rows="5" />

    <div v-else-if="items.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
      <table class="w-full min-w-[860px] text-sm">
        <thead>
          <tr class="border-b border-border text-left text-xs text-text-tertiary">
            <th class="px-4 py-2.5 font-medium">日期</th>
            <th class="px-4 py-2.5 font-medium">股票</th>
            <th class="px-4 py-2.5 font-medium">策略</th>
            <th class="px-4 py-2.5 font-medium">提示</th>
            <th class="px-4 py-2.5 text-right font-medium">参考价格</th>
            <th class="px-4 py-2.5 font-medium">为什么出现</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="signal in items" :key="signal.id" class="border-b border-border-subtle last:border-0 hover:bg-hover">
            <td class="whitespace-nowrap px-4 py-3 text-text-secondary">{{ signal.date }}</td>
            <td class="px-4 py-3">
              <router-link :to="`/stock/${signal.code}`" class="font-medium hover:text-accent">{{ nameOf(signal) }}</router-link>
              <div class="mt-0.5 text-xs text-text-tertiary">{{ signal.code }}</div>
            </td>
            <td class="px-4 py-3">
              <div>{{ strategyLabel(signal) }}</div>
              <code class="text-[11px] text-text-tertiary">{{ signal.strategy }}</code>
            </td>
            <td class="px-4 py-3">
              <span class="inline-flex items-center gap-1.5 whitespace-nowrap rounded-md px-2 py-1 text-xs font-medium" :class="sideClass(signal.side)">
                <span class="h-1.5 w-1.5 rounded-full bg-current" />
                {{ sideLabel(signal) }}
              </span>
            </td>
            <td class="px-4 py-3 text-right tabular-nums">{{ fmtPrice(signal.price) }}</td>
            <td class="max-w-md px-4 py-3 text-xs leading-5 text-text-secondary">{{ reasonText(signal.reason, signal.reason_text) }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-else class="rounded-md border border-dashed border-border px-5 py-10 text-center">
      <p class="text-sm font-medium">没有匹配的策略提示</p>
      <p class="mt-1 text-xs text-text-tertiary">可清除日期、股票或策略条件后重新查询。</p>
    </div>
  </div>
</template>
