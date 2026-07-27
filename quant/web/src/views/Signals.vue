<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { Search } from 'lucide-vue-next'
import { api, type SignalItem, type WatchItem } from '../api'
import { loadCatalog, reasonText, signalName, templateName } from '../catalog'
import LoadingRows from '../components/LoadingRows.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import PageHeader from '../components/PageHeader.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import StockSearchInput from '../components/StockSearchInput.vue'
import StrategySelect from '../components/StrategySelect.vue'
import { fmtPrice } from '../format'

const items = ref<SignalItem[]>([])
const watchMap = ref<Record<string, WatchItem>>({})
const loading = ref(true)
const error = ref('')
const fDate = ref('')
const fCode = ref('')
/** null = 全部策略 */
const fStrategyId = ref<number | null>(null)
const fSide = ref('')

const signalColumns: QuTableColumn<SignalItem>[] = [
  { key: 'date', label: '日期', cellClass: 'whitespace-nowrap text-text-secondary' },
  { key: 'stock', label: '股票' },
  { key: 'strategy', label: '策略' },
  { key: 'side', label: '提示' },
  { key: 'price', label: '参考价格', align: 'right', cellClass: 'tabular-nums' },
  { key: 'reason', label: '为什么出现', cellClass: 'max-w-md text-xs leading-5 text-text-secondary' },
]

async function load() {
  loading.value = true
  error.value = ''
  try {
    const result = await api.signals({
      date: fDate.value || undefined,
      code: fCode.value.trim() || undefined,
      strategy_id: fStrategyId.value ?? undefined,
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

/** 展示策略实例名(用户自定义);算法模板名作为副标题 */
function strategyLabel(signal: SignalItem): string {
  return signal.strategy_name || `策略 ${signal.strategy_id}`
}

function templateLabel(signal: SignalItem): string {
  return signal.template ? templateName(signal.template) : ''
}

function sideLabel(signal: SignalItem): string {
  return signal.side_name || signalName(signal.side)
}

onMounted(async () => {
  await loadCatalog()
  try {
    const watch = await api.watchlist()
    watchMap.value = Object.fromEntries(watch.items.map((item) => [item.code, item]))
  } catch {
    watchMap.value = {}
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
      <StrategySelect v-model="fStrategyId" allow-empty :manage-link="false" />
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

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="5" />

    <div v-else-if="items.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
      <QuTable
        :data="items"
        :columns="signalColumns"
        row-key="id"
        class="min-w-[860px]"
        header-cell-class="px-4 py-2.5 font-medium"
        body-cell-class="px-4 py-3"
      >
        <template #cell-stock="{ row: signal }">
          <router-link :to="`/stock/${signal.code}`" class="font-medium hover:text-accent">{{ nameOf(signal) }}</router-link>
          <div class="mt-0.5 text-xs text-text-tertiary">{{ signal.code }}</div>
        </template>
        <template #cell-strategy="{ row: signal }">
          <div class="flex items-center gap-1.5">
            <span>{{ strategyLabel(signal) }}</span>
            <span v-if="signal.is_system === false" class="rounded bg-active px-1.5 py-0.5 text-[11px] text-accent">自定义</span>
          </div>
          <div class="mt-0.5 text-[11px] text-text-tertiary">{{ templateLabel(signal) }}</div>
        </template>
        <template #cell-side="{ row: signal }">
          <span class="inline-flex items-center gap-1.5 whitespace-nowrap rounded-md px-2 py-1 text-xs font-medium" :class="sideClass(signal.side)">
            <span class="h-1.5 w-1.5 rounded-full bg-current" />
            {{ sideLabel(signal) }}
          </span>
        </template>
        <template #cell-price="{ row: signal }">{{ fmtPrice(signal.price) }}</template>
        <template #cell-reason="{ row: signal }">{{ reasonText(signal.reason, signal.reason_text) }}</template>
      </QuTable>
    </div>

    <div v-else class="rounded-md border border-dashed border-border px-5 py-10 text-center">
      <p class="text-sm font-medium">没有匹配的策略提示</p>
      <p class="mt-1 text-xs text-text-tertiary">可清除日期、股票或策略条件后重新查询。</p>
    </div>
  </div>
</template>
