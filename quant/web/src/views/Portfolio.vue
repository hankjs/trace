<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { api, type PortfolioSummary, type Position, type ResearchPlanSummary, type Trade } from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import StockSearchInput from '../components/StockSearchInput.vue'
import PortfolioResearchPlan from '../components/PortfolioResearchPlan.vue'
import { fmtAmount, fmtPrice, fmtQty, fmtSigned, localDateISO, pnlClass } from '../format'

const summary = ref<PortfolioSummary | null>(null)
const trades = ref<Trade[]>([])
const nameMap = ref<Record<string, string>>({})
const loading = ref(true)
const error = ref('')
const formError = ref('')
const submitting = ref(false)
const researchPlans = ref<ResearchPlanSummary[]>([])
const latestRebalancePlan = computed(() => [...researchPlans.value].sort((a, b) =>
  `${b.data_date}-${b.generated_at ?? ''}`.localeCompare(`${a.data_date}-${a.generated_at ?? ''}`)
)[0] ?? null)

const positionColumns: QuTableColumn<Position>[] = [
  { key: 'stock', label: '股票' },
  { key: 'qty', label: '数量', align: 'right' },
  { key: 'avg-cost', label: '成本价', align: 'right' },
  { key: 'last-price', label: '最新价', align: 'right' },
  { key: 'market-value', label: '市值', align: 'right' },
  { key: 'unrealized-pnl', label: '浮动盈亏', align: 'right', cellClass: (position) => pnlClass(position.unrealized_pnl) },
  { key: 'realized-pnl', label: '已实现盈亏', align: 'right', cellClass: (position) => pnlClass(position.realized_pnl) },
]

const tradeColumns: QuTableColumn<Trade>[] = [
  { key: 'id', label: 'ID', cellClass: 'text-text-tertiary' },
  { key: 'trade_date', label: '日期' },
  { key: 'stock', label: '股票' },
  { key: 'side', label: '方向' },
  { key: 'price', label: '价格', align: 'right' },
  { key: 'qty', label: '数量', align: 'right' },
  { key: 'fee', label: '费用', align: 'right' },
  { key: 'note', label: '备注', cellClass: 'text-text-secondary' },
  { key: 'actions', label: '', align: 'right' },
]

const form = reactive({
  code: '',
  trade_date: localDateISO(),
  side: 'buy',
  price: '',
  qty: '',
  fee: '',
  note: '',
})

function nameOf(code: string, name?: string): string {
  return name || nameMap.value[code] || '名称待同步'
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [pos, tr, watch, planResult] = await Promise.all([
      api.positions(),
      api.trades(),
      api.watchlist(),
      api.portfolioResearchPlans({ limit: 5 }).catch(() => null),
    ])
    summary.value = pos
    trades.value = tr.items
    nameMap.value = Object.fromEntries(watch.items.map((i) => [i.code, i.name]))
    const summaries = planResult?.items ?? []
    researchPlans.value = await Promise.all(summaries.slice(0, 5).map((plan) =>
      plan.id > 0 ? api.researchPlan(plan.id).catch(() => plan) : Promise.resolve(plan)
    ))
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}

async function submit() {
  formError.value = ''
  if (!form.code.trim()) {
    formError.value = '请填写股票代码'
    return
  }
  submitting.value = true
  try {
    await api.addTrade({
      code: form.code.trim(),
      trade_date: form.trade_date,
      side: form.side,
      price: Number(form.price),
      qty: Number(form.qty),
      fee: form.fee ? Number(form.fee) : 0,
      note: form.note,
    })
    form.price = ''
    form.qty = ''
    form.fee = ''
    form.note = ''
    await load()
  } catch (e) {
    formError.value = (e as Error).message
  } finally {
    submitting.value = false
  }
}

async function removeTrade(id: number) {
  if (!window.confirm(`确认删除成交记录 #${id}?`)) return
  try {
    await api.deleteTrade(id)
    await load()
  } catch (e) {
    error.value = (e as Error).message
  }
}

onMounted(load)
</script>

