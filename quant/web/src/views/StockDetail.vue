<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import type { EChartsCoreOption } from 'echarts/core'
import { api, type KlineBar, type SignalItem } from '../api'
import { fmtBigAmount, fmtPct, fmtPrice, pnlClass } from '../format'
import EChart from '../components/EChart.vue'

const UP = '#d43a3a'
const DOWN = '#1a9e6b'

const route = useRoute()
const code = route.params.code as string

const bars = ref<KlineBar[]>([])
const signals = ref<SignalItem[]>([])
const stockName = ref('')
const industry = ref('')
const lastPrice = ref<number | null>(null)
const lastPct = ref<number | null>(null)
const loading = ref(true)
const error = ref('')
const backfilling = ref(false)
const backfillMsg = ref('')

const ranges = [
  { label: '近3月', days: 66 },
  { label: '近6月', days: 132 },
  { label: '近1年', days: 264 },
  { label: '全部', days: 0 },
]
const activeRange = ref(0) // index into ranges; 0=全部
const zoomStart = ref(0)

function ma(values: number[], window: number): (number | null)[] {
  return values.map((_, i) => {
    if (i < window - 1) return null
    let sum = 0
    for (let j = i - window + 1; j <= i; j++) sum += values[j]
    return +(sum / window).toFixed(3)
  })
}

const chartOption = computed<EChartsCoreOption>(() => {
  const dates = bars.value.map((b) => b.date)
  const kdata = bars.value.map((b) => [b.open, b.close, b.low, b.high])
  const closes = bars.value.map((b) => b.close)
  const vols = bars.value.map((b) => b.volume)
  const volColors = bars.value.map((b) => (b.close >= b.open ? UP : DOWN))

  const sigBuy = signals.value
    .filter((s) => s.side === 'buy')
    .map((s) => [s.date, s.price])
  const sigSell = signals.value
    .filter((s) => s.side === 'sell')
    .map((s) => [s.date, s.price])

  return {
    animation: false,
    legend: { data: ['5日平均线', '20日平均线'], top: 0, textStyle: { color: '#666' } },
    tooltip: { trigger: 'axis', axisPointer: { type: 'cross' } },
    axisPointer: { link: [{ xAxisIndex: 'all' }] },
    grid: [
      { left: 60, right: 20, top: 30, height: '58%' },
      { left: 60, right: 20, top: '72%', height: '16%' },
    ],
    xAxis: [
      { type: 'category', data: dates, gridIndex: 0, axisLabel: { show: false } },
      { type: 'category', data: dates, gridIndex: 1 },
    ],
    yAxis: [
      { scale: true, gridIndex: 0, splitLine: { lineStyle: { color: '#eee' } } },
      { scale: true, gridIndex: 1, splitLine: { show: false } },
    ],
    dataZoom: [
      { type: 'inside', xAxisIndex: [0, 1], start: zoomStart.value, end: 100 },
      { type: 'slider', xAxisIndex: [0, 1], start: zoomStart.value, end: 100, top: '92%' },
    ],
    series: [
      {
        name: '日K线',
        type: 'candlestick',
        data: kdata,
        itemStyle: {
          color: UP,
          color0: DOWN,
          borderColor: UP,
          borderColor0: DOWN,
        },
      },
      {
        name: '5日平均线',
        type: 'line',
        data: ma(closes, 5),
        smooth: true,
        showSymbol: false,
        lineStyle: { width: 1, color: '#f0a030' },
      },
      {
        name: '20日平均线',
        type: 'line',
        data: ma(closes, 20),
        smooth: true,
        showSymbol: false,
        lineStyle: { width: 1, color: '#4070d0' },
      },
      {
        name: '买入信号',
        type: 'scatter',
        data: sigBuy,
        symbol: 'triangle',
        symbolSize: 12,
        itemStyle: { color: UP },
        tooltip: { formatter: (p: unknown) => `买入信号 ${(p as { value: [string, number] }).value[0]}` },
      },
      {
        name: '卖出信号',
        type: 'scatter',
        data: sigSell,
        symbol: 'triangle',
        symbolRotate: 180,
        symbolSize: 12,
        itemStyle: { color: DOWN },
        tooltip: { formatter: (p: unknown) => `卖出信号 ${(p as { value: [string, number] }).value[0]}` },
      },
      {
        name: '成交量',
        type: 'bar',
        xAxisIndex: 1,
        yAxisIndex: 1,
        data: vols.map((v, i) => ({ value: v, itemStyle: { color: volColors[i] } })),
        tooltip: { valueFormatter: (v: unknown) => fmtBigAmount(v as number) },
      },
    ],
  }
})

