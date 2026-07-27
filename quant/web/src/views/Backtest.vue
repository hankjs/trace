<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { AlertTriangle } from 'lucide-vue-next'
import type { EChartsCoreOption } from 'echarts/core'
import { api, hasSurvivorshipBias, type BacktestResult, type SweepResult, type SweepResultItem, type WatchItem } from '../api'
import { catalogEntry, loadCatalog, metricName, templateName } from '../catalog'
import { aggregateMetric, fmtPct, fmtPrice } from '../format'
import EChart from '../components/EChart.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import PoolSelect from '../components/PoolSelect.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import StockSearchInput from '../components/StockSearchInput.vue'
import StrategySelect from '../components/StrategySelect.vue'
import StrategyParamFields from '../components/StrategyParamFields.vue'
import { poolById } from '../pools'
import { invalidateStrategies, strategyById, useStrategies } from '../strategies'
import { useStrategyParamForm } from '../useStrategyParamForm'

const route = useRoute()

const { load: loadStrategies } = useStrategies()
const watchlist = ref<WatchItem[]>([])
const result = ref<BacktestResult | null>(null)
const running = ref(false)
const error = ref('')
const notice = ref('')
const runIdInput = ref('')
const searchCode = ref('')
/** 组合策略的研究范围;单标的策略不使用 */
const poolId = ref<number | null>(null)
/** 选中的策略实例 id;参数元数据来自它的算法模板 */
const strategyId = ref<number | null>(null)
const saveAsName = ref('')

/** 模式:single 单次回测 / sweep 参数扫描 */
const mode = ref<'single' | 'sweep'>('single')

const form = reactive({
  codes: [] as string[],
  codesText: '',
  start: '',
  end: '',
})

const strategy = computed(() => strategyById(strategyId.value))
/** 算法模板的元数据:说明、限制与参数定义 */
const templateMeta = computed(() =>
  strategy.value ? catalogEntry('strategy_templates', strategy.value.template) : undefined
)
const strategyParams = computed(() => templateMeta.value?.params ?? [])
const isPortfolio = computed(() => strategy.value?.kind === 'portfolio')
const parameterForm = useStrategyParamForm(strategyParams)
const parameterValues = parameterForm.values

/** 与策略自身参数是否有差异:有差异才是「临时调参」,才值得提示另存 */
const paramsTweaked = computed(() => parameterForm.differsFrom(strategy.value?.effective_params ?? {}))

watch([strategy, strategyParams], () => {
  parameterForm.reset(strategy.value?.effective_params ?? {})
  notice.value = ''
  saveAsName.value = ''
  if (isPortfolio.value && mode.value === 'sweep') mode.value = 'single'
})

/** 把临时调好的参数另存为一条新策略,不改动原策略(公共策略本就只读) */
async function saveTweakedAsStrategy() {
  const source = strategy.value
  if (!source) return
  if (!parameterForm.validate()) {
    error.value = '请修正策略参数后再另存'
    return
  }
  running.value = true
  error.value = ''
  try {
    const created = await api.duplicateStrategy(source.id, {
      name: saveAsName.value.trim() || undefined,
      params: parameterForm.overrides.value,
    })
    invalidateStrategies()
    await loadStrategies(true)
    strategyId.value = created.id
    saveAsName.value = ''
    notice.value = `已另存为「${created.name}」，之后可直接选用。`
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    running.value = false
  }
}

// ---- 参数扫描 ----

interface GridRow {
  name: string
  valuesText: string
}

const gridRows = ref<GridRow[]>([{ name: '', valuesText: '' }])
const sweepResult = ref<SweepResult | null>(null)

function addGridRow() {
  gridRows.value.push({ name: '', valuesText: '' })
}

function removeGridRow(i: number) {
  gridRows.value.splice(i, 1)
}

function parseGrid(): Record<string, number[]> {
  const grid: Record<string, number[]> = {}
  for (const row of gridRows.value) {
    const name = row.name.trim()
    if (!name) continue
    const values = row.valuesText
      .split(/[,,\s]+/)
      .map((s) => Number(s.trim()))
      .filter((v) => !Number.isNaN(v))
    if (values.length) grid[name] = [...new Set(values)]
  }
  return grid
}