<template>
  <div class="space-y-6">
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="5" />

    <template v-else-if="summary">
      <PortfolioResearchPlan v-if="latestRebalancePlan" :plan="latestRebalancePlan" />

      <section class="grid overflow-hidden rounded-md border border-border bg-surface-raised sm:grid-cols-3">
        <div class="border-b border-border-subtle p-4 sm:border-b-0 sm:border-r">
          <div class="text-xs text-text-tertiary">总市值</div>
          <div class="mt-1 text-lg font-semibold">{{ fmtAmount(summary.total_market_value) }}</div>
        </div>
        <div class="border-b border-border-subtle p-4 sm:border-b-0 sm:border-r">
          <div class="text-xs text-text-tertiary">总浮动盈亏</div>
          <div class="mt-1 text-lg font-semibold" :class="pnlClass(summary.total_unrealized_pnl)">
            {{ fmtSigned(summary.total_unrealized_pnl) }}
          </div>
        </div>
        <div class="p-4">
          <div class="text-xs text-text-tertiary">总已实现盈亏</div>
          <div class="mt-1 text-lg font-semibold" :class="pnlClass(summary.total_realized_pnl)">
            {{ fmtSigned(summary.total_realized_pnl) }}
          </div>
        </div>
      </section>

      <section>
        <h3 class="mb-2 text-base font-semibold">当前持仓</h3>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <QuTable :data="summary.positions" :columns="positionColumns" row-key="code">
            <template #cell-stock="{ row: position }">
              <router-link :to="`/stock/${position.code}`" class="font-medium hover:text-accent">{{ nameOf(position.code, position.name) }}</router-link>
              <div class="text-xs text-text-tertiary">{{ position.code }}</div>
            </template>
            <template #cell-qty="{ row: position }">{{ fmtQty(position.qty) }}</template>
            <template #cell-avg-cost="{ row: position }">{{ fmtPrice(position.avg_cost) }}</template>
            <template #cell-last-price="{ row: position }">{{ fmtPrice(position.last_price) }}</template>
            <template #cell-market-value="{ row: position }">{{ fmtAmount(position.market_value) }}</template>
            <template #cell-unrealized-pnl="{ row: position }">{{ fmtSigned(position.unrealized_pnl) }}</template>
            <template #cell-realized-pnl="{ row: position }">{{ fmtSigned(position.realized_pnl) }}</template>
          </QuTable>
          <p v-if="!summary.positions.length" class="px-4 py-6 text-center text-sm text-text-tertiary">暂无持仓</p>
        </div>
      </section>

      <section>
        <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
          <h3 class="text-base font-semibold">手工记账</h3>
          <span class="text-xs text-text-tertiary">这里只记录已完成的成交，不会提交订单</span>
        </div>
        <form class="flex flex-wrap items-end gap-3 rounded-lg border border-border bg-surface-raised p-4" @submit.prevent="submit">
          <StockSearchInput v-model="form.code" label="股票" required />
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">日期</span>
            <input v-model="form.trade_date" required type="date" class="rounded-md border border-border px-2 py-1.5" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">方向</span>
            <select v-model="form.side" class="rounded-md border border-border px-2 py-1.5">
              <option value="buy">买入</option>
              <option value="sell">卖出</option>
            </select>
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">价格</span>
            <input v-model="form.price" required type="number" step="0.01" min="0.01" class="w-28 rounded-md border border-border px-2 py-1.5" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">数量</span>
            <input v-model="form.qty" required type="number" step="100" min="1" class="w-28 rounded-md border border-border px-2 py-1.5" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">费用</span>
            <input v-model="form.fee" type="number" step="0.01" min="0" class="w-24 rounded-md border border-border px-2 py-1.5" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">备注</span>
            <input v-model="form.note" class="w-40 rounded-md border border-border px-2 py-1.5" />
          </label>
          <button type="submit" :disabled="submitting" class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50">
            {{ submitting ? '保存中…' : '保存记录' }}
          </button>
          <InlineFeedback v-if="formError" tone="error" class="w-full">{{ formError }}</InlineFeedback>
        </form>
      </section>

      <section>
        <h3 class="mb-2 text-base font-semibold">成交记录</h3>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <QuTable :data="trades" :columns="tradeColumns" row-key="id">
            <template #cell-stock="{ row: trade }">
              <span class="font-medium">{{ nameOf(trade.code, trade.name) }}</span>
              <div class="text-xs text-text-tertiary">{{ trade.code }}</div>
            </template>
            <template #cell-side="{ row: trade }">
              <span :class="trade.side === 'buy' ? 'text-up' : 'text-down'" class="font-medium">
                {{ trade.side === 'buy' ? '买入' : '卖出' }}
              </span>
            </template>
            <template #cell-price="{ row: trade }">{{ fmtPrice(trade.price) }}</template>
            <template #cell-qty="{ row: trade }">{{ fmtQty(trade.qty) }}</template>
            <template #cell-fee="{ row: trade }">{{ fmtPrice(trade.fee) }}</template>
            <template #cell-actions="{ row: trade }">
              <button class="text-xs text-text-tertiary hover:text-up" @click="removeTrade(trade.id)">删除</button>
            </template>
          </QuTable>
          <p v-if="!trades.length" class="px-4 py-6 text-center text-sm text-text-tertiary">暂无成交记录</p>
        </div>
      </section>
    </template>
  </div>
</template>
