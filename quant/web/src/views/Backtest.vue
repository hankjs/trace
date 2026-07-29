<script setup lang="ts">
import { computed, onActivated, onMounted, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { AlertTriangle, CheckCircle2, ChevronRight, Code2, Fingerprint, Plus, TriangleAlert } from 'lucide-vue-next'
import type { EChartsCoreOption } from 'echarts/core'
import {
  api,
  hasSurvivorshipBias,
  type BacktestExitReasonCount,
  type BacktestExitReasonDistribution,
  type BacktestResult,
  type BacktestTradeDetail,
  type BacktestValidation,
  type StrategyEvidenceStatus,
  type StrategyParamValue,
  type StrategySpec,
  type SweepResult,
  type SweepResultItem,
  type WatchItem,
} from '../api'
import { metricName } from '../catalog'
import { aggregateMetric, fmtPct, fmtPrice } from '../format'
import EChart from '../components/EChart.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import PoolSelect from '../components/PoolSelect.vue'
import QuTable from '../components/QuTable.vue'
import type { QuTableColumn } from '../components/quTable'
import StockPicker from '../components/StockPicker.vue'
import StrategySelect from '../components/StrategySelect.vue'
import { poolById } from '../pools'
import { strategyById, useStrategies } from '../strategies'
import {
  RESEARCH_PLAN_BOUNDARY,
} from '../researchPlans'

const route = useRoute()

const { load: loadStrategies } = useStrategies()
const watchlist = ref<WatchItem[]>([])
const result = ref<BacktestResult | null>(null)
const running = ref(false)
const error = ref('')
const runIdInput = ref('')
/** 股票池:组合策略的研究范围;单标的策略在「按股票池」模式下使用 */
const poolId = ref<number | null>(null)
/** 单标的策略的选股方式:手动选股 / 按股票池(与 codes 互斥) */
const scopeMode = ref<'stocks' | 'pool'>('stocks')
/** 选中的数据库策略；回测只按当前完整规格运行。 */
const strategyId = ref<number | null>(null)

/** 模式:single 单次回测 / sweep 参数扫描 */
const mode = ref<'single' | 'sweep'>('single')

const form = reactive({
  codes: [] as string[],
  start: '',
  end: '',
})

const strategy = computed(() => strategyById(strategyId.value))
const isPortfolio = computed(() => strategy.value?.kind === 'portfolio')
const costForm = reactive({ commissionWan: 2.5, stampTaxWan: 5, slippageWan: 1 })
const selectedCapability = computed(() => strategy.value?.capability ?? null)
const strategyRunnable = computed(() => selectedCapability.value?.status === 'supported')
const selectedHypothesis = computed(() => String(strategy.value?.spec?.metadata?.hypothesis ?? '未提供研究假设'))

watch(strategy, () => {
  if (isPortfolio.value && mode.value === 'sweep') mode.value = 'single'
})

// ---- 参数扫描 ----

interface GridRow {
  path: string
  valuesText: string
}

const gridRows = ref<GridRow[]>([{ path: '', valuesText: '' }])
const sweepResult = ref<SweepResult | null>(null)

/** 当前策略在规格里声明的参数扫描(validation.parameter_scans) */
const declaredScans = computed(() => {
  const validation = record(strategy.value?.spec?.validation)
  return Array.isArray(validation.parameter_scans)
    ? validation.parameter_scans.map(record).filter((scan) => typeof scan.path === 'string')
    : []
})

const scanPathLabels: Record<string, string> = {
  '$.positioning.target': '目标仓位',
  '$.holding.cooldown_days': '冷却天数',
  '$.overlays.risk.enabled': '风险止损开关',
  '$.overlays.risk.value': '风险止损数值',
  '$.overlays.risk.atr_period': '风险 ATR 周期',
  '$.overlays.risk.trailing': '风险追踪开关',
  '$.overlays.take_profit.enabled': '止盈开关',
  '$.overlays.take_profit.value': '止盈数值',
  '$.overlays.take_profit.atr_period': '止盈 ATR 周期',
  '$.overlays.take_profit.trailing': '止盈追踪开关',
  '$.universe.min_listing_days': '最少上市天数',
  '$.universe.min_amount_avg20': '20 日平均成交额下限',
  '$.execution.max_entry_premium': '最大入场跳空比例',
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
}

function specPathExists(spec: StrategySpec | undefined, path: string): boolean {
  if (!spec || !path.startsWith('$.')) return false
  let current: unknown = spec
  for (const part of path.slice(2).split('.')) {
    const parent = record(current)
    if (!(part in parent)) return false
    current = parent[part]
  }
  return typeof current === 'number' || typeof current === 'boolean'
}

const scanParameterOptions = computed(() => Object.entries(scanPathLabels)
  .filter(([path]) => specPathExists(strategy.value?.spec, path))
  .map(([path, name]) => ({ path, name })))

function addGridRow() {
  gridRows.value.push({ path: '', valuesText: '' })
}

function removeGridRow(i: number) {
  gridRows.value.splice(i, 1)
}

function gridPlaceholder(path: string): string {
  if (path.endsWith('.enabled') || path.endsWith('.trailing')) return 'true, false'
  return '候选值，如 3, 5, 8'
}

function parseGrid(): Record<string, Array<number | boolean>> {
  const grid: Record<string, Array<number | boolean>> = {}
  for (const row of gridRows.value) {
    const path = row.path.trim()
    if (!path) continue
    if (!/^\$\.[a-zA-Z0-9_.]+$/.test(path)) {
      throw new Error(`扫描路径必须以 $. 开头且只包含字段名：${path}`)
    }
    const tokens = row.valuesText
      .split(/[,，\s]+/)
      .map((value) => value.trim())
      .filter(Boolean)
    const values = tokens.map((value): number | boolean => {
      const normalized = value.toLowerCase()
      if (normalized === 'true') return true
      if (normalized === 'false') return false
      const number = Number(value)
      if (Number.isFinite(number)) return number
      throw new Error(`候选值只支持有限数字或 true / false：${value}`)
    })
    if (!values.length) throw new Error(`请填写 ${path} 的候选值`)
    grid[path] = [...new Set<number | boolean>(values)]
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
  { key: 'params', label: '规格路径', cellClass: 'font-medium' },
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

function pathName(path: string): string {
  return scanPathLabels[path] ?? path
}

function paramsText(params: Record<string, StrategyParamValue>): string {
  return Object.entries(params)
    .map(([path, value]) => `${pathName(path)}=${String(value)}`)
    .join(', ')
}

function runCosts() {
  return {
    commission: costForm.commissionWan / 10000,
    stamp_tax: costForm.stampTaxWan / 10000,
    slippage: costForm.slippageWan / 10000,
  }
}

/** 恰好两个参数时画热力图:两参数为轴,总收益为值 */
const heatmapOption = computed<EChartsCoreOption | null>(() => {
  const rows = sweepResult.value?.results ?? []
  if (!rows.length) return null
  const names = [...new Set(rows.flatMap((r) => Object.keys(r.params)))]
    .filter((name) => {
      const values = rows.map((row) => row.params[name])
      return values.every((value) => typeof value === 'number')
        && new Set(values.map(String)).size > 1
    })
  if (names.length !== 2) return null
  const [xName, yName] = names
  const xVals = [...new Set(rows.map((r) => r.params[xName] as number))].sort((a, b) => a - b)
  const yVals = [...new Set(rows.map((r) => r.params[yName] as number))].sort((a, b) => a - b)
  const data: [number, number, number][] = []
  for (const r of rows) {
    const v = aggregateMetric(r.metrics, 'total_return')
    if (v === undefined) continue
    data.push([xVals.indexOf(r.params[xName] as number), yVals.indexOf(r.params[yName] as number), +v.toFixed(4)])
  }
  if (!data.length) return null
  const values = data.map((d) => d[2])
  return {
    animation: false,
    tooltip: {
      formatter: (p: { data: [number, number, number] }) =>
        `${pathName(xName)}=${xVals[p.data[0]]}, ${pathName(yName)}=${yVals[p.data[1]]}<br/>总收益: ${fmtPct(p.data[2])}`,
    },
    grid: { left: 90, right: 90, top: 30, bottom: 50 },
    xAxis: { type: 'category', name: pathName(xName), data: xVals.map(String) },
    yAxis: { type: 'category', name: pathName(yName), data: yVals.map(String) },
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
  if (!strategyRunnable.value) {
    error.value = '当前策略存在数据或引擎能力缺口，不能运行参数扫描'
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
  let param_grid: Record<string, Array<number | boolean>>
  try {
    param_grid = parseGrid()
  } catch (caught) {
    error.value = (caught as Error).message
    return
  }
  if (!Object.keys(param_grid).length) {
    error.value = '请至少配置一个规格路径及其候选值'
    return
  }
  await submitSweep({ param_grid })
}

/** 按规格 validation.parameter_scans 的声明执行扫描(服务端组装候选网格) */
async function runDeclaredSweep() {
  error.value = ''
  const codes = parsedCodes()
  if (strategyId.value === null) {
    error.value = '请选择策略'
    return
  }
  if (!strategyRunnable.value) {
    error.value = '当前策略存在数据或引擎能力缺口，不能运行参数扫描'
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
  if (!declaredScans.value.length) {
    error.value = '当前策略规格未声明参数扫描'
    return
  }
  await submitSweep({ declared: true })
}

async function submitSweep(body: {
  param_grid?: Record<string, Array<number | boolean>>
  declared?: boolean
}) {
  running.value = true
  try {
    sweepResult.value = await api.sweepBacktest({
      strategy_id: strategyId.value as number,
      codes: parsedCodes(),
      start: form.start,
      end: form.end,
      costs: runCosts(),
      ...body,
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
const resultPool = computed(() => result.value?.pool ?? ((isPortfolio.value || scopeMode.value === 'pool') ? poolById(poolId.value) : null))
const resultBiased = computed(() => hasSurvivorshipBias(resultPool.value))

const resultCosts = computed(() => result.value?.costs ?? {})
const resultEvidence = computed(() => result.value?.evidence ?? result.value?.metrics.evidence)
const resultDataQuality = computed(() => {
  const top = result.value?.data_quality
  if (top) return top
  const metrics = result.value?.metrics as { data_quality?: typeof top } | undefined
  return metrics?.data_quality ?? null
})
const resultAttribution = computed(() => {
  const top = result.value?.execution_attribution
  if (top) return top
  const metrics = result.value?.metrics as { execution_attribution?: typeof top } | undefined
  return metrics?.execution_attribution ?? null
})
const resultSpecSnapshot = computed(() => result.value?.strategy_spec_snapshot ?? resultEvidence.value?.strategy_spec_snapshot)
const resultSpecHash = computed(() => result.value?.strategy_spec_hash ?? resultEvidence.value?.strategy_spec_hash ?? '')
const resultCompilerVersion = computed(() => result.value?.compiler_version ?? resultEvidence.value?.compiler_version ?? '')
const resultComponentVersions = computed(() => result.value?.component_versions ?? resultEvidence.value?.component_versions ?? {})
const resultFingerprints = computed(() => [
  { label: '执行指纹', value: result.value?.execution_fingerprint ?? resultEvidence.value?.execution_fingerprint ?? '' },
  { label: '数据指纹', value: result.value?.data_fingerprint ?? resultEvidence.value?.data_fingerprint ?? '' },
  { label: '股票池指纹', value: result.value?.universe_fingerprint ?? resultEvidence.value?.universe_fingerprint ?? '' },
  { label: '费用指纹', value: result.value?.cost_fingerprint ?? resultEvidence.value?.cost_fingerprint ?? '' },
].filter((item) => item.value))
const resultSpecJson = computed(() => resultSpecSnapshot.value ? JSON.stringify(resultSpecSnapshot.value, null, 2) : '')
const resultCurrentStrategy = computed(() => strategyById(result.value?.strategy_id ?? null))

/** 规格身份:剔除 evidence_status(状态推进会改变 spec_hash,但规则内容未变) */
function specIdentity(spec: StrategySpec | undefined | null): string {
  if (!spec) return ''
  const copy = JSON.parse(JSON.stringify(spec)) as Record<string, unknown>
  const metadata = record(copy.metadata)
  delete metadata.evidence_status
  copy.metadata = metadata
  return JSON.stringify(copy)
}

const exactHashMatch = computed<boolean | null>(() => {
  const current = resultCurrentStrategy.value?.spec
  if (!current || !resultSpecSnapshot.value) return null
  return specIdentity(current) === specIdentity(resultSpecSnapshot.value)
})

// ---- 规格验证报告(基线对比 / OOS 分段 / 否决判定) ----

const resultValidation = computed<BacktestValidation | null>(
  () => result.value?.validation ?? result.value?.metrics.validation ?? null,
)
const validationOos = computed(() => {
  const oos = resultValidation.value?.oos
  return oos && oos.enabled ? oos : null
})
const validationRejection = computed(() => resultValidation.value?.rejection ?? null)

const EVIDENCE_STATUS_NAMES: Record<StrategyEvidenceStatus, string> = {
  unverified: '未验证',
  design_complete: '验证设计完成',
  backtested: '已回测（样本内）',
  oos_passed: '样本外否决条件通过',
  rejected: '已否决',
}

const evidenceTransitionText = computed(() => {
  const transition = result.value?.evidence_transition
  if (!transition) return ''
  return `证据状态已推进:${EVIDENCE_STATUS_NAMES[transition.from] ?? transition.from} → ${EVIDENCE_STATUS_NAMES[transition.to] ?? transition.to}`
})

/** OOS 分段指标展示行:样本内 / 样本外并排 */
const oosMetricRows = computed(() => {
  const oos = validationOos.value
  if (!oos?.available) return []
  const keys = [
    { key: 'total_return', label: metricName('total_return'), pct: true },
    { key: 'annual_return', label: metricName('annual_return'), pct: true },
    { key: 'max_drawdown', label: metricName('max_drawdown'), pct: true },
    { key: 'sharpe', label: metricName('sharpe'), pct: false },
  ]
  return keys.map((item) => {
    const format = (value: unknown) => {
      if (typeof value !== 'number') return '--'
      return item.pct ? fmtPct(value) : fmtPrice(value)
    }
    return {
      label: item.label,
      inSample: format(oos.in_sample?.[item.key]),
      oos: format(oos.oos?.[item.key]),
    }
  })
})
const resultTradeDetails = computed(() => result.value?.trade_details ?? resultEvidence.value?.trade_details ?? [])

const exitReasonRows = computed<BacktestExitReasonCount[]>(() => {
  const distribution = result.value?.exit_reason_distribution ?? resultEvidence.value?.exit_reason_distribution
  if (!distribution) return []
  if (Array.isArray(distribution)) return distribution
  const structured = distribution as BacktestExitReasonDistribution
  if (structured.by_primary && structured.all_hits) {
    const reasons = new Set([...Object.keys(structured.by_primary), ...Object.keys(structured.all_hits)])
    return [...reasons].map((reason) => ({
      reason,
      reason_name: exitReasonName(reason),
      count: structured.all_hits[reason] ?? 0,
      primary_count: structured.by_primary[reason] ?? 0,
    }))
  }
  return Object.entries(distribution)
    .filter(([, count]) => typeof count === 'number')
    .map(([reason, count]) => ({ reason, reason_name: exitReasonName(reason), count: count as number }))
})

const tradeColumns: QuTableColumn<BacktestTradeDetail>[] = [
  { key: 'stock', label: '股票' },
  { key: 'side', label: '方向' },
  { key: 'signal_date', label: '信号日' },
  { key: 'execution_date', label: '模拟成交日' },
  { key: 'execution_price', label: '模拟成交价', align: 'right' },
  { key: 'size', label: '模拟数量', align: 'right' },
  { key: 'fees', label: '费用', align: 'right' },
  { key: 'all_reasons', label: '全部原因', cellClass: 'min-w-64' },
  { key: 'realized_pnl', label: '已实现模拟盈亏', align: 'right' },
]

const exitReasonLabels: Record<string, string> = {
  risk_overlay: '风险覆盖层',
  take_profit: '止盈覆盖层',
  native: '策略原生退出',
  native_exit: '策略原生退出',
  native_entry: '策略原生入场',
  rebalance: '组合调仓或资格变化',
}

function exitReasonName(reason: string): string {
  return exitReasonLabels[reason] ?? reason
}

function stockName(code: string): string {
  return result.value?.stocks?.find((item) => item.code === code)?.name
    || sweepResult.value?.stocks?.find((item) => item.code === code)?.name
    || watchlist.value.find((item) => item.code === code)?.name
    || '名称待同步'
}

function parsedCodes(): string[] {
  return [...new Set(form.codes)]
}

async function run() {
  error.value = ''
  // 组合策略始终可用池;单标的策略仅在「按股票池」模式下用池,与手动 codes 互斥
  const usePool = poolId.value !== null && (isPortfolio.value || scopeMode.value === 'pool')
  const codes = usePool ? [] : parsedCodes()
  if (strategyId.value === null) {
    error.value = '请选择策略'
    return
  }
  if (!strategyRunnable.value) {
    error.value = '当前策略存在数据或引擎能力缺口，请先在策略管理中修正'
    return
  }
  if (!codes.length && !usePool && !isPortfolio.value) {
    error.value = '请选择股票，或切换为按股票池回测'
    return
  }
  if (!form.start || !form.end) {
    error.value = '请选择起止日期'
    return
  }
  running.value = true
  try {
    result.value = await api.runBacktest({
      strategy_id: strategyId.value,
      codes,
      start: form.start,
      end: form.end,
      // 组合策略按股票池解析成分(取代旧的「codes 留空隐式动态池」约定);
      // 单标的「按股票池」模式同样留空 codes 并下发 pool_id
      ...(usePool && !codes.length ? { pool_id: poolId.value! } : {}),
      costs: runCosts(),
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

/** 从路由同步策略预选。KeepAlive 下需 watch / onActivated,不能只在 onMounted 读一次。 */
function syncStrategyFromRoute() {
  const raw = route.query.strategy
  const requested = Number(Array.isArray(raw) ? raw[0] : raw)
  if (requested && strategyById(requested)) {
    strategyId.value = requested
  }
}

onMounted(async () => {
  try {
    // 策略列表由 StrategySelect 自行加载,这里只补自选股;
    // 仍等待策略列表完成，确保 strategy_id 与能力状态先落位。
    const [, w] = await Promise.all([loadStrategies(), api.watchlist()])
    watchlist.value = w.items
    syncStrategyFromRoute()
  } catch (e) {
    error.value = (e as Error).message
  }
  // 支持 /backtest?run=1 直接查看历史回测
  const q = Number(Array.isArray(route.query.run) ? route.query.run[0] : route.query.run)
  if (q) {
    runIdInput.value = String(q)
    await loadRun()
  }
})

// KeepAlive: 从策略管理再次「回测验证」时组件可能已挂载,需在激活/query 变化时重读
onActivated(() => {
  syncStrategyFromRoute()
})

watch(
  () => route.query.strategy,
  () => {
    syncStrategyFromRoute()
  },
)
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div class="flex items-center gap-4">
        <h2 class="text-base font-semibold">历史回测</h2>
        <div class="segmented" role="group" aria-label="回测模式">
          <button
            type="button"
            :aria-pressed="mode === 'single'"
            @click="mode = 'single'"
          >
            单次回测
          </button>
          <button
            type="button"
            :aria-pressed="mode === 'sweep'"
            :disabled="isPortfolio"
            @click="mode = 'sweep'"
          >
            参数扫描
          </button>
        </div>
      </div>
      <div v-if="mode === 'single'" class="flex items-center gap-2">
        <label for="history-run-id" class="text-xs text-text-tertiary">查询历史回测编号</label>
        <input
          id="history-run-id"
          v-model="runIdInput"
          type="number"
          min="1"
          class="h-8 w-28 rounded-md border border-border px-2.5 text-sm"
        />
        <button :disabled="running" class="btn btn-secondary btn-sm" @click="loadRun">
          查询
        </button>
      </div>
    </div>

    <form
      class="space-y-3 rounded-lg border border-border bg-surface-raised p-4"
      @submit.prevent="mode === 'single' ? run() : runSweep()"
    >
      <div class="flex flex-wrap items-end gap-3">
        <StrategySelect v-model="strategyId" />
      </div>

      <div v-if="strategy" class="border-t border-border-subtle pt-3">
        <InlineFeedback v-if="selectedCapability && selectedCapability.status !== 'supported'" tone="error">
          {{ selectedCapability.issues[0]?.message ?? '当前策略不能回测' }}
          <template v-if="selectedCapability.issues[0]?.path">（{{ selectedCapability.issues[0].path }}）</template>
        </InlineFeedback>
        <ul v-if="selectedCapability && selectedCapability.status !== 'supported' && selectedCapability.issues.length > 1" class="mt-2 space-y-1 text-xs text-text-secondary">
          <li v-for="issue in selectedCapability.issues.slice(1)" :key="`${issue.code}-${issue.path}`">
            {{ issue.message }} <code class="text-text-tertiary">{{ issue.path }}</code>
          </li>
        </ul>
        <div v-else class="text-xs">
          <p class="font-medium text-text-primary">{{ selectedHypothesis }}</p>
          <p class="mt-1 text-text-tertiary">回测将固化当前完整规格，页面不提供临时规则覆盖。</p>
        </div>
      </div>

      <div v-if="isPortfolio" class="border-t border-border-subtle pt-3">
        <PoolSelect v-model="poolId" label="股票池（组合策略的选股范围）" />
      </div>

      <div class="flex flex-wrap items-end gap-3 border-t border-border-subtle pt-3">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">开始日期</span>
          <input v-model="form.start" type="date" class="h-9 rounded-md border border-border px-2.5 text-sm" />
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">结束日期</span>
          <input v-model="form.end" type="date" class="h-9 rounded-md border border-border px-2.5 text-sm" />
        </label>
        <div class="min-w-64 flex-1">
          <div class="mb-1 flex items-center justify-between gap-2">
            <span class="text-xs text-text-tertiary">
              {{ isPortfolio
                ? '股票（留空则使用所选股票池在区间内的动态成分）'
                : scopeMode === 'pool'
                  ? '股票池（按成分逐日解析，入池前不会建仓）'
                  : '股票' }}
            </span>
            <div v-if="!isPortfolio && mode === 'single'" class="segmented" role="group" aria-label="选股方式">
              <button
                type="button"
                :aria-pressed="scopeMode === 'stocks'"
                @click="scopeMode = 'stocks'"
              >
                手动选股
              </button>
              <button
                type="button"
                :aria-pressed="scopeMode === 'pool'"
                @click="scopeMode = 'pool'"
              >
                按股票池
              </button>
            </div>
          </div>
          <StockPicker
            v-if="isPortfolio || scopeMode === 'stocks'"
            v-model="form.codes"
            placeholder="点击选择股票（可多选）"
          />
          <PoolSelect v-else v-model="poolId" hide-label :manage-link="false" />
        </div>
      </div>

      <div v-if="mode === 'sweep'" class="border-t border-border-subtle pt-3">
        <div class="mb-1 flex items-center justify-between">
          <span class="text-xs text-text-tertiary">规格路径与候选值</span>
          <button type="button" class="btn btn-ghost btn-sm" @click="addGridRow">
            <Plus :size="13" />
            添加路径
          </button>
        </div>
        <div class="space-y-2">
          <div v-for="(row, i) in gridRows" :key="i" class="flex items-center gap-2">
            <input
              v-model.trim="row.path"
              data-testid="sweep-path"
              list="strategy-scan-paths"
              placeholder="$.overlays.risk.value"
              class="h-9 w-72 rounded-md border border-border px-2.5 font-mono text-sm"
            />
            <input
              v-model="row.valuesText"
              data-testid="sweep-values"
              :placeholder="gridPlaceholder(row.path)"
              class="h-9 w-64 rounded-md border border-border px-2.5 text-sm"
            />
            <button
              v-if="gridRows.length > 1"
              type="button"
              class="btn btn-ghost-danger btn-sm"
              @click="removeGridRow(i)"
            >
              删除
            </button>
          </div>
        </div>
        <datalist id="strategy-scan-paths">
          <option v-for="parameter in scanParameterOptions" :key="parameter.path" :value="parameter.path">{{ parameter.name }}</option>
        </datalist>
        <p class="mt-2 text-xs leading-5 text-text-tertiary">
          路径必须以 <code>$.</code> 开头并指向当前规格中的数字或布尔字段。候选值用逗号分隔，最多 200 组组合。
        </p>
        <div v-if="declaredScans.length" class="mt-3 flex flex-wrap items-center gap-3 rounded-md bg-surface-muted px-3 py-2">
          <span class="text-xs text-text-secondary">
            规格声明了 {{ declaredScans.length }} 项参数扫描：{{ declaredScans.map((scan) => String(scan.path)).join('、') }}
          </span>
          <button
            type="button"
            :disabled="running || !strategyRunnable"
            class="btn btn-secondary btn-sm"
            @click="runDeclaredSweep"
          >按规格声明扫描</button>
        </div>
      </div>

      <details class="group border-t border-border-subtle pt-3">
        <summary class="flex cursor-pointer list-none items-center gap-1.5 text-xs font-medium text-text-secondary transition-colors hover:text-text-primary">
          <ChevronRight :size="13" class="text-text-tertiary transition-transform group-open:rotate-90" />
          模拟费用假设（万分）
          <span class="font-normal text-text-tertiary">
            佣金 {{ costForm.commissionWan }} · 印花税 {{ costForm.stampTaxWan }} · 滑点 {{ costForm.slippageWan }}
          </span>
        </summary>
        <div class="mt-3 flex flex-wrap gap-3">
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">双边佣金</span>
            <input v-model.number="costForm.commissionWan" type="number" min="0" max="500" step="0.1" class="h-9 w-28 rounded-md border border-border px-2.5 text-sm" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">卖出印花税</span>
            <input v-model.number="costForm.stampTaxWan" type="number" min="0" max="500" step="0.1" class="h-9 w-28 rounded-md border border-border px-2.5 text-sm" />
          </label>
          <label class="text-sm">
            <span class="mb-1 block text-xs text-text-tertiary">双边滑点</span>
            <input v-model.number="costForm.slippageWan" type="number" min="0" max="1000" step="0.1" class="h-9 w-28 rounded-md border border-border px-2.5 text-sm" />
          </label>
        </div>
      </details>

      <div class="flex justify-end border-t border-border-subtle pt-3">
        <button type="submit" :disabled="running || !strategyRunnable" class="btn btn-primary">
          {{ running ? '运行中…' : mode === 'single' ? '运行回测' : '开始扫描' }}
        </button>
      </div>
    </form>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>

    <!-- 参数扫描结果 -->
    <template v-if="mode === 'sweep' && sweepResult">
      <div class="flex items-center gap-3 text-sm text-text-secondary">
        <span>
          策略: <span class="font-medium text-text-primary">{{ sweepResult.strategy_name }}</span>
          <code v-if="sweepResult.strategy_spec_hash" class="ml-2 text-xs text-text-tertiary">{{ sweepResult.strategy_spec_hash.slice(0, 12) }}</code>
        </span>
        <span>{{ sweepResult.start }} ~ {{ sweepResult.end }}</span>
        <span>{{ sweepRows.length }} 组路径组合</span>
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
        <h3 class="px-2 pt-2 text-base font-semibold">规格路径热力图（总收益）</h3>
        <EChart :option="heatmapOption" height="380px" />
      </section>

      <section
        v-if="sweepResult.declared && sweepResult.stability"
        class="rounded-lg border border-border bg-surface-raised p-4"
        aria-labelledby="sweep-stability-heading"
      >
        <h3 id="sweep-stability-heading" class="text-sm font-semibold">参数稳定性(按规格声明)</h3>
        <template v-if="sweepResult.stability.status === 'evaluated'">
          <p class="mt-2 text-sm text-text-secondary">
            当前参数年化中位数 {{ fmtPct(sweepResult.stability.current ?? 0) }}，
            扫描中位数 {{ fmtPct(sweepResult.stability.median ?? 0) }}，
            优于当前参数的组合占比 {{ fmtPct(sweepResult.stability.better_share ?? 0) }}。
          </p>
          <p
            class="mt-2 rounded-md px-3 py-2 text-sm"
            :class="sweepResult.stability.unstable
              ? 'bg-up/10 text-up'
              : 'bg-down/10 text-down'"
          >
            {{ sweepResult.stability.unstable
              ? '超过一半的扫描组合优于当前参数,参数不稳定(unstable_parameters 将命中否决)。'
              : '当前参数处于扫描组合的前半,未触发参数不稳定否决。' }}
          </p>
        </template>
        <p v-else class="mt-2 text-sm text-text-tertiary">
          稳定性不可评估:{{ sweepResult.stability.reason ?? '扫描结果不可用' }}
        </p>
      </section>
    </template>

    <!-- 单次回测结果 -->
    <template v-if="mode === 'single' && result">
      <div class="flex flex-wrap items-center gap-3 text-sm text-text-secondary">
        <span>回测编号：<span class="font-medium text-text-primary">{{ result.run_id }}</span></span>
        <template v-if="result.strategy_id">
          <!-- 策略行可能已被删除,后端此时回显 strategy_name = null -->
          <span>策略: {{ result.strategy_name ?? '策略已删除' }}</span>
          <span>{{ result.start }} ~ {{ result.end }}</span>
        </template>
        <span v-if="resultPool">股票池: {{ resultPool.name }}</span>
        <span v-if="evidenceTransitionText" class="rounded bg-info-soft px-2 py-0.5 text-xs text-text-secondary">
          {{ evidenceTransitionText }}
        </span>
      </div>

      <div
        v-if="resultSpecHash"
        class="flex items-start gap-2 rounded-md border px-4 py-3 text-sm leading-5"
        :class="exactHashMatch === true
          ? 'border-down/30 bg-down/5 text-text-secondary'
          : exactHashMatch === false
            ? 'border-warning/30 bg-warning-soft text-text-secondary'
            : 'border-border bg-info-soft text-text-secondary'"
      >
        <CheckCircle2 v-if="exactHashMatch === true" :size="17" class="mt-0.5 shrink-0 text-down" />
        <TriangleAlert v-else-if="exactHashMatch === false" :size="17" class="mt-0.5 shrink-0 text-warning" />
        <Fingerprint v-else :size="17" class="mt-0.5 shrink-0 text-text-tertiary" />
        <span v-if="exactHashMatch === true">本次规格哈希与当前策略一致，可作为当前策略的模拟回测证据（须先完成验证设计，且不代表可交易）。</span>
        <span v-else-if="exactHashMatch === false">这是旧规格的历史快照，不作为当前策略证据。当前策略修改不会改变本次结果。</span>
        <span v-else>本次回测保留了完整规格快照，但当前策略已不可用，无法建立精确哈希关联。</span>
      </div>

      <p
        v-if="resultDataQuality?.warnings?.length"
        class="flex items-start gap-2 rounded-md border border-warning/30 bg-warning-soft px-4 py-3 text-sm leading-6 text-text-secondary"
      >
        <AlertTriangle :size="16" class="mt-0.5 shrink-0 text-warning" />
        <span>
          <strong class="font-medium text-text-primary">数据信任警告</strong>
          <template v-if="resultDataQuality.st_history_incomplete">
            ：ST 历史不完整（空值 bar 占比 {{ fmtPct(resultDataQuality.st_null_bar_ratio ?? 0) }}）。
          </template>
          <ul class="mt-1 list-disc pl-4">
            <li v-for="(w, i) in resultDataQuality.warnings" :key="i">{{ w }}</li>
          </ul>
        </span>
      </p>

      <div
        v-if="resultAttribution"
        class="rounded-md border border-border bg-surface-raised px-4 py-3 text-sm leading-6 text-text-secondary"
      >
        <strong class="font-medium text-text-primary">成交归因</strong>
        <span class="ml-2 text-xs text-text-tertiary">理论信号 vs 模拟可成交</span>
        <dl class="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-xs sm:grid-cols-3">
          <div>买入信号日 {{ resultAttribution.buy_signal_days ?? 0 }}</div>
          <div>买入成交 {{ resultAttribution.buy_filled ?? 0 }}</div>
          <div>涨停/停牌未买 {{ resultAttribution.buy_blocked_limit_up_or_halt ?? 0 }}</div>
          <div>卖出信号日 {{ resultAttribution.sell_signal_days ?? 0 }}</div>
          <div>卖出成交 {{ resultAttribution.sell_filled ?? 0 }}</div>
          <div>跌停延迟卖出 {{ resultAttribution.sell_delayed ?? 0 }}</div>
          <div>缺 bar 阻断 {{ resultAttribution.missing_bar_block ?? 0 }}</div>
        </dl>
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

      <section class="border-y border-border bg-surface-raised px-4 py-4" aria-labelledby="backtest-snapshot-heading">
        <div class="flex items-center gap-2">
          <Fingerprint :size="17" class="text-text-tertiary" />
          <h3 id="backtest-snapshot-heading" class="text-sm font-semibold">不可变执行证据</h3>
        </div>
        <div class="mt-3 grid gap-4 text-xs leading-5 md:grid-cols-2">
          <div>
            <h4 class="font-medium text-text-primary">规格与编译器</h4>
            <dl class="mt-1 space-y-1 text-text-secondary">
              <div class="flex justify-between gap-3"><dt>StrategySpec 哈希</dt><dd class="font-mono" :title="resultSpecHash">{{ resultSpecHash ? resultSpecHash.slice(0, 16) : '未提供' }}</dd></div>
              <div class="flex justify-between gap-3"><dt>编译器版本</dt><dd class="font-mono">{{ resultCompilerVersion || '未提供' }}</dd></div>
            </dl>
          </div>
          <div>
            <h4 class="font-medium text-text-primary">组件版本</h4>
            <dl v-if="Object.keys(resultComponentVersions).length" class="mt-1 space-y-1 text-text-secondary">
              <div v-for="(version, component) in resultComponentVersions" :key="component" class="flex justify-between gap-3">
                <dt class="font-mono">{{ component }}</dt><dd class="font-mono">{{ version }}</dd>
              </div>
            </dl>
            <p v-else class="mt-1 text-text-tertiary">未提供组件版本。</p>
          </div>
          <div class="md:col-span-2">
            <h4 class="font-medium text-text-primary">复现指纹</h4>
            <dl v-if="resultFingerprints.length" class="mt-1 grid gap-1 md:grid-cols-2">
              <div v-for="item in resultFingerprints" :key="item.label" class="flex min-w-0 justify-between gap-3 rounded-md bg-surface-muted px-3 py-1.5 text-text-secondary">
                <dt>{{ item.label }}</dt>
                <dd class="max-w-[65%] truncate font-mono" :title="item.value">{{ item.value }}</dd>
              </div>
            </dl>
            <p v-else class="mt-1 text-text-tertiary">该历史记录未保存完整指纹。</p>
          </div>
          <div>
            <h4 class="font-medium text-text-primary">费用假设</h4>
            <dl class="mt-1 space-y-1 text-text-secondary">
              <div class="flex justify-between gap-3"><dt>双边佣金</dt><dd>{{ resultCosts.commission == null ? '未提供' : fmtPct(resultCosts.commission) }}</dd></div>
              <div class="flex justify-between gap-3"><dt>卖出印花税</dt><dd>{{ resultCosts.stamp_tax == null ? '未提供' : fmtPct(resultCosts.stamp_tax) }}</dd></div>
              <div class="flex justify-between gap-3"><dt>双边滑点</dt><dd>{{ resultCosts.slippage == null ? '未提供' : fmtPct(resultCosts.slippage) }}</dd></div>
            </dl>
          </div>
        </div>
        <details v-if="resultSpecJson" class="mt-4 border-t border-border-subtle pt-3">
          <summary class="flex cursor-pointer list-none items-center gap-2 text-xs font-medium text-text-secondary">
            <Code2 :size="14" />
            完整 StrategySpec 快照
          </summary>
          <pre class="mt-2 max-h-80 overflow-auto rounded-md bg-surface-muted p-3 text-xs leading-5 text-text-secondary">{{ resultSpecJson }}</pre>
        </details>
      </section>

      <section class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
        <div v-for="m in metrics" :key="m.label" class="rounded-lg border border-border bg-surface-raised p-3">
          <div class="text-xs text-text-tertiary">{{ m.label }}</div>
          <div class="mt-1 text-lg font-semibold" :class="m.cls">{{ m.value }}</div>
        </div>
      </section>

      <section
        v-if="resultValidation"
        class="rounded-lg border border-border bg-surface-raised p-4"
        aria-labelledby="spec-validation-report-heading"
      >
        <h3 id="spec-validation-report-heading" class="text-sm font-semibold">规格验证</h3>

        <div
          v-if="validationRejection"
          class="mt-3 rounded-md px-3 py-2 text-sm leading-5"
          :class="validationRejection.verdict === 'rejected'
            ? 'bg-up/10 text-up'
            : validationRejection.verdict === 'passed'
              ? 'bg-down/10 text-down'
              : 'bg-surface-muted text-text-secondary'"
        >
          <template v-if="validationRejection.verdict === 'rejected'">
            命中否决条件,策略已被标记为已否决:
            <ul class="mt-1 list-inside list-disc">
              <li v-for="hit in validationRejection.hits" :key="hit.criterion + hit.detail">
                {{ hit.detail }}
              </li>
            </ul>
          </template>
          <template v-else-if="validationRejection.verdict === 'passed'">
            全部否决条件均通过。
          </template>
          <template v-else>
            否决条件未命中,但有条件缺少数据未能评估:
          </template>
          <ul
            v-if="validationRejection.unevaluated.length"
            class="mt-1 list-inside list-disc text-xs"
          >
            <li v-for="item in validationRejection.unevaluated" :key="item.criterion">
              {{ item.criterion }}:{{ item.reason }}
            </li>
          </ul>
        </div>

        <div v-if="validationOos" class="mt-4">
          <h4 class="text-xs font-medium text-text-secondary">
            锁定样本外(最后 {{ Math.round((validationOos.fraction ?? 0.2) * 100) }}% 交易日)
          </h4>
          <table v-if="validationOos.available" class="mt-2 w-full max-w-xl text-sm">
            <thead class="border-b border-border text-left text-xs text-text-tertiary">
              <tr>
                <th class="py-1.5 pr-4 font-medium">指标</th>
                <th class="py-1.5 pr-4 text-right font-medium">样本内({{ validationOos.in_sample_bars }} 日)</th>
                <th class="py-1.5 text-right font-medium">样本外({{ validationOos.oos_bars }} 日,自 {{ validationOos.oos_start }})</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in oosMetricRows" :key="row.label" class="border-b border-border-subtle last:border-0">
                <td class="py-1.5 pr-4 text-text-secondary">{{ row.label }}</td>
                <td class="py-1.5 pr-4 text-right tabular-nums">{{ row.inSample }}</td>
                <td class="py-1.5 text-right tabular-nums">{{ row.oos }}</td>
              </tr>
            </tbody>
          </table>
          <p v-else class="mt-2 text-sm text-text-tertiary">
            {{ validationOos.message ?? '样本外段不可用' }}
          </p>
        </div>

        <div v-if="resultValidation.baselines.length" class="mt-4">
          <h4 class="text-xs font-medium text-text-secondary">对照基线(策略 − 基线)</h4>
          <div class="mt-2 overflow-x-auto">
            <table class="w-full min-w-[36rem] text-sm">
              <thead class="border-b border-border text-left text-xs text-text-tertiary">
                <tr>
                  <th class="py-1.5 pr-4 font-medium">基线</th>
                  <th class="py-1.5 pr-4 text-right font-medium">{{ metricName('total_return') }}</th>
                  <th class="py-1.5 pr-4 text-right font-medium">{{ metricName('annual_return') }}</th>
                  <th class="py-1.5 pr-4 text-right font-medium">{{ metricName('max_drawdown') }}</th>
                  <th class="py-1.5 pr-4 text-right font-medium">{{ metricName('sharpe') }}</th>
                  <th class="py-1.5 text-right font-medium">年化差值</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="baseline in resultValidation.baselines"
                  :key="baseline.baseline_id"
                  class="border-b border-border-subtle last:border-0"
                >
                  <td class="py-1.5 pr-4 text-text-secondary">{{ baseline.name }}</td>
                  <template v-if="baseline.status === 'ok' && baseline.metrics">
                    <td class="py-1.5 pr-4 text-right tabular-nums">{{ baseline.metrics.total_return == null ? '--' : fmtPct(baseline.metrics.total_return) }}</td>
                    <td class="py-1.5 pr-4 text-right tabular-nums">{{ baseline.metrics.annual_return == null ? '--' : fmtPct(baseline.metrics.annual_return) }}</td>
                    <td class="py-1.5 pr-4 text-right tabular-nums">{{ baseline.metrics.max_drawdown == null ? '--' : fmtPct(baseline.metrics.max_drawdown) }}</td>
                    <td class="py-1.5 pr-4 text-right tabular-nums">{{ baseline.metrics.sharpe == null ? '--' : fmtPrice(baseline.metrics.sharpe) }}</td>
                    <td
                      class="py-1.5 text-right tabular-nums"
                      :class="(baseline.delta?.annual_return ?? 0) >= 0 ? 'text-up' : 'text-down'"
                    >{{ baseline.delta?.annual_return == null ? '--' : fmtPct(baseline.delta.annual_return) }}</td>
                  </template>
                  <td v-else colspan="5" class="py-1.5 text-text-tertiary">
                    {{ baseline.message ?? '基线不可用' }}
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section class="rounded-lg border border-border bg-surface-raised p-2">
        <EChart :option="chartOption" height="380px" />
      </section>

      <section aria-labelledby="exit-reasons-heading">
        <h3 id="exit-reasons-heading" class="mb-2 text-base font-semibold">退出原因分布</h3>
        <div v-if="exitReasonRows.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
          <table class="min-w-[32rem] w-full text-sm">
            <thead class="border-b border-border bg-surface-muted text-left text-xs text-text-tertiary">
              <tr><th class="px-4 py-2 font-medium">退出原因</th><th class="px-4 py-2 text-right font-medium">主原因次数</th><th class="px-4 py-2 text-right font-medium">全部命中次数</th></tr>
            </thead>
            <tbody>
              <tr v-for="row in exitReasonRows" :key="row.reason" class="border-b border-border-subtle last:border-0">
                <td class="px-4 py-2">{{ row.reason_name || exitReasonName(row.reason) }}</td>
                <td class="px-4 py-2 text-right tabular-nums">{{ row.primary_count ?? row.count }}</td>
                <td class="px-4 py-2 text-right tabular-nums">{{ row.count }}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <p v-else class="rounded-md border border-dashed border-border px-4 py-5 text-sm text-text-tertiary">本次结果未提供退出原因统计；旧回测不会借用其他配置的数据。</p>
      </section>

      <section aria-labelledby="simulated-trades-heading">
        <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
          <h3 id="simulated-trades-heading" class="text-base font-semibold">逐笔模拟交易</h3>
          <span class="text-xs text-text-tertiary">T 日收盘形成信号，T+1 日开盘按可成交约束模拟成交</span>
        </div>
        <div v-if="resultTradeDetails.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
          <QuTable :data="resultTradeDetails" :columns="tradeColumns" :row-key="(trade, index) => `${trade.code}-${trade.execution_date}-${trade.side}-${index}`" class="min-w-[1080px]">
            <template #cell-stock="{ row: trade }">
              <div class="font-medium">{{ trade.name || stockName(trade.code) }}</div>
              <div class="text-xs text-text-tertiary">{{ trade.code }}</div>
            </template>
            <template #cell-side="{ row: trade }"><span class="font-medium">{{ trade.side === 'buy' ? '模拟进入' : '模拟退出' }}</span></template>
            <template #cell-execution_price="{ row: trade }">{{ fmtPrice(trade.execution_price) }}</template>
            <template #cell-size="{ row: trade }">{{ fmtPrice(trade.size) }}</template>
            <template #cell-fees="{ row: trade }">{{ fmtPrice(trade.fees) }}</template>
            <template #cell-all_reasons="{ row: trade }">
              <div class="flex flex-wrap gap-1">
                <span v-for="reason in trade.all_reasons" :key="reason.code" class="rounded bg-surface-muted px-1.5 py-0.5 text-xs">
                  {{ reason.name || exitReasonName(reason.code) }}<template v-if="reason.price_line != null"> · {{ fmtPrice(reason.price_line) }}</template>
                </span>
              </div>
            </template>
            <template #cell-realized_pnl="{ row: trade }">{{ fmtPrice(trade.realized_pnl) }}</template>
          </QuTable>
        </div>
        <p v-else class="rounded-md border border-dashed border-border px-4 py-5 text-sm text-text-tertiary">本次结果未提供逐笔模拟交易明细。</p>
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

      <p class="border-t border-border pt-3 text-xs leading-5 text-text-secondary">{{ RESEARCH_PLAN_BOUNDARY }}</p>
    </template>
  </div>
</template>