/** 扫描结果按总收益降序 */
const sweepRows = computed(() => {
  const rows = [...(sweepResult.value?.results ?? [])]
  rows.sort(
    (a, b) => (aggregateMetric(b.metrics, 'total_return') ?? -Infinity) - (aggregateMetric(a.metrics, 'total_return') ?? -Infinity)
  )
  return rows
})

const bestKey = computed(() => {
  const best = sweepRows.value[0]
  return best ? JSON.stringify(best.params) : ''
})

const sweepColumns = computed<QuTableColumn<SweepResultItem>[]>(() => [
  { key: 'rank', label: '#', cellClass: 'text-text-tertiary' },
  { key: 'params', label: '参数', cellClass: 'font-medium' },
  { key: 'total-return', label: metricName('total_return'), align: 'right', cellClass: (row) => (aggregateMetric(row.metrics, 'total_return') ?? 0) >= 0 ? 'text-up' : 'text-down' },
  { key: 'annual-return', label: metricName('annual_return'), align: 'right', cellClass: (row) => (aggregateMetric(row.metrics, 'annual_return') ?? 0) >= 0 ? 'text-up' : 'text-down' },
  { key: 'max-drawdown', label: metricName('max_drawdown'), align: 'right', cellClass: 'text-down' },
  { key: 'sharpe', label: metricName('sharpe'), align: 'right' },
  { key: 'win-rate', label: metricName('win_rate'), align: 'right' },
  { key: 'trade-count', label: metricName('trade_count'), align: 'right' },
])

interface PerCodeRow {
  code: string
  metrics: Record<string, number>
}

const perCodeRows = computed<PerCodeRow[]>(() => Object.entries(result.value?.metrics.per_code ?? {}).map(([code, metrics]) => ({
  code,
  metrics: metrics as Record<string, number>,
})))
const perCodeColumns = computed<QuTableColumn<PerCodeRow>[]>(() => [
  { key: 'stock', label: '股票' },
  { key: 'total-return', label: metricName('total_return'), align: 'right', cellClass: (row) => Number(row.metrics.total_return) >= 0 ? 'text-up' : 'text-down' },
  { key: 'trade-count', label: metricName('trade_count'), align: 'right' },
  { key: 'round-trips', label: metricName('round_trips'), align: 'right' },
  { key: 'win-rate', label: metricName('win_rate'), align: 'right' },
])

function paramsText(params: Record<string, number>): string {
  return Object.entries(params)
    .map(([k, v]) => `${strategyParams.value.find((item) => item.key === k)?.name ?? k}=${v}`)
    .join(', ')
}

/** 恰好两个参数时画热力图:两参数为轴,总收益为值 */
const heatmapOption = computed<EChartsCoreOption | null>(() => {
  const rows = sweepResult.value?.results ?? []
  if (!rows.length) return null
  const names = [...new Set(rows.flatMap((r) => Object.keys(r.params)))]
  if (names.length !== 2) return null
  const [xName, yName] = names
  const xVals = [...new Set(rows.map((r) => r.params[xName]))].sort((a, b) => a - b)
  const yVals = [...new Set(rows.map((r) => r.params[yName]))].sort((a, b) => a - b)
  const data: [number, number, number][] = []
  for (const r of rows) {
    const v = aggregateMetric(r.metrics, 'total_return')
    if (v === undefined) continue
    data.push([xVals.indexOf(r.params[xName]), yVals.indexOf(r.params[yName]), +v.toFixed(4)])
  }
  if (!data.length) return null
  const values = data.map((d) => d[2])
  return {
    animation: false,
    tooltip: {
      formatter: (p: { data: [number, number, number] }) =>
        `${xName}=${xVals[p.data[0]]}, ${yName}=${yVals[p.data[1]]}<br/>总收益: ${fmtPct(p.data[2])}`,
    },
    grid: { left: 90, right: 90, top: 30, bottom: 50 },
    xAxis: { type: 'category', name: xName, data: xVals.map(String) },
    yAxis: { type: 'category', name: yName, data: yVals.map(String) },
    visualMap: {
      min: Math.min(...values),
      max: Math.max(...values),
      calculable: true,
      orient: 'vertical',
      right: 0,
      top: 'center',
      formatter: (v: number) => fmtPct(v),
      /* 涨红跌绿:高收益偏红,低收益偏绿 */
      inRange: { color: ['#3a9a78', '#f2e8c9', '#d04050'] },
    },
    series: [
      {
        type: 'heatmap',
        data,
        label: {
          show: true,
          formatter: (p: { data: [number, number, number] }) => fmtPct(p.data[2]),
          fontSize: 11,
        },
        emphasis: { itemStyle: { shadowBlur: 8, shadowColor: 'rgba(0,0,0,0.3)' } },
      },
    ],
  }
})

