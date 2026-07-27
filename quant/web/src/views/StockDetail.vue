<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import type { EChartsCoreOption } from 'echarts/core'
import { api, type KlineBar, type ResearchCondition, type ResearchPlanSummary, type ResearchPriceReference, type SignalItem } from '../api'
import { fmtBigAmount, fmtPct, fmtPrice, pnlClass } from '../format'
import EChart from '../components/EChart.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import ResearchPlanSummaryView from '../components/ResearchPlanSummary.vue'
import { priceReferenceText, researchPlanStatusName } from '../researchPlans'

const UP = '#d43a3a'
const DOWN = '#1a9e6b'

const route = useRoute()
const code = route.params.code as string

const bars = ref<KlineBar[]>([])
const signals = ref<SignalItem[]>([])
const plans = ref<ResearchPlanSummary[]>([])
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

const latestPlan = computed(() => [...plans.value].sort((a, b) =>
  `${b.data_date}-${b.generated_at ?? ''}`.localeCompare(`${a.data_date}-${a.generated_at ?? ''}`)
)[0] ?? null)

const researchReferences = computed<ResearchPriceReference[]>(() => {
  const plan = latestPlan.value
  if (!plan) return []
  const refs: ResearchPriceReference[] = []
  if (plan.entry?.line) refs.push(plan.entry.line)
  if (plan.entry?.range) refs.push(plan.entry.range)
  for (const rule of [
    ...(plan.risk_rules ?? []),
    ...(plan.take_profit_rules ?? []),
    ...(plan.native_exit_rules ?? []),
  ]) {
    if (rule.price_reference) refs.push(rule.price_reference)
  }
  return refs
})

const dynamicRules = computed<ResearchCondition[]>(() => {
  const plan = latestPlan.value
  if (!plan) return []
  return [
    ...(plan.entry?.conditions ?? []),
    ...(plan.risk_rules ?? []),
    ...(plan.take_profit_rules ?? []),
    ...(plan.native_exit_rules ?? []),
  ].filter((rule) => !rule.price_reference)
})

