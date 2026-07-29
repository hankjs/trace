<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Archive, FlaskConical, Plus, RefreshCw } from 'lucide-vue-next'
import {
  api,
  type EvidencePromotionTodo,
  type ExperimentDetail,
  type ExperimentSummary,
  type ExperimentTrial,
  type Strategy,
} from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import QuSelect from '../components/QuSelect.vue'
import QuDatePicker from '../components/QuDatePicker.vue'
import {
  expandParamColumns,
  formatParamPatch,
  objectiveValue,
  pickBestTrial,
  sortTrials,
  summarizeTrials,
  type CompareTrial,
  type ObjectiveKey,
  type SortState,
} from '../experimentCompare'
import { fmtPct } from '../format'
import { strategyById, useStrategies } from '../strategies'

const COL_PREF_KEY = 'quant.experiment.compare.columns'

const { load: loadStrategies, strategies } = useStrategies()
const items = ref<ExperimentSummary[]>([])
const selected = ref<ExperimentDetail | null>(null)
const loading = ref(false)
const error = ref('')
const notice = ref('')

const form = ref({
  title: '',
  hypothesis: '',
  permanent_candidate_id: '',
  strategy_id: null as number | null,
})

const trialForm = ref({
  codesText: 'sh.600519',
  start: '2023-01-01',
  end: '2024-12-31',
  paramPath: '',
  paramValue: '',
  batchJson: '[{}, {"$.native_exit.condition.right.window": 10}]',
})

const objective = ref<ObjectiveKey>('sharpe')
const objectiveOptions: { value: ObjectiveKey; label: string }[] = [
  { value: 'sharpe', label: '夏普' },
  { value: 'annual_return', label: '年化' },
  { value: 'total_return', label: '总收益' },
  { value: 'calmar', label: 'Calmar' },
  { value: 'max_drawdown', label: '回撤(越接近0越好)' },
]
/** 创建实验的关联策略下拉:null = 未选择 */
const strategyOptions = computed(() => [
  { value: null, label: '请选择' },
  ...strategies.value.map((s) => ({ value: s.id, label: `${s.name} (#${s.id})` })),
])
const sort = ref<SortState>({ key: 'sharpe', dir: 'desc' })
/** 单次/批量 trial 运行中,防双提交并给出反馈 */
const trialBusy = ref(false)

const columnPrefs = ref({
  win_rate: false,
  trade_count: true,
  backtest_id: true,
  error: true,
})

try {
  const raw = localStorage.getItem(COL_PREF_KEY)
  if (raw) columnPrefs.value = { ...columnPrefs.value, ...JSON.parse(raw) }
} catch { /* ignore */ }

watch(columnPrefs, (v) => {
  try { localStorage.setItem(COL_PREF_KEY, JSON.stringify(v)) } catch { /* ignore */ }
}, { deep: true })

const outcomeName: Record<string, string> = {
  ok: '完成',
  no_trades: '无交易',
  error: '失败',
  rejected: '否决',
}

const compareRows = computed<CompareTrial[]>(() =>
  (selected.value?.trials ?? []).map((t: ExperimentTrial) => ({
    id: t.id,
    trial_index: t.trial_index,
    outcome: t.outcome,
    param_patch: t.param_patch,
    metrics_summary: t.metrics_summary as Record<string, number | null | undefined> | null,
    backtest_run_id: t.backtest_run_id,
    error: t.error,
  })),
)

const sortedRows = computed(() => sortTrials(compareRows.value, sort.value))
const best = computed(() => pickBestTrial(compareRows.value, objective.value))
const summary = computed(() => summarizeTrials(compareRows.value, objective.value))
const paramCols = computed(() => expandParamColumns(compareRows.value))
const pendingPromotions = computed<EvidencePromotionTodo[]>(
  () => selected.value?.pending_promotions
    ?? (selected.value?.evidence_promotions ?? []).filter((p) => p.status === 'pending'),
)

const TARGET_NAMES: Record<string, string> = {
  backtested: '已回测（样本内）',
  oos_passed: '样本外否决条件通过',
  rejected: '已否决',
}

