<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import type { EChartsCoreOption } from 'echarts/core'
import { api, type BacktestResult, type SweepResult, type SweepMetrics, type WatchItem } from '../api'
import { fmtPct, fmtPrice } from '../format'
import EChart from '../components/EChart.vue'

const route = useRoute()

const strategies = ref<string[]>([])
const watchlist = ref<WatchItem[]>([])
const result = ref<BacktestResult | null>(null)
const running = ref(false)
const error = ref('')
const runIdInput = ref('')

/** 模式:single 单次回测 / sweep 参数扫描 */
const mode = ref<'single' | 'sweep'>('single')

const form = reactive({
  strategy: '',
  codes: [] as string[],
  codesText: '',
  start: '',
  end: '',
})

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
    (a, b) => (metricOf(b.metrics, 'total_return') ?? -Infinity) - (metricOf(a.metrics, 'total_return') ?? -Infinity)
  )
  return rows
})

const bestKey = computed(() => {
  const best = sweepRows.value[0]
  return best ? JSON.stringify(best.params) : ''
})

/** 扫描指标聚合口径可能带 _mean/_median 后缀,做兜底 */
function metricOf(m: SweepMetrics, key: string): number | undefined {
  const v = m[`${key}_mean`] ?? m[key] ?? m[`${key}_median`]
  return typeof v === 'number' && !Number.isNaN(v) ? v : undefined
}

function paramsText(params: Record<string, number>): string {
  return Object.entries(params)
    .map(([k, v]) => `${k}=${v}`)
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
    const v = metricOf(r.metrics, 'total_return')
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
  const param_grid = parseGrid()
  if (!Object.keys(param_grid).length) {
    error.value = '请至少配置一个参数网格(参数名 + 逗号分隔的候选值)'
    return
  }
  running.value = true
  try {
    sweepResult.value = await api.sweepBacktest({
      strategy: form.strategy,
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
    <div class="flex items-center gap-4">
      <h2 class="text-lg font-semibold">回测</h2>
      <div class="flex rounded-md border border-border text-sm">
        <button
          class="rounded-l-md px-3 py-1"
          :class="mode === 'single' ? 'bg-active font-medium text-text-primary' : 'text-text-secondary hover:bg-hover'"
          @click="mode = 'single'"
        >
          单次回测
        </button>
        <button
          class="rounded-r-md px-3 py-1"
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
          {{ running ? '运行中…' : mode === 'single' ? '运行回测' : '开始扫描' }}
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

      <div v-if="mode === 'sweep'">
        <div class="mb-1 flex items-center justify-between">
          <span class="text-xs text-text-tertiary">参数网格(参数名 + 逗号分隔候选值,如 fast: 3,5,8)</span>
          <button type="button" class="text-xs text-accent hover:underline" @click="addGridRow">+ 添加参数</button>
        </div>
        <div class="space-y-2">
          <div v-for="(row, i) in gridRows" :key="i" class="flex items-center gap-2">
            <input
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
        <span class="mb-1 block text-xs text-text-tertiary">查询历史回测 run_id</span>
        <input v-model="runIdInput" type="number" min="1" class="w-32 rounded-md border border-border px-2 py-1.5" />
      </label>
      <button :disabled="running" class="rounded-md border border-border px-4 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50" @click="loadRun">
        查询
      </button>
    </div>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>

    <!-- 参数扫描结果 -->
    <template v-if="mode === 'sweep' && sweepResult">
      <div class="flex items-center gap-3 text-sm text-text-secondary">
        <span>策略: <span class="font-medium text-text-primary">{{ sweepResult.strategy }}</span></span>
        <span>{{ sweepResult.start }} ~ {{ sweepResult.end }}</span>
        <span>{{ sweepRows.length }} 组参数组合</span>
      </div>

      <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-xs text-text-tertiary">
              <th class="px-4 py-2 font-medium">#</th>
              <th class="px-4 py-2 font-medium">参数</th>
              <th class="px-4 py-2 text-right font-medium">总收益</th>
              <th class="px-4 py-2 text-right font-medium">年化收益</th>
              <th class="px-4 py-2 text-right font-medium">最大回撤</th>
              <th class="px-4 py-2 text-right font-medium">夏普</th>
              <th class="px-4 py-2 text-right font-medium">胜率</th>
              <th class="px-4 py-2 text-right font-medium">交易次数</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="(r, i) in sweepRows"
              :key="JSON.stringify(r.params)"
              class="border-b border-border-subtle last:border-0 hover:bg-hover"
              :class="JSON.stringify(r.params) === bestKey ? 'bg-down/5' : ''"
            >
              <td class="px-4 py-2 text-text-tertiary">
                {{ i + 1 }}
                <span
                  v-if="JSON.stringify(r.params) === bestKey"
                  class="ml-1 rounded bg-down/10 px-1.5 py-0.5 text-xs font-medium text-down"
                >最优</span>
              </td>
              <td class="px-4 py-2 font-medium">{{ paramsText(r.params) }}</td>
              <td class="px-4 py-2 text-right" :class="(metricOf(r.metrics, 'total_return') ?? 0) >= 0 ? 'text-up' : 'text-down'">
                {{ metricOf(r.metrics, 'total_return') !== undefined ? fmtPct(metricOf(r.metrics, 'total_return')) : '--' }}
              </td>
              <td class="px-4 py-2 text-right" :class="(metricOf(r.metrics, 'annual_return') ?? 0) >= 0 ? 'text-up' : 'text-down'">
                {{ metricOf(r.metrics, 'annual_return') !== undefined ? fmtPct(metricOf(r.metrics, 'annual_return')) : '--' }}
              </td>
              <td class="px-4 py-2 text-right text-down">
                {{ metricOf(r.metrics, 'max_drawdown') !== undefined ? fmtPct(metricOf(r.metrics, 'max_drawdown')) : '--' }}
              </td>
              <td class="px-4 py-2 text-right">
                {{ metricOf(r.metrics, 'sharpe') !== undefined ? fmtPrice(metricOf(r.metrics, 'sharpe')) : '--' }}
              </td>
              <td class="px-4 py-2 text-right">
                {{ metricOf(r.metrics, 'win_rate') !== undefined ? fmtPct(metricOf(r.metrics, 'win_rate')) : '--' }}
              </td>
              <td class="px-4 py-2 text-right">{{ metricOf(r.metrics, 'trade_count') ?? '--' }}</td>
            </tr>
          </tbody>
        </table>
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