function referenceColor(source: ResearchPriceReference['source']): string {
  if (source === 'risk_overlay' || source === 'native_risk') return '#b4533c'
  if (source === 'take_profit') return '#9a6a22'
  if (source === 'native_exit') return '#6d5c92'
  return '#277f90'
}

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

  const markLines = researchReferences.value.flatMap((reference) => {
    const values = reference.value != null
      ? [reference.value]
      : [reference.lower, reference.upper].filter((value): value is number => value != null)
    return values.map((value, index) => ({
      name: `${reference.name}${values.length > 1 ? (index === 0 ? '下界' : '上界') : ''} ${value.toFixed(2)} · ${reference.data_date}`,
      yAxis: value,
      lineStyle: { color: referenceColor(reference.source), type: index === 0 ? 'solid' : 'dashed', width: 1 },
      label: { color: referenceColor(reference.source), fontSize: 10, position: 'insideEndTop' },
    }))
  })
  const markAreas = researchReferences.value
    .filter((reference) => reference.lower != null && reference.upper != null)
    .map((reference) => [
      { name: `${reference.name} ${reference.data_date}`, yAxis: reference.lower, itemStyle: { color: 'rgba(39,127,144,0.08)' } },
      { yAxis: reference.upper },
    ])

  return {
    animation: false,
    legend: { data: ['5日平均线', '20日平均线'], top: 0 },
    tooltip: { trigger: 'axis', axisPointer: { type: 'cross' } },
    axisPointer: { link: [{ xAxisIndex: 'all' }] },
    grid: [
      { left: 88, right: 20, top: 30, height: '58%' },
      { left: 88, right: 20, top: '72%', height: '16%' },
    ],
    xAxis: [
      { type: 'category', data: dates, gridIndex: 0, axisLabel: { show: false } },
      { type: 'category', data: dates, gridIndex: 1 },
    ],
    yAxis: [
      { scale: true, gridIndex: 0 },
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
        name: '入场提示',
        type: 'scatter',
        data: sigBuy,
        symbol: 'triangle',
        symbolSize: 12,
        itemStyle: { color: UP },
        tooltip: { formatter: (p: unknown) => `入场提示 ${(p as { value: [string, number] }).value[0]}` },
      },
      {
        name: '退出提示',
        type: 'scatter',
        data: sigSell,
        symbol: 'triangle',
        symbolRotate: 180,
        symbolSize: 12,
        itemStyle: { color: DOWN },
        tooltip: { formatter: (p: unknown) => `退出提示 ${(p as { value: [string, number] }).value[0]}` },
      },
      {
        name: '研究参考线',
        type: 'line',
        data: [],
        showSymbol: false,
        markLine: { symbol: ['none', 'none'], silent: true, data: markLines },
        markArea: { silent: true, data: markAreas },
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
    const [k, sig, snap, watch, planResult] = await Promise.all([
      api.kline(code),
      api.signals({ code, limit: 500 }),
      api.snapshot(),
      api.watchlist(),
      api.stockResearchPlans(code).catch(() => null),
    ])
    bars.value = k.bars
    signals.value = sig.items
    const inlinePlans = sig.items
      .map((signal) => signal.research_plan ?? signal.plan_summary)
      .filter((plan): plan is ResearchPlanSummary => Boolean(plan))
    const summaryPlans = [...(planResult?.items ?? []), ...inlinePlans]
    const fullPlans = await Promise.all(summaryPlans
      .filter((plan) => plan.id > 0)
      .slice(0, 20)
      .map((plan) => api.researchPlan(plan.id).catch(() => plan)))
    const mergedPlans = [...fullPlans, ...summaryPlans]
    plans.value = [...new Map(mergedPlans.map((plan) => [plan.id, plan])).values()]
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
      <div v-if="latestPlan" class="min-w-0 basis-full text-sm sm:basis-auto" aria-live="polite">
        <span class="inline-flex rounded bg-surface-muted px-2 py-1 text-xs font-medium">
          计划状态：{{ latestPlan.status_name || researchPlanStatusName(latestPlan.status) }}
        </span>
        <p v-if="latestPlan.status_reason" class="mt-1 max-w-[65ch] text-xs leading-5" :class="latestPlan.status === 'needs_review' ? 'font-medium text-warning' : 'text-text-secondary'">
          {{ latestPlan.status === 'needs_review' ? '需要重新评估：' : '状态原因：' }}{{ latestPlan.status_reason }}
        </p>
      </div>
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
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="6" />

    <section v-else-if="bars.length" class="rounded-md border border-border bg-surface-raised p-2" aria-label="日 K 线与研究参考">
      <EChart :option="chartOption" height="560px" />
      <div v-if="researchReferences.length" class="border-t border-border-subtle px-2 py-3">
        <h3 class="text-xs font-semibold">图中研究参考</h3>
        <div class="mt-2 overflow-x-auto">
          <table class="min-w-[42rem] w-full text-xs">
            <thead class="text-left text-text-tertiary">
              <tr><th class="pb-1 font-medium">名称</th><th class="pb-1 font-medium">数值或区间</th><th class="pb-1 font-medium">数据日期</th><th class="pb-1 font-medium">计算依据</th></tr>
            </thead>
            <tbody>
              <tr v-for="reference in researchReferences" :key="reference.id ?? `${reference.source}-${reference.name}-${reference.data_date}`" class="border-t border-border-subtle">
                <td class="py-2 font-medium">{{ reference.name }}</td>
                <td class="py-2 tabular-nums">{{ priceReferenceText(reference) }}</td>
                <td class="py-2 text-text-secondary">{{ reference.data_date }}</td>
                <td class="py-2 text-text-secondary">{{ reference.calculation || '按策略规则计算' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </section>
    <p v-else class="text-sm text-text-tertiary">暂无K线数据,可点击"回填历史数据"</p>

    <section v-if="dynamicRules.length" aria-labelledby="dynamic-rules-heading">
      <h3 id="dynamic-rules-heading" class="mb-2 text-sm font-semibold">动态研究规则</h3>
      <div class="overflow-x-auto rounded-md border border-border bg-surface-raised">
        <table class="min-w-[48rem] w-full text-sm">
          <thead class="border-b border-border bg-surface-muted text-left text-xs text-text-tertiary">
            <tr><th class="px-4 py-2 font-medium">条件</th><th class="px-4 py-2 font-medium">当前状态</th><th class="px-4 py-2 font-medium">当前值 / 阈值</th><th class="px-4 py-2 font-medium">数据日期</th></tr>
          </thead>
          <tbody>
            <tr v-for="rule in dynamicRules" :key="rule.id ?? `${rule.source}-${rule.name}`" class="border-b border-border-subtle last:border-0">
              <td class="px-4 py-2"><span class="font-medium">{{ rule.name }}</span><p class="mt-0.5 text-xs text-text-secondary">{{ rule.summary }}</p></td>
              <td class="px-4 py-2">{{ rule.status || (rule.triggered ? '已命中' : '未命中') }}</td>
              <td class="px-4 py-2 tabular-nums">{{ rule.current_value ?? '数据不足' }}{{ rule.unit ?? '' }}<span v-if="rule.threshold != null" class="text-text-tertiary"> / 阈值 {{ rule.threshold }}{{ rule.unit ?? '' }}</span></td>
              <td class="px-4 py-2 text-text-secondary">{{ rule.data_date || latestPlan?.data_date }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <ResearchPlanSummaryView v-if="latestPlan" :plan="latestPlan" />
  </div>
</template>