function targetName(t: string) {
  return TARGET_NAMES[t] ?? t
}

function describePromotionNotice(evalResult?: {
  eligible?: boolean
  block_reasons?: string[]
  todo?: EvidencePromotionTodo | null
  suggested_target?: string | null
} | null): string {
  if (!evalResult) return ''
  if (evalResult.todo?.status === 'pending') {
    return `系统提名证据推进 → ${targetName(evalResult.todo.suggested_target)}（待你确认）`
  }
  if (evalResult.eligible === false && evalResult.block_reasons?.length) {
    return `未达证据门槛：${evalResult.block_reasons[0]}`
  }
  return ''
}

function toggleSort(key: SortState['key']) {
  if (sort.value.key === key) {
    sort.value = { key, dir: sort.value.dir === 'desc' ? 'asc' : 'desc' }
  } else {
    // 指标默认 desc(含 max_drawdown: 负回撤数值越大越接近 0)
    sort.value = { key, dir: 'desc' }
  }
}

/** 收益/回撤/胜率为百分比小数;夏普/Calmar 为原值 */
function isPctMetric(key: string): boolean {
  return key === 'annual_return'
    || key === 'total_return'
    || key === 'max_drawdown'
    || key === 'win_rate'
}

function fmtMetric(v: number | null | undefined, pct = false): string {
  if (v == null || !Number.isFinite(v)) return '—'
  return pct ? fmtPct(v) : v.toFixed(3)
}

function fmtObjectiveMetric(v: number | null | undefined, key: ObjectiveKey = objective.value): string {
  return fmtMetric(v, isPctMetric(key))
}