async function runSweep() {
  error.value = ''
  const codes = parsedCodes()
  if (strategyId.value === null) {
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
  const param_grid = parseGrid()
  if (!Object.keys(param_grid).length) {
    error.value = '请至少配置一个参数网格(参数名 + 逗号分隔的候选值)'
    return
  }
  running.value = true
  try {
    sweepResult.value = await api.sweepBacktest({
      strategy_id: strategyId.value,
      codes,
      start: form.start,
      end: form.end,
      param_grid,
    })
  } catch (e) {
    sweepResult.value = null
    error.value = (e as Error).message
  } finally {
    running.value = false
  }
}

// ---- 单次回测(原有逻辑) ----

const chartOption = computed<EChartsCoreOption>(() => {
  const eq = result.value?.equity ?? []
  return {
    animation: false,
    tooltip: { trigger: 'axis' },
    grid: { left: 70, right: 20, top: 20, bottom: 60 },
    xAxis: { type: 'category', data: eq.map((e) => e.date) },
    yAxis: { scale: true },
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
    { label: metricName('total_return'), value: fmtPct(m.total_return), cls: m.total_return >= 0 ? 'text-up' : 'text-down' },
    { label: metricName('annual_return'), value: fmtPct(m.annual_return), cls: m.annual_return >= 0 ? 'text-up' : 'text-down' },
    { label: metricName('max_drawdown'), value: fmtPct(m.max_drawdown), cls: 'text-down' },
    { label: metricName('win_rate'), value: fmtPct(m.win_rate), cls: 'text-text-primary' },
    { label: metricName('trade_count'), value: String(m.trade_count), cls: 'text-text-primary' },
    { label: metricName('round_trips'), value: String(m.round_trips), cls: 'text-text-primary' },
  ]
})

/**
 * 回测结果所用池是否为静态池。优先用后端回显的 pool(查询历史回测时本地没有选择状态),
 * 回退到当前选择。预置池(index/all)按逐日成分解析,不标注。
 */
const resultPool = computed(() => result.value?.pool ?? (isPortfolio.value ? poolById(poolId.value) : null))
const resultBiased = computed(() => hasSurvivorshipBias(resultPool.value))

function toggleCode(code: string) {
  const i = form.codes.indexOf(code)
  if (i >= 0) form.codes.splice(i, 1)
  else form.codes.push(code)
}

function addSearchCode() {
  const code = searchCode.value.trim().toLowerCase()
  if (code && !form.codes.includes(code)) form.codes.push(code)
  searchCode.value = ''
}

function stockName(code: string): string {
  return result.value?.stocks?.find((item) => item.code === code)?.name
    || sweepResult.value?.stocks?.find((item) => item.code === code)?.name
    || watchlist.value.find((item) => item.code === code)?.name
    || '名称待同步'
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
  notice.value = ''
  const codes = parsedCodes()
  if (strategyId.value === null) {
    error.value = '请选择策略'
    return
  }
  if (!codes.length && !isPortfolio.value) {
    error.value = '请至少选择一个股票代码'
    return
  }
  if (!form.start || !form.end) {
    error.value = '请选择起止日期'
    return
  }
  if (!parameterForm.validate()) {
    error.value = '请修正策略参数后再运行回测'
    return
  }
  running.value = true
  try {
    result.value = await api.runBacktest({
      strategy_id: strategyId.value,
      codes,
      start: form.start,
      end: form.end,
      // 组合策略按股票池解析成分(取代旧的「codes 留空隐式动态池」约定)
      ...(isPortfolio.value && poolId.value !== null ? { pool_id: poolId.value } : {}),
      params: parameterForm.snapshot(),
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
    error.value = '请输入有效的回测编号'
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
  await loadCatalog()
  try {
    // 策略列表由 StrategySelect 自行加载,这里只补自选股;
    // 但仍等一次 load,避免 strategyId 落位前先渲染出空参数表单
    const [, w] = await Promise.all([loadStrategies(), api.watchlist()])
    watchlist.value = w.items
    parameterForm.reset(strategy.value?.effective_params ?? {})
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
    <div class="flex items-center gap-4">
      <h2 class="text-base font-semibold">历史回测</h2>
      <div class="flex rounded-md border border-border text-sm">
        <button
          class="rounded-l-md px-3 py-1"
          :class="mode === 'single' ? 'bg-active font-medium text-text-primary' : 'text-text-secondary hover:bg-hover'"
          @click="mode = 'single'"
        >
          单次回测
        </button>
        <button
          class="rounded-r-md px-3 py-1 disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="isPortfolio"
          :class="mode === 'sweep' ? 'bg-active font-medium text-text-primary' : 'text-text-secondary hover:bg-hover'"
          @click="mode = 'sweep'"
        >
          参数扫描
        </button>
      </div>
    </div>

    <form
      class="space-y-3 rounded-lg border border-border bg-surface-raised p-4"
      @submit.prevent="mode === 'single' ? run() : runSweep()"
    >
      <div class="flex flex-wrap items-end gap-3">
        <StrategySelect v-model="strategyId" />
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">开始日期</span>
          <input v-model="form.start" type="date" class="rounded-md border border-border px-2 py-1.5" />
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">结束日期</span>
          <input v-model="form.end" type="date" class="rounded-md border border-border px-2 py-1.5" />
        </label>
        <button type="submit" :disabled="running" class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50">
          {{ running ? '运行中…' : mode === 'single' ? '运行回测' : '开始扫描' }}
        </button>
      </div>

      <div v-if="templateMeta" class="max-w-3xl text-xs leading-5 text-text-secondary">
        <p>{{ templateMeta.description }}</p>
        <p v-if="templateMeta.caveat" class="mt-1 text-text-tertiary">限制：{{ templateMeta.caveat }}</p>
      </div>

      <div v-if="mode === 'single' && strategyParams.length" class="border-t border-border-subtle pt-3">
        <span class="mb-2 block text-xs font-medium text-text-secondary">
          策略参数
          <span class="ml-1 font-normal text-text-tertiary">（此处调参只作用于本次回测，不改动策略本身）</span>
        </span>
        <StrategyParamFields
          v-model="parameterValues"
          :parameters="strategyParams"
          :errors="parameterForm.errors"
          id-prefix="backtest-param"
        />

        <!-- 临时调参后可固化为一条新策略,原策略(尤其公共策略)保持不变 -->
        <div v-if="paramsTweaked" class="mt-3 flex flex-wrap items-end gap-2">
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">新策略名称（留空自动加「副本」）</span>
            <input
              v-model="saveAsName"
              placeholder="如 双均线（快5慢30）"
              class="w-56 rounded-md border border-border px-2 py-1.5 text-sm"
            />
          </label>
          <button
            type="button"
            :disabled="running"
            class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
            @click="saveTweakedAsStrategy"
          >
            另存为我的策略
          </button>
        </div>
      </div>

      <div v-if="isPortfolio" class="border-t border-border-subtle pt-3">
        <PoolSelect v-model="poolId" label="股票池（组合策略的选股范围）" />
      </div>

      <div>
        <span class="mb-1 block text-xs text-text-tertiary">
          {{ isPortfolio ? '股票（留空则使用所选股票池在区间内的动态成分）' : '股票（点击选择自选股，或逗号分隔输入）' }}
        </span>
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
            {{ w.name || '名称待同步' }} · {{ w.code }}
          </button>
        </div>
        <div class="mb-2 flex items-end gap-2">
          <StockSearchInput v-model="searchCode" label="搜索其他股票" />
          <button type="button" :disabled="!searchCode" class="h-9 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover disabled:opacity-40" @click="addSearchCode">添加</button>
        </div>
        <input
          v-model="form.codesText"
          placeholder="sh.600519, sz.000001"
          class="w-full rounded-md border border-border px-2 py-1.5 text-sm sm:w-96"
        />
      </div>

      <div v-if="mode === 'sweep'">
        <div class="mb-1 flex items-center justify-between">
          <span class="text-xs text-text-tertiary">参数网格(参数名 + 逗号分隔候选值,如 fast: 3,5,8)</span>
          <button type="button" class="text-xs text-accent hover:underline" @click="addGridRow">+ 添加参数</button>
        </div>
        <div class="space-y-2">
          <div v-for="(row, i) in gridRows" :key="i" class="flex items-center gap-2">
            <select
              v-if="strategyParams.length"
              v-model="row.name"
              class="w-48 rounded-md border border-border px-2 py-1.5 text-sm"
            >
              <option value="">选择参数</option>
              <option v-for="parameter in strategyParams" :key="parameter.key" :value="parameter.key">{{ parameter.name }}</option>
            </select>
            <input
              v-else
              v-model="row.name"
              placeholder="参数名,如 fast"
              class="w-40 rounded-md border border-border px-2 py-1.5 text-sm"
            />
            <input
              v-model="row.valuesText"
              placeholder="候选值,如 3,5,8"
              class="w-64 rounded-md border border-border px-2 py-1.5 text-sm"
            />
            <button
              v-if="gridRows.length > 1"
              type="button"
              class="text-xs text-text-tertiary hover:text-up"
              @click="removeGridRow(i)"
            >
              删除
            </button>
          </div>
        </div>
      </div>
    </form>

    <div v-if="mode === 'single'" class="flex items-end gap-3 rounded-lg border border-border bg-surface-raised p-4">
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">查询历史回测编号</span>
        <input v-model="runIdInput" type="number" min="1" class="w-32 rounded-md border border-border px-2 py-1.5" />
      </label>
      <button :disabled="running" class="rounded-md border border-border px-4 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50" @click="loadRun">
        查询
      </button>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice">{{ notice }}</InlineFeedback>

    <!-- 参数扫描结果 -->
    <template v-if="mode === 'sweep' && sweepResult">
      <div class="flex items-center gap-3 text-sm text-text-secondary">
        <span>
          策略: <span class="font-medium text-text-primary">{{ sweepResult.strategy_name }}</span>
          <span class="ml-1 text-xs text-text-tertiary">（{{ templateName(sweepResult.template) }}）</span>
        </span>
        <span>{{ sweepResult.start }} ~ {{ sweepResult.end }}</span>
        <span>{{ sweepRows.length }} 组参数组合</span>
      </div>

      <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
        <QuTable
          :data="sweepRows"
          :columns="sweepColumns"
          :row-key="(row) => JSON.stringify(row.params)"
          :body-row-class="(row) => `border-b border-border-subtle last:border-0 hover:bg-hover ${JSON.stringify(row.params) === bestKey ? 'bg-down/5' : ''}`"
        >
          <template #cell-rank="{ row, rowIndex }">
            {{ rowIndex + 1 }}
            <span
              v-if="JSON.stringify(row.params) === bestKey"
              class="ml-1 rounded bg-down/10 px-1.5 py-0.5 text-xs font-medium text-down"
            >最优</span>
          </template>
          <template #cell-params="{ row }">{{ paramsText(row.params) }}</template>
          <template #cell-total-return="{ row }">{{ aggregateMetric(row.metrics, 'total_return') !== undefined ? fmtPct(aggregateMetric(row.metrics, 'total_return')) : '--' }}</template>
          <template #cell-annual-return="{ row }">{{ aggregateMetric(row.metrics, 'annual_return') !== undefined ? fmtPct(aggregateMetric(row.metrics, 'annual_return')) : '--' }}</template>
          <template #cell-max-drawdown="{ row }">{{ aggregateMetric(row.metrics, 'max_drawdown') !== undefined ? fmtPct(aggregateMetric(row.metrics, 'max_drawdown')) : '--' }}</template>
          <template #cell-sharpe="{ row }">{{ aggregateMetric(row.metrics, 'sharpe') !== undefined ? fmtPrice(aggregateMetric(row.metrics, 'sharpe')) : '--' }}</template>
          <template #cell-win-rate="{ row }">{{ aggregateMetric(row.metrics, 'win_rate') !== undefined ? fmtPct(aggregateMetric(row.metrics, 'win_rate')) : '--' }}</template>
          <template #cell-trade-count="{ row }">{{ aggregateMetric(row.metrics, 'trade_count') ?? '--' }}</template>
        </QuTable>
        <p v-if="!sweepRows.length" class="px-4 py-6 text-center text-sm text-text-tertiary">无扫描结果</p>
      </div>

      <section v-if="heatmapOption" class="rounded-lg border border-border bg-surface-raised p-2">
        <h3 class="px-2 pt-2 text-base font-semibold">参数热力图(总收益)</h3>
        <EChart :option="heatmapOption" height="380px" />
      </section>
    </template>

    <!-- 单次回测结果 -->
    <template v-if="mode === 'single' && result">
      <div class="flex items-center gap-3 text-sm text-text-secondary">
        <span>回测编号：<span class="font-medium text-text-primary">{{ result.run_id }}</span></span>
        <template v-if="result.strategy_id">
          <!-- 策略行可能已被删除,后端此时回显 strategy_name = null -->
          <span>策略: {{ result.strategy_name ?? '策略已删除' }}</span>
          <span v-if="result.template" class="text-xs text-text-tertiary">{{ templateName(result.template) }}</span>
          <span>{{ result.start }} ~ {{ result.end }}</span>
        </template>
        <span v-if="resultPool">股票池: {{ resultPool.name }}</span>
      </div>

      <!-- 静态池无成员历史,历史区间结果含幸存者偏差;预置池逐日解析成分,不标注 -->
      <p
        v-if="resultBiased"
        class="flex items-start gap-2 rounded-md border border-warning/30 bg-warning-soft px-4 py-3 text-sm leading-6 text-text-secondary"
      >
        <AlertTriangle :size="16" class="mt-0.5 shrink-0 text-warning" />
        <span>
          本次回测使用自定义静态股票池<template v-if="resultPool">「{{ resultPool.name }}」</template>，
          该池只保存当前成员名单、不含成员变动历史，等于用今天的名单回溯过去，
          已退市或期间被移出的股票不在样本内，结果存在<strong class="font-medium text-text-primary">幸存者偏差</strong>，
          收益通常偏乐观。需要严格历史口径时请改用预置池（全部A股 / 指数成分）重跑。
        </span>
      </p>

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
          <QuTable :data="perCodeRows" :columns="perCodeColumns" row-key="code">
            <template #cell-stock="{ row }">
              <div class="font-medium">{{ stockName(row.code) }}</div>
              <div class="text-xs text-text-tertiary">{{ row.code }}</div>
            </template>
            <template #cell-total-return="{ row }">{{ fmtPct(Number(row.metrics.total_return ?? 0)) }}</template>
            <template #cell-trade-count="{ row }">{{ row.metrics.trade_count ?? '--' }}</template>
            <template #cell-round-trips="{ row }">{{ row.metrics.round_trips ?? '--' }}</template>
            <template #cell-win-rate="{ row }">{{ fmtPrice(Number(row.metrics.win_rate ?? 0) * 100) }}%</template>
          </QuTable>
        </div>
      </section>
    </template>
  </div>
</template>
