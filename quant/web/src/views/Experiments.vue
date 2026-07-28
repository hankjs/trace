<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { Archive, FlaskConical, Plus, RefreshCw } from 'lucide-vue-next'
import {
  api,
  type ExperimentDetail,
  type ExperimentSummary,
  type Strategy,
} from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import { fmtPct } from '../format'
import { strategyById, useStrategies } from '../strategies'

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
})

const outcomeName: Record<string, string> = {
  ok: '完成',
  no_trades: '无交易',
  error: '失败',
  rejected: '否决',
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

async function runTrial() {
  if (!selected.value) return
  error.value = ''
  notice.value = ''
  const codes = trialForm.value.codesText
    .split(/[\s,，]+/)
    .map((c) => c.trim().toLowerCase())
    .filter(Boolean)
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
            参数路径(可选,如 $.native_exit.condition.right.value)
            <input v-model="trialForm.paramPath" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
          </label>
          <label class="text-xs">
            参数值
            <input v-model="trialForm.paramValue" class="mt-1 w-full rounded border border-border bg-surface px-2 py-1.5 text-sm" />
          </label>
        </div>
        <button
          type="button"
          class="workspace-command"
          :disabled="selected.status === 'archived'"
          @click="runTrial"
        >
          运行试验
        </button>

        <table class="w-full text-left text-xs">
          <thead class="text-text-tertiary">
            <tr>
              <th class="py-1">#</th>
              <th>结果</th>
              <th>回测</th>
              <th>收益</th>
              <th>回撤</th>
              <th>错误</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="t in selected.trials" :key="t.id" class="border-t border-border-subtle">
              <td class="py-1.5">{{ t.trial_index }}</td>
              <td>{{ outcomeName[t.outcome] ?? t.outcome }}</td>
              <td>{{ t.backtest_run_id ?? '—' }}</td>
              <td>{{ t.metrics_summary?.total_return == null ? '—' : fmtPct(t.metrics_summary.total_return) }}</td>
              <td>{{ t.metrics_summary?.max_drawdown == null ? '—' : fmtPct(t.metrics_summary.max_drawdown) }}</td>
              <td class="max-w-[12rem] truncate text-text-tertiary" :title="t.error || ''">{{ t.error || '—' }}</td>
            </tr>
          </tbody>
        </table>
      </section>

      <section v-else class="terminal-panel flex min-h-[280px] items-center justify-center p-6 text-xs text-text-tertiary">
        选择左侧实验查看 trial 账本
      </section>
    </div>
  </div>
</template>
