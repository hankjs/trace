<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { api, type PortfolioSummary, type Trade } from '../api'
import { fmtAmount, fmtPrice, fmtQty, fmtSigned, pnlClass } from '../format'

const summary = ref<PortfolioSummary | null>(null)
const trades = ref<Trade[]>([])
const nameMap = ref<Record<string, string>>({})
const loading = ref(true)
const error = ref('')
const formError = ref('')
const submitting = ref(false)

const form = reactive({
  code: '',
  trade_date: new Date().toISOString().slice(0, 10),
  side: 'buy',
  price: '',
  qty: '',
  fee: '',
  note: '',
})

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [pos, tr, watch] = await Promise.all([
      api.positions(),
      api.trades(),
      api.watchlist(),
    ])
    summary.value = pos
    trades.value = tr.items
    nameMap.value = Object.fromEntries(watch.items.map((i) => [i.code, i.name]))
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
    <h2 class="text-lg font-semibold">持仓</h2>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <template v-else-if="summary">
      <section class="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div class="rounded-lg border border-border bg-surface-raised p-4">
          <div class="text-xs text-text-tertiary">总市值</div>
          <div class="mt-1 text-xl font-semibold">{{ fmtAmount(summary.total_market_value) }}</div>
        </div>
        <div class="rounded-lg border border-border bg-surface-raised p-4">
          <div class="text-xs text-text-tertiary">总浮动盈亏</div>
          <div class="mt-1 text-xl font-semibold" :class="pnlClass(summary.total_unrealized_pnl)">
            {{ fmtSigned(summary.total_unrealized_pnl) }}
          </div>
        </div>
        <div class="rounded-lg border border-border bg-surface-raised p-4">
          <div class="text-xs text-text-tertiary">总已实现盈亏</div>
          <div class="mt-1 text-xl font-semibold" :class="pnlClass(summary.total_realized_pnl)">
            {{ fmtSigned(summary.total_realized_pnl) }}
          </div>
        </div>
      </section>

      <section>
        <h3 class="mb-2 text-base font-semibold">当前持仓</h3>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs text-text-tertiary">
                <th class="px-4 py-2 font-medium">代码</th>
                <th class="px-4 py-2 font-medium">名称</th>
                <th class="px-4 py-2 text-right font-medium">数量</th>
                <th class="px-4 py-2 text-right font-medium">成本价</th>
                <th class="px-4 py-2 text-right font-medium">最新价</th>
                <th class="px-4 py-2 text-right font-medium">市值</th>
                <th class="px-4 py-2 text-right font-medium">浮动盈亏</th>
                <th class="px-4 py-2 text-right font-medium">已实现盈亏</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="p in summary.positions"
                :key="p.code"
                class="border-b border-border-subtle last:border-0 hover:bg-hover"
              >
                <td class="px-4 py-2">
                  <router-link :to="`/stock/${p.code}`" class="text-accent hover:underline">{{ p.code }}</router-link>
                </td>
                <td class="px-4 py-2">{{ nameMap[p.code] ?? '' }}</td>
                <td class="px-4 py-2 text-right">{{ fmtQty(p.qty) }}</td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(p.avg_cost) }}</td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(p.last_price) }}</td>
                <td class="px-4 py-2 text-right">{{ fmtAmount(p.market_value) }}</td>
                <td class="px-4 py-2 text-right" :class="pnlClass(p.unrealized_pnl)">{{ fmtSigned(p.unrealized_pnl) }}</td>
                <td class="px-4 py-2 text-right" :class="pnlClass(p.realized_pnl)">{{ fmtSigned(p.realized_pnl) }}</td>
              </tr>
            </tbody>
          </table>
          <p v-if="!summary.positions.length" class="px-4 py-6 text-center text-sm text-text-tertiary">暂无持仓</p>
        </div>
      </section>

      <section>
        <h3 class="mb-2 text-base font-semibold">记账</h3>
        <form class="flex flex-wrap items-end gap-3 rounded-lg border border-border bg-surface-raised p-4" @submit.prevent="submit">
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">代码</span>
            <input v-model="form.code" required placeholder="sh.600519" class="w-32 rounded-md border border-border px-2 py-1.5" />
          </label>
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
          <button type="submit" :disabled="submitting" class="rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover disabled:opacity-50">
            {{ submitting ? '提交中…' : '添加' }}
          </button>
          <p v-if="formError" class="w-full text-sm text-up">{{ formError }}</p>
        </form>
      </section>

      <section>
        <h3 class="mb-2 text-base font-semibold">成交记录</h3>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs text-text-tertiary">
                <th class="px-4 py-2 font-medium">ID</th>
                <th class="px-4 py-2 font-medium">日期</th>
                <th class="px-4 py-2 font-medium">代码</th>
                <th class="px-4 py-2 font-medium">方向</th>
                <th class="px-4 py-2 text-right font-medium">价格</th>
                <th class="px-4 py-2 text-right font-medium">数量</th>
                <th class="px-4 py-2 text-right font-medium">费用</th>
                <th class="px-4 py-2 font-medium">备注</th>
                <th class="px-4 py-2 font-medium"></th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="t in trades"
                :key="t.id"
                class="border-b border-border-subtle last:border-0 hover:bg-hover"
              >
                <td class="px-4 py-2 text-text-tertiary">{{ t.id }}</td>
                <td class="px-4 py-2">{{ t.trade_date }}</td>
                <td class="px-4 py-2">{{ t.code }}</td>
                <td class="px-4 py-2">
                  <span :class="t.side === 'buy' ? 'text-up' : 'text-down'" class="font-medium">
                    {{ t.side === 'buy' ? '买入' : '卖出' }}
                  </span>
                </td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(t.price) }}</td>
                <td class="px-4 py-2 text-right">{{ fmtQty(t.qty) }}</td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(t.fee) }}</td>
                <td class="px-4 py-2 text-text-secondary">{{ t.note }}</td>
                <td class="px-4 py-2 text-right">
                  <button class="text-xs text-text-tertiary hover:text-up" @click="removeTrade(t.id)">删除</button>
                </td>
              </tr>
            </tbody>
          </table>
          <p v-if="!trades.length" class="px-4 py-6 text-center text-sm text-text-tertiary">暂无成交记录</p>
        </div>
      </section>
    </template>
  </div>
</template>
