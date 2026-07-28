<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Archive, FlaskConical, Plus, RefreshCw } from 'lucide-vue-next'
import {
  api,
  type ExperimentDetail,
  type ExperimentSummary,
  type ExperimentTrial,
  type Strategy,
} from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
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
const sort = ref<SortState>({ key: 'sharpe', dir: 'desc' })

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

function toggleSort(key: SortState['key']) {
  if (sort.value.key === key) {
    sort.value = { key, dir: sort.value.dir === 'desc' ? 'asc' : 'desc' }
  } else {
    sort.value = { key, dir: key === 'max_drawdown' ? 'desc' : 'desc' }
  }
}

function fmtMetric(v: number | null | undefined, pct = false): string {
  if (v == null || !Number.isFinite(v)) return '—'
  return pct ? fmtPct(v) : v.toFixed(3)
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
  if (!selected.value) return
  error.value = ''
  notice.value = ''
  const codes = parseCodes()
  const param_patch: Record<string, number | string | boolean> = {}
  if (trialForm.value.paramPath) {
    const raw = trialForm.value.paramValue
    const num = Number(raw)
    param_patch[trialForm.value.paramPath] = Number.isFinite(num) && raw !== '' ? num : raw
  }
  try {
    const result = await api.createExperimentTrial(selected.value.id, {
      codes,
      start: trialForm.value.start,
      end: trialForm.value.end,
      param_patch,
    })
    notice.value = `试验 #${result.trial.trial_index} 结果: ${outcomeName[result.trial.outcome] ?? result.trial.outcome}`
    await openExperiment(selected.value.id)
  } catch (e) {
    error.value = (e as Error).message
  }
}

async function runBatch() {
  if (!selected.value) return
  error.value = ''
  notice.value = ''
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
  try {
    const result = await api.createExperimentTrialsBatch(selected.value.id, {
      codes: parseCodes(),
      start: trialForm.value.start,
      end: trialForm.value.end,
      param_patches: patches,
    })
    notice.value = `批量完成 ${result.count} 个 trial`
    await openExperiment(selected.value.id)
  } catch (e) {
    error.value = (e as Error).message
  }
}

async function archiveSelected() {
  if (!selected.value) return
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
    <InlineFeedback v-if="error" tone="warning">{{ error }}</InlineFeedback>
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
          <input v-model="form.title" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
        </label>
        <label class="text-xs">
          永久候选 ID
          <input
            v-model="form.permanent_candidate_id"
            placeholder="CAN-TRD-01"
            class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm"
          />
        </label>
        <label class="text-xs md:col-span-2">
          假设
          <textarea
            v-model="form.hypothesis"
            rows="2"
            class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm"
          />
        </label>
        <label class="text-xs md:col-span-2">
          关联策略(冻结其当前规格)
          <select
            v-model.number="form.strategy_id"
            class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm"
          >
            <option :value="null">请选择</option>
            <option v-for="s in strategies" :key="s.id" :value="s.id">
              {{ s.name }} (#{{ s.id }})
            </option>
          </select>
        </label>
      </div>
      <button type="button" class="workspace-command" @click="create">
        <Plus :size="14" /> 创建实验
      </button>
    </section>

    <div class="grid gap-4 lg:grid-cols-[minmax(260px,0.9fr)_minmax(0,1.4fr)]">
      <section class="terminal-panel min-h-[280px]">
        <div class="terminal-panel-header">
          <h2 class="text-sm font-semibold">实验列表</h2>
          <button type="button" class="workspace-command" :disabled="loading" @click="refreshList">
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
            class="workspace-command"
            :disabled="selected.status === 'archived'"
            @click="archiveSelected"
          >
            <Archive :size="14" /> 归档
          </button>
        </div>

        <!-- 对比摘要卡 -->
        <div class="rounded border border-border bg-surface-muted px-3 py-2 text-xs leading-5 text-text-secondary">
          <strong class="text-text-primary">对比摘要</strong>
          · 共 {{ summary.total }}
          · ok {{ summary.ok }}
          · 无交易 {{ summary.no_trades }}
          · 失败 {{ summary.error }}
          · 否决 {{ summary.rejected }}
          <template v-if="summary.best_trial_index != null">
            · 最优 #{{ summary.best_trial_index }} ({{ objective }}={{ fmtMetric(summary.best_value) }})
          </template>
          <template v-if="summary.min != null">
            · ok 子集 min/median/max {{ fmtMetric(summary.min) }} / {{ fmtMetric(summary.median) }} / {{ fmtMetric(summary.max) }}
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
            <select v-model="objective" class="rounded border border-border bg-surface px-1.5 py-1">
              <option value="sharpe">夏普</option>
              <option value="annual_return">年化</option>
              <option value="total_return">总收益</option>
              <option value="calmar">Calmar</option>
              <option value="max_drawdown">回撤(越接近0越好)</option>
            </select>
          </label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.win_rate" type="checkbox" /> 胜率</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.trade_count" type="checkbox" /> 交易数</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.backtest_id" type="checkbox" /> 回测 ID</label>
          <label class="flex items-center gap-1"><input v-model="columnPrefs.error" type="checkbox" /> 错误</label>
        </div>

        <div class="grid gap-2 md:grid-cols-2">
          <label class="text-xs">
            代码(逗号分隔)
            <input v-model="trialForm.codesText" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
          </label>
          <label class="text-xs">
            区间
            <div class="mt-1 flex gap-1">
              <input v-model="trialForm.start" type="date" class="w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
              <input v-model="trialForm.end" type="date" class="w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
            </div>
          </label>
          <label class="text-xs">
            参数路径(可选)
            <input v-model="trialForm.paramPath" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
          </label>
          <label class="text-xs">
            参数值
            <input v-model="trialForm.paramValue" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
          </label>
          <label class="text-xs md:col-span-2">
            批量 param_patches JSON(最多 32)
            <textarea
              v-model="trialForm.batchJson"
              rows="2"
              class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 font-mono text-xs"
            />
          </label>
        </div>
        <div class="flex flex-wrap gap-2">
          <button
            type="button"
            class="workspace-command"
            :disabled="selected.status === 'archived'"
            @click="runTrial"
          >
            运行试验
          </button>
          <button
            type="button"
            class="workspace-command"
            :disabled="selected.status === 'archived'"
            @click="runBatch"
          >
            批量运行
          </button>
        </div>

        <div class="overflow-x-auto">
          <table class="w-full text-left text-xs">
            <thead class="text-text-tertiary">
              <tr>
                <th class="cursor-pointer py-1" @click="toggleSort('trial_index')">#</th>
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
                class="border-t border-border-subtle"
                :class="{
                  'bg-info-soft/60 font-medium': best && t.id === best.id,
                  'opacity-60': t.outcome !== 'ok',
                }"
              >
                <td class="py-1.5">{{ t.trial_index }}</td>
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