async function refreshList() {
  loading.value = true
  error.value = ''
  try {
    const result = await api.listExperiments()
    items.value = result.items
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}

async function openExperiment(id: number) {
  error.value = ''
  try {
    selected.value = await api.getExperiment(id)
  } catch (e) {
    error.value = (e as Error).message
  }
}

async function create() {
  error.value = ''
  notice.value = ''
  const strategyId = form.value.strategy_id
  if (strategyId == null) {
    error.value = '请选择关联策略(trial 回测需要合法 strategy_id)'
    return
  }
  const strategy = strategyById(strategyId) as Strategy | undefined
  if (!strategy?.spec) {
    error.value = '所选策略没有可用规格'
    return
  }
  try {
    const created = await api.createExperiment({
      title: form.value.title,
      hypothesis: form.value.hypothesis,
      permanent_candidate_id: form.value.permanent_candidate_id,
      strategy_id: strategyId,
      spec: strategy.spec,
    })
    notice.value = `已创建实验 #${created.id}`
    form.value.title = ''
    form.value.hypothesis = ''
    form.value.permanent_candidate_id = ''
    await refreshList()
    await openExperiment(created.id)
  } catch (e) {
    error.value = (e as Error).message
  }
}

function parseCodes(): string[] {
  return trialForm.value.codesText
    .split(/[\s,，]+/)
    .map((c) => c.trim().toLowerCase())
    .filter(Boolean)
}

async function runTrial() {
  if (!selected.value || trialBusy.value) return
  error.value = ''
  notice.value = ''
  const codes = parseCodes()
  if (!codes.length) {
    error.value = '请至少填写一个股票代码'
    return
  }
  if (trialForm.value.start >= trialForm.value.end) {
    error.value = '开始日期必须早于结束日期'
    return
  }
  const param_patch: Record<string, number | string | boolean> = {}
  if (trialForm.value.paramPath) {
    const raw = trialForm.value.paramValue
    const num = Number(raw)
    param_patch[trialForm.value.paramPath] = Number.isFinite(num) && raw !== '' ? num : raw
  }
  trialBusy.value = true
  notice.value = '正在运行试验…'
  try {
    const result = await api.createExperimentTrial(selected.value.id, {
      codes,
      start: trialForm.value.start,
      end: trialForm.value.end,
      param_patch,
    })
    const base = `试验 #${result.trial.trial_index} 结果: ${outcomeName[result.trial.outcome] ?? result.trial.outcome}`
    const promoNote = describePromotionNotice(result.promotion)
    notice.value = promoNote ? `${base} · ${promoNote}` : base
    await openExperiment(selected.value.id)
  } catch (e) {
    notice.value = ''
    error.value = (e as Error).message
  } finally {
    trialBusy.value = false
  }
}

async function runBatch() {
  if (!selected.value || trialBusy.value) return
  error.value = ''
  notice.value = ''
  const codes = parseCodes()
  if (!codes.length) {
    error.value = '请至少填写一个股票代码'
    return
  }
  if (trialForm.value.start >= trialForm.value.end) {
    error.value = '开始日期必须早于结束日期'
    return
  }
  let patches: Array<Record<string, number | string | boolean>>
  try {
    const parsed = JSON.parse(trialForm.value.batchJson)
    if (!Array.isArray(parsed) || !parsed.length) {
      error.value = 'batch 须为非空 JSON 数组(每项为 param_patch 对象)'
      return
    }
    if (parsed.length > 32) {
      error.value = '单批最多 32 个 param_patch'
      return
    }
    patches = parsed.map((p) => (p && typeof p === 'object' ? p as Record<string, number | string | boolean> : {}))
  } catch {
    error.value = 'batch JSON 解析失败'
    return
  }
  trialBusy.value = true
  notice.value = `正在批量运行 ${patches.length} 个 trial…`
  try {
    const result = await api.createExperimentTrialsBatch(selected.value.id, {
      codes,
      start: trialForm.value.start,
      end: trialForm.value.end,
      param_patches: patches,
    })
    const counts = { ok: 0, no_trades: 0, error: 0, rejected: 0, other: 0 }
    for (const item of result.items ?? []) {
      const outcome = item.trial?.outcome
      if (outcome === 'ok') counts.ok += 1
      else if (outcome === 'no_trades') counts.no_trades += 1
      else if (outcome === 'error') counts.error += 1
      else if (outcome === 'rejected') counts.rejected += 1
      else counts.other += 1
    }
    const pendingN = result.pending_promotions?.length
      ?? result.items.filter((i) => i.promotion?.todo?.status === 'pending').length
    notice.value = `批量完成 ${result.count} 个：完成 ${counts.ok} / 无交易 ${counts.no_trades} / 失败 ${counts.error}`
      + (counts.rejected ? ` / 否决 ${counts.rejected}` : '')
      + (pendingN ? ` · 新增 ${pendingN} 条证据推进待办` : '')
    await openExperiment(selected.value.id)
  } catch (e) {
    notice.value = ''
    error.value = (e as Error).message
  } finally {
    trialBusy.value = false
  }
}

async function acceptPromotion(todo: EvidencePromotionTodo) {
  if (trialBusy.value) return
  trialBusy.value = true
  error.value = ''
  notice.value = '正在写入证据状态…'
  try {
    const result = await api.acceptEvidencePromotion(todo.id)
    const tr = result.evidence_transition
    notice.value = tr
      ? `已采纳：证据 ${tr.from} → ${tr.to}`
      : '已采纳（状态机未再前进，可能已是更高状态）'
    if (selected.value) await openExperiment(selected.value.id)
    await loadStrategies(true)
  } catch (e) {
    notice.value = ''
    error.value = (e as Error).message
  } finally {
    trialBusy.value = false
  }
}

async function dismissPromotion(todo: EvidencePromotionTodo) {
  if (trialBusy.value) return
  trialBusy.value = true
  error.value = ''
  try {
    await api.dismissEvidencePromotion(todo.id)
    notice.value = '已忽略该推进待办（试验账本保留）'
    if (selected.value) await openExperiment(selected.value.id)
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    trialBusy.value = false
  }
}

async function archiveSelected() {
  if (!selected.value || trialBusy.value) return
  error.value = ''
  try {
    await api.archiveExperiment(selected.value.id)
    notice.value = '已归档实验(trial 仍保留)'
    selected.value = null
    await refreshList()
  } catch (e) {
    error.value = (e as Error).message
  }
}

onMounted(async () => {
  await loadStrategies()
  await refreshList()
})
</script>

<template>
  <div class="space-y-4">
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice" tone="info">{{ notice }}</InlineFeedback>

    <section class="terminal-panel space-y-3 p-4">
      <div class="flex items-center gap-2">
        <FlaskConical :size="16" />
        <h2 class="text-sm font-semibold">新建实验</h2>
      </div>
      <p class="text-xs text-text-tertiary">
        冻结当前策略规格为试验族。所有 trial(含失败)永久保留,用于回答「一共试过多少变体」。
      </p>
      <div class="grid gap-2 md:grid-cols-2">
        <label class="text-xs">
          标题
          <input v-model="form.title" class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
        </label>
        <label class="text-xs">
          永久候选 ID
          <input
            v-model="form.permanent_candidate_id"
            placeholder="CAN-TRD-01"
            class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm"
          />
        </label>
        <label class="text-xs md:col-span-2">
          假设
          <textarea
            v-model="form.hypothesis"
            rows="2"
            class="mt-1 w-full rounded-md border border-border bg-surface px-2.5 py-2 text-sm"
          />
        </label>
        <label class="text-xs md:col-span-2">
          关联策略(冻结其当前规格)
          <QuSelect
            v-model="form.strategy_id"
            :options="strategyOptions"
            class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm"
          />
        </label>
      </div>
      <div>
        <button type="button" class="btn btn-primary" @click="create">
          <Plus :size="14" /> 创建实验
        </button>
      </div>
    </section>

    <div class="grid gap-4 lg:grid-cols-[minmax(260px,0.9fr)_minmax(0,1.4fr)]">
      <section class="terminal-panel min-h-[280px]">
        <div class="terminal-panel-header">
          <h2 class="text-sm font-semibold">实验列表</h2>
          <button type="button" class="icon-button !h-8 !w-8" :disabled="loading" aria-label="刷新列表" @click="refreshList">
            <RefreshCw :size="14" :class="loading ? 'animate-spin' : ''" />
          </button>
        </div>
        <ul v-if="items.length" class="divide-y divide-border-subtle">
          <li
            v-for="item in items"
            :key="item.id"
            class="cursor-pointer px-3 py-2 text-sm hover:bg-surface-muted"
            :class="selected?.id === item.id ? 'bg-info-soft' : ''"
            @click="openExperiment(item.id)"
          >
            <div class="font-medium">{{ item.title }}</div>
            <div class="text-[11px] text-text-tertiary">
              {{ item.permanent_candidate_id }} · {{ item.status }} · trial {{ item.trial_count ?? 0 }}
            </div>
          </li>
        </ul>
        <p v-else class="px-3 py-6 text-center text-xs text-text-tertiary">暂无实验</p>
      </section>

      <section v-if="selected" class="terminal-panel space-y-3 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div>
            <h2 class="text-sm font-semibold">{{ selected.title }}</h2>
            <p class="mt-1 text-xs text-text-secondary">{{ selected.hypothesis }}</p>
            <p class="mt-1 text-[11px] text-text-tertiary">
              身份哈希 {{ selected.identity_hash.slice(0, 12) }}… · 规格 {{ selected.frozen_spec_hash.slice(0, 12) }}…
            </p>
          </div>
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            :disabled="selected.status === 'archived' || trialBusy"
            @click="archiveSelected"
          >
            <Archive :size="14" /> 归档
          </button>
        </div>

        <!-- 证据推进待办：系统达标提名，用户确认才改 evidence_status -->
        <div
          v-if="pendingPromotions.length"
          class="rounded border border-accent/40 bg-info-soft px-3 py-2 text-xs leading-5 text-text-secondary"
        >
          <strong class="text-text-primary">证据推进待办</strong>
          <span class="text-text-tertiary"> · 试验不自动改状态；质量未达标不会出现在此</span>
          <ul class="mt-2 space-y-2">
            <li
              v-for="todo in pendingPromotions"
              :key="todo.id"
              class="flex flex-wrap items-start justify-between gap-2 rounded border border-border bg-surface px-2 py-1.5"
            >
              <div class="min-w-0">
                <div class="font-medium text-text-primary">
                  建议 → {{ targetName(todo.suggested_target) }}
                  <span class="font-normal text-text-tertiary">
                    · trial #{{ selected?.trials.find((t) => t.id === todo.trial_id)?.trial_index ?? todo.trial_id }}
                    · run {{ todo.backtest_run_id }}
                  </span>
                </div>
                <p class="mt-0.5 text-[11px] text-text-tertiary">
                  通过门槛 ≠ 策略有效。采纳将写入策略证据状态机。
                </p>
              </div>
              <div class="flex shrink-0 gap-1.5">
                <button
                  type="button"
                  class="btn btn-primary btn-sm"
                  :disabled="trialBusy"
                  @click="acceptPromotion(todo)"
                >采纳为证据</button>
                <button
                  type="button"
                  class="btn btn-ghost btn-sm"
                  :disabled="trialBusy"
                  @click="dismissPromotion(todo)"
                >忽略</button>
              </div>
            </li>
          </ul>
        </div>
        <p v-else class="text-[11px] text-text-tertiary">
          试验账本不自动推进证据。基准 trial 达标后会出现「证据推进待办」供你确认；质量差（无交易/样本不足/未设计完成等）由系统拦截。
        </p>

        <!-- 对比摘要卡 -->
        <div class="rounded border border-border bg-surface-muted px-3 py-2 text-xs leading-5 text-text-secondary">
          <strong class="text-text-primary">对比摘要</strong>
          · 共 {{ summary.total }}
          · ok {{ summary.ok }}
          · 无交易 {{ summary.no_trades }}
          · 失败 {{ summary.error }}
          · 否决 {{ summary.rejected }}
          <template v-if="summary.best_trial_index != null">
            · 最优 #{{ summary.best_trial_index }} ({{ objective }}={{ fmtObjectiveMetric(summary.best_value) }})
          </template>
          <template v-if="summary.min != null">
            · ok 子集 min/median/max {{ fmtObjectiveMetric(summary.min) }} / {{ fmtObjectiveMetric(summary.median) }} / {{ fmtObjectiveMetric(summary.max) }}
          </template>
        </div>

        <div
          v-if="selected.multiplicity"
          class="rounded border border-border bg-surface-muted px-3 py-2 text-xs leading-5 text-text-secondary"
        >
          <strong class="text-text-primary">多重检验提示</strong>
          · 试验 {{ selected.multiplicity.n_trials }} 组
          <template v-if="selected.multiplicity.best_metric != null">
            · 最优指标 {{ selected.multiplicity.best_metric }}
          </template>
          <p class="mt-1 text-text-tertiary">{{ selected.multiplicity.disclaimer }}</p>
        </div>

        <div class="flex flex-wrap items-center gap-3 text-xs">
          <label class="flex items-center gap-1">
            优化目标
            <QuSelect v-model="objective" :options="objectiveOptions" class="h-8 rounded-md border border-border bg-surface px-2 text-xs" />
          </label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.win_rate" type="checkbox" /> 胜率</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.trade_count" type="checkbox" /> 交易数</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.backtest_id" type="checkbox" /> 回测 ID</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.error" type="checkbox" /> 错误</label>
        </div>

        <div class="grid gap-2 md:grid-cols-2">
          <label class="text-xs">
            代码(逗号分隔)
            <input v-model="trialForm.codesText" class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
          </label>
          <label class="text-xs">
            区间
            <div class="mt-1 flex gap-1">
              <QuDatePicker v-model="trialForm.start" class="h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
              <QuDatePicker v-model="trialForm.end" class="h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
            </div>
          </label>
          <label class="text-xs">
            参数路径(可选)
            <input v-model="trialForm.paramPath" class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
          </label>
          <label class="text-xs">
            参数值
            <input v-model="trialForm.paramValue" class="mt-1 h-9 w-full rounded-md border border-border bg-surface px-2.5 text-sm" />
          </label>
          <label class="text-xs md:col-span-2">
            批量 param_patches JSON(最多 32)
            <textarea
              v-model="trialForm.batchJson"
              rows="2"
              class="mt-1 w-full rounded-md border border-border bg-surface px-2.5 py-2 font-mono text-xs"
            />
          </label>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="btn btn-primary"
            :disabled="selected.status === 'archived' || trialBusy"
            @click="runTrial"
          >
            <RefreshCw v-if="trialBusy" :size="14" class="animate-spin" />
            {{ trialBusy ? '运行中…' : '运行试验' }}
          </button>
          <button
            type="button"
            class="btn btn-secondary"
            :disabled="selected.status === 'archived' || trialBusy"
            @click="runBatch"
          >
            批量运行
          </button>
        </div>

        <div class="overflow-x-auto">
          <table class="terminal-table">
            <thead>
              <tr>
                <th class="cursor-pointer" @click="toggleSort('trial_index')">#</th>
                <th class="cursor-pointer" @click="toggleSort('outcome')">结果</th>
                <template v-if="paramCols.mode === 'columns'">
                  <th v-for="k in paramCols.keys" :key="k">{{ k.replace(/^\$\./, '') }}</th>
                </template>
                <th v-else>参数</th>
                <th class="cursor-pointer" @click="toggleSort('total_return')">总收益</th>
                <th class="cursor-pointer" @click="toggleSort('annual_return')">年化</th>
                <th class="cursor-pointer" @click="toggleSort('max_drawdown')">回撤</th>
                <th class="cursor-pointer" @click="toggleSort('sharpe')">夏普</th>
                <th v-if="columnPrefs.win_rate">胜率</th>
                <th v-if="columnPrefs.trade_count">交易/往返</th>
                <th v-if="columnPrefs.backtest_id">回测</th>
                <th v-if="columnPrefs.error">错误</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="t in sortedRows"
                :key="t.id"
                :class="{
                  'bg-info-soft/60 font-medium': best && t.id === best.id,
                  'opacity-60': t.outcome !== 'ok',
                }"
              >
                <td>{{ t.trial_index }}</td>
                <td>
                  {{ outcomeName[t.outcome] ?? t.outcome }}
                  <span v-if="!Object.keys(t.param_patch || {}).length" class="ml-1 text-[10px] text-text-tertiary">基准</span>
                </td>
                <template v-if="paramCols.mode === 'columns'">
                  <td v-for="k in paramCols.keys" :key="k" class="font-mono text-[11px]">
                    {{ t.param_patch && k in t.param_patch ? JSON.stringify(t.param_patch[k]) : '—' }}
                  </td>
                </template>
                <td v-else class="max-w-[10rem] whitespace-pre-wrap font-mono text-[10px] text-text-tertiary">
                  {{ formatParamPatch(t.param_patch) }}
                </td>
                <td>{{ fmtMetric(objectiveValue(t, 'total_return'), true) }}</td>
                <td>{{ fmtMetric(objectiveValue(t, 'annual_return'), true) }}</td>
                <td>{{ fmtMetric(objectiveValue(t, 'max_drawdown'), true) }}</td>
                <td>{{ fmtMetric(objectiveValue(t, 'sharpe')) }}</td>
                <td v-if="columnPrefs.win_rate">{{ fmtMetric(t.metrics_summary?.win_rate as number | null, true) }}</td>
                <td v-if="columnPrefs.trade_count">
                  {{ t.metrics_summary?.trade_count ?? '—' }} / {{ t.metrics_summary?.round_trips ?? '—' }}
                </td>
                <td v-if="columnPrefs.backtest_id">{{ t.backtest_run_id ?? '—' }}</td>
                <td
                  v-if="columnPrefs.error"
                  class="max-w-[10rem] truncate text-text-tertiary"
                  :title="t.error || ''"
                >{{ t.error || '—' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section v-else class="terminal-panel flex min-h-[280px] items-center justify-center p-6 text-xs text-text-tertiary">
        选择左侧实验查看 trial 对比表
      </section>
    </div>
  </div>
</template>