async function loadAll() {
  loading.value = true
  error.value = ''
  try {
    const [k, sig, snap, watch] = await Promise.all([
      api.kline(code),
      api.signals({ code, limit: 500 }),
      api.snapshot(),
      api.watchlist(),
    ])
    bars.value = k.bars
    signals.value = sig.items
    const w = watch.items.find((i) => i.code === code)
    stockName.value = k.name || w?.name || ''
    industry.value = k.industry || w?.industry || ''
    const s = snap.items.find((i) => i.code === code)
    lastPrice.value = s?.price ?? (bars.value.length ? bars.value[bars.value.length - 1].close : null)
    lastPct.value = s?.pct_chg ?? null
    if (bars.value.length) {
      const last = bars.value[bars.value.length - 1]
      if (lastPct.value === null && bars.value.length > 1) {
        const prev = bars.value[bars.value.length - 2]
        lastPct.value = ((last.close - prev.close) / prev.close) * 100
      }
    }
    setRange(0)
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}

function setRange(idx: number) {
  activeRange.value = idx
  const days = ranges[idx].days
  zoomStart.value = days === 0 || bars.value.length <= days
    ? 0
    : ((bars.value.length - days) / bars.value.length) * 100
}

async function doBackfill() {
  backfilling.value = true
  backfillMsg.value = ''
  try {
    const r = await api.backfill(code)
    backfillMsg.value = `回填完成:新增/更新 ${r.bars} 根K线`
    await loadAll()
  } catch (e) {
    backfillMsg.value = `回填失败:${(e as Error).message}`
  } finally {
    backfilling.value = false
  }
}

onMounted(loadAll)
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-4">
      <h2 class="text-lg font-semibold">
        {{ stockName || '名称待同步' }}
        <span class="ml-2 text-sm font-normal text-text-tertiary">{{ code }}</span>
      </h2>
      <span v-if="industry" class="rounded bg-active px-2 py-1 text-xs text-text-secondary">{{ industry }}</span>
      <span v-if="lastPrice !== null" class="text-xl font-semibold" :class="pnlClass(lastPct)">
        {{ fmtPrice(lastPrice) }}
      </span>
      <span v-if="lastPct !== null" class="text-sm" :class="pnlClass(lastPct)">
        {{ fmtPct(lastPct / 100) }}
      </span>
      <div class="ml-auto flex items-center gap-2">
        <button
          v-for="(r, i) in ranges"
          :key="r.label"
          class="rounded-md border px-3 py-1 text-sm"
          :class="i === activeRange
            ? 'border-accent bg-active text-text-primary'
            : 'border-border text-text-secondary hover:bg-hover'"
          @click="setRange(i)"
        >
          {{ r.label }}
        </button>
        <button
          class="rounded-md border border-border px-3 py-1 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
          :disabled="backfilling"
          @click="doBackfill"
        >
          {{ backfilling ? '回填中(可能需要十几秒)…' : '回填历史数据' }}
        </button>
      </div>
    </div>

    <p v-if="backfillMsg" class="text-sm text-text-secondary">{{ backfillMsg }}</p>
    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <div v-else-if="bars.length" class="rounded-lg border border-border bg-surface-raised p-2">
      <EChart :option="chartOption" height="560px" />
    </div>
    <p v-else class="text-sm text-text-tertiary">暂无K线数据,可点击"回填历史数据"</p>
  </div>
</template>
