<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import type { EChartsCoreOption } from 'echarts/core'
import { api, type BacktestResult, type WatchItem } from '../api'
import { fmtPct, fmtPrice } from '../format'
import EChart from '../components/EChart.vue'

const route = useRoute()

const strategies = ref<string[]>([])
const watchlist = ref<WatchItem[]>([])
const result = ref<BacktestResult | null>(null)
const running = ref(false)
const error = ref('')
const runIdInput = ref('')

const form = reactive({
  strategy: '',
  codes: [] as string[],
  codesText: '',
  start: '',
  end: '',
})

const chartOption = computed<EChartsCoreOption>(() => {
  const eq = result.value?.equity ?? []
  return {
    animation: false,
    tooltip: { trigger: 'axis' },
    grid: { left: 70, right: 20, top: 20, bottom: 60 },
    xAxis: { type: 'category', data: eq.map((e) => e.date) },
    yAxis: { scale: true, splitLine: { lineStyle: { color: '#eee' } } },
    dataZoom: [
      { type: 'inside' },
      { type: 'slider' },
    ],
    series: [
      {
        name: '净值',
        type: 'line',
        data: eq.map((e) => e.equity),
        showSymbol: false,
        lineStyle: { width: 1.5, color: '#4070d0' },
        areaStyle: { color: 'rgba(64,112,208,0.08)' },
      },
    ],
  }
})

const metrics = computed(() => {
  const m = result.value?.metrics
  if (!m) return []
  return [
    { label: '总收益', value: fmtPct(m.total_return), cls: m.total_return >= 0 ? 'text-up' : 'text-down' },
    { label: '年化收益', value: fmtPct(m.annual_return), cls: m.annual_return >= 0 ? 'text-up' : 'text-down' },
    { label: '最大回撤', value: fmtPct(m.max_drawdown), cls: 'text-down' },
    { label: '胜率', value: fmtPct(m.win_rate), cls: 'text-text-primary' },
    { label: '交易次数', value: String(m.trade_count), cls: 'text-text-primary' },
    { label: '完整回合', value: String(m.round_trips), cls: 'text-text-primary' },
  ]
})

function toggleCode(code: string) {
  const i = form.codes.indexOf(code)
  if (i >= 0) form.codes.splice(i, 1)
  else form.codes.push(code)
}

function parsedCodes(): string[] {
  const extra = form.codesText
    .split(/[,,\s]+/)
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean)
  return [...new Set([...form.codes, ...extra])]
}

async function run() {
  error.value = ''
  const codes = parsedCodes()
  if (!form.strategy) {
    error.value = '请选择策略'
    return
  }
  if (!codes.length) {
    error.value = '请至少选择一个股票代码'
    return
  }
  if (!form.start || !form.end) {
    error.value = '请选择起止日期'
    return
  }
  running.value = true
  try {
    result.value = await api.runBacktest({
      strategy: form.strategy,
      codes,
      start: form.start,
      end: form.end,
    })
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    running.value = false
  }
}

async function loadRun() {
  error.value = ''
  const id = Number(runIdInput.value)
  if (!id) {
    error.value = '请输入有效的 run_id'
    return
  }
  running.value = true
  try {
    result.value = await api.getBacktest(id)
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    running.value = false
  }
}

onMounted(async () => {
  try {
    const [s, w] = await Promise.all([api.strategies(), api.watchlist()])
    strategies.value = s.strategies
    watchlist.value = w.items
    if (s.strategies.length) form.strategy = s.strategies[0]
  } catch (e) {
    error.value = (e as Error).message
  }
  // 支持 /backtest?run=1 直接查看历史回测
  const q = Number(route.query.run)
  if (q) {
    runIdInput.value = String(q)
    await loadRun()
  }
})
</script>

<template>
  <div class="space-y-6">
    <h2 class="text-lg font-semibold">回测</h2>

    <form class="space-y-3 rounded-lg border border-border bg-surface-raised p-4" @submit.prevent="run">
      <div class="flex flex-wrap items-end gap-3">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">策略</span>
          <select v-model="form.strategy" class="rounded-md border border-border px-2 py-1.5">
            <option v-for="s in strategies" :key="s" :value="s">{{ s }}</option>
          </select>
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">开始日期</span>
          <input v-model="form.start" type="date" class="rounded-md border border-border px-2 py-1.5" />
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">结束日期</span>
          <input v-model="form.end" type="date" class="rounded-md border border-border px-2 py-1.5" />
        </label>
        <button type="submit" :disabled="running" class="rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover disabled:opacity-50">
          {{ running ? '运行中…' : '运行回测' }}
        </button>
      </div>

      <div>
        <span class="mb-1 block text-xs text-text-tertiary">股票(点击选择自选股,或逗号分隔输入)</span>
        <div class="mb-2 flex flex-wrap gap-2">
          <button
            v-for="w in watchlist"
            :key="w.code"
            type="button"
            class="rounded-md border px-2.5 py-1 text-xs"
            :class="form.codes.includes(w.code)
              ? 'border-accent bg-active text-text-primary'
              : 'border-border text-text-secondary hover:bg-hover'"
            @click="toggleCode(w.code)"
          >
            {{ w.name || w.code }}
          </button>
        </div>
        <input
          v-model="form.codesText"
          placeholder="sh.600519, sz.000001"
          class="w-full rounded-md border border-border px-2 py-1.5 text-sm sm:w-96"
        />
      </div>
    </form>

    <div class="flex items-end gap-3 rounded-lg border border-border bg-surface-raised p-4">
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">查询历史回测 run_id</span>
        <input v-model="runIdInput" type="number" min="1" class="w-32 rounded-md border border-border px-2 py-1.5" />
      </label>
      <button :disabled="running" class="rounded-md border border-border px-4 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50" @click="loadRun">
        查询
      </button>
    </div>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>

    <template v-if="result">
      <div class="flex items-center gap-3 text-sm text-text-secondary">
        <span>run_id: <span class="font-medium text-text-primary">{{ result.run_id }}</span></span>
        <template v-if="result.strategy">
          <span>策略: {{ result.strategy }}</span>
          <span>{{ result.start }} ~ {{ result.end }}</span>
        </template>
      </div>

      <section class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
        <div v-for="m in metrics" :key="m.label" class="rounded-lg border border-border bg-surface-raised p-3">
          <div class="text-xs text-text-tertiary">{{ m.label }}</div>
          <div class="mt-1 text-lg font-semibold" :class="m.cls">{{ m.value }}</div>
        </div>
      </section>

      <section class="rounded-lg border border-border bg-surface-raised p-2">
        <EChart :option="chartOption" height="380px" />
      </section>

      <section v-if="result.metrics.per_code && Object.keys(result.metrics.per_code).length">
        <h3 class="mb-2 text-base font-semibold">分股票明细</h3>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs text-text-tertiary">
                <th class="px-4 py-2 font-medium">代码</th>
                <th class="px-4 py-2 text-right font-medium">总收益</th>
                <th class="px-4 py-2 text-right font-medium">交易次数</th>
                <th class="px-4 py-2 text-right font-medium">回合</th>
                <th class="px-4 py-2 text-right font-medium">胜率</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="(v, c) in result.metrics.per_code"
                :key="c"
                class="border-b border-border-subtle last:border-0 hover:bg-hover"
              >
                <td class="px-4 py-2">{{ c }}</td>
                <td class="px-4 py-2 text-right" :class="Number((v as Record<string, number>).total_return) >= 0 ? 'text-up' : 'text-down'">
                  {{ fmtPct(Number((v as Record<string, number>).total_return ?? 0)) }}
                </td>
                <td class="px-4 py-2 text-right">{{ (v as Record<string, number>).trade_count ?? '--' }}</td>
                <td class="px-4 py-2 text-right">{{ (v as Record<string, number>).round_trips ?? '--' }}</td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(Number((v as Record<string, number>).win_rate ?? 0) * 100) }}%</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </template>
  </div>
</template>
