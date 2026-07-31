<script setup lang="ts">
/**
 * A2A 缺口排行(admin 专属):聚合审计缺口列与 Agent findings。
 *
 * - 顶部说明卡强调「聚合数据,非 LLM 建议」
 * - since_days 切换 7/30/90 天
 * - 三个区块:合并排行(主表)、审计缺口、Agent findings
 */
import { onMounted, ref, watch } from 'vue'
import { AlertTriangle, RefreshCw } from 'lucide-vue-next'
import { api, type GapItem, type GapSummaryResponse } from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import QuTable from '../components/QuTable.vue'
import { useAsyncAction } from '../useAsyncAction'

const sinceDays = ref(30)
const loading = ref(true)
const data = ref<GapSummaryResponse | null>(null)
const { busy, error, notice, fail, run } = useAsyncAction()

interface SinceOption {
  label: string
  value: number
}

const sinceOptions: SinceOption[] = [
  { label: '近 7 天', value: 7 },
  { label: '近 30 天', value: 30 },
  { label: '近 90 天', value: 90 },
]

const mergedColumns = [
  { key: 'missing_capability', label: '缺失能力 / 详情' },
  { key: 'failure_kind', label: '失败类别' },
  { key: 'count', label: '出现次数', align: 'right' as const },
]

const detailColumns = [
  { key: 'missing_capability', label: '缺失能力 / 详情' },
  { key: 'failure_kind', label: '失败类别' },
  { key: 'count', label: '出现次数', align: 'right' as const },
  { key: 'last_seen', label: '最近出现' },
]

async function load() {
  try {
    data.value = await api.adminA2aGaps(20, sinceDays.value)
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  } finally {
    loading.value = false
  }
}

async function refresh() {
  await run(async () => {
    loading.value = true
    await load()
    return true
  }, { success: '已刷新缺口排行。' })
}

watch(sinceDays, () => void load(), { immediate: false })

onMounted(() => void load())

function isEmpty(items: GapItem[] | undefined): boolean {
  return !items || items.length === 0
}
</script>

<template>
  <div class="space-y-4">
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-else-if="notice">{{ notice }}</InlineFeedback>

    <section class="rounded border border-border bg-surface-raised">
      <div class="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <AlertTriangle :size="15" class="text-accent" />
        <h2 class="text-sm font-medium text-text-primary">A2A 缺口排行</h2>
        <button
          type="button"
          class="icon-button ml-auto"
          title="刷新"
          :disabled="loading || busy"
          @click="refresh"
        >
          <RefreshCw :size="14" :class="{ 'animate-spin': loading || busy }" />
          <span class="sr-only">刷新</span>
        </button>
      </div>
      <div class="space-y-2 px-3 py-2.5 text-xs leading-5 text-text-secondary">
        <p>
          本页聚合两类系统信号:<strong class="text-text-primary">A2A 审计缺口列</strong>
          (validate 失败与运行期失败写入的 failure_kind / missing_capability)与
          <strong class="text-text-primary">Agent findings</strong>
          (Orchestrator Conclude 步骤落表的系统缺口)。
        </p>
        <p class="text-text-tertiary">
          排行仅反映「哪些能力不足/哪些失败反复出现」,供人工评估补强优先级;
          不是 LLM 给出的优化建议,也不自动触发任何系统变更。
        </p>
      </div>
    </section>

    <section class="rounded border border-border bg-surface-raised">
      <div class="flex items-center justify-between gap-3 border-b border-border px-3 py-2.5">
        <h3 class="text-sm font-medium text-text-primary">合并排行</h3>
        <div class="inline-flex rounded border border-border bg-surface p-0.5">
          <button
            v-for="opt in sinceOptions"
            :key="opt.value"
            type="button"
            class="px-2.5 py-1 text-xs transition-colors"
            :class="sinceDays === opt.value
              ? 'rounded bg-active text-text-primary font-medium'
              : 'text-text-secondary hover:text-text-primary'"
            @click="sinceDays = opt.value"
          >
            {{ opt.label }}
          </button>
        </div>
      </div>
      <div class="overflow-x-auto">
        <QuTable
          v-if="!loading && data && !isEmpty(data.merged)"
          :data="data.merged"
          :columns="mergedColumns"
          body-cell-class="px-3 py-2"
          header-cell-class="px-3 py-2 font-medium"
        />
        <div v-else-if="loading" class="px-3 py-6 text-sm text-text-tertiary">加载中…</div>
        <div v-else class="px-3 py-8 text-center text-sm text-text-tertiary">
          <AlertTriangle :size="24" class="mx-auto mb-2 text-text-tertiary" />
          暂无缺口记录
        </div>
      </div>
    </section>

    <div class="grid gap-4 lg:grid-cols-2">
      <section class="overflow-hidden rounded border border-border bg-surface-raised">
        <div class="border-b border-border px-3 py-2.5">
          <h3 class="text-sm font-medium text-text-primary">审计缺口</h3>
        </div>
        <div class="overflow-x-auto">
          <QuTable
            v-if="!loading && data && !isEmpty(data.audit_items)"
            :data="data.audit_items"
            :columns="detailColumns"
            body-cell-class="px-3 py-2"
            header-cell-class="px-3 py-2 font-medium"
          />
          <div v-else-if="loading" class="px-3 py-6 text-sm text-text-tertiary">加载中…</div>
          <div v-else class="px-3 py-8 text-center text-sm text-text-tertiary">暂无审计缺口记录</div>
        </div>
      </section>

      <section class="overflow-hidden rounded border border-border bg-surface-raised">
        <div class="border-b border-border px-3 py-2.5">
          <h3 class="text-sm font-medium text-text-primary">Agent findings</h3>
        </div>
        <div class="overflow-x-auto">
          <QuTable
            v-if="!loading && data && !isEmpty(data.finding_items)"
            :data="data.finding_items"
            :columns="detailColumns"
            body-cell-class="px-3 py-2"
            header-cell-class="px-3 py-2 font-medium"
          />
          <div v-else-if="loading" class="px-3 py-6 text-sm text-text-tertiary">加载中…</div>
          <div v-else class="px-3 py-8 text-center text-sm text-text-tertiary">暂无 findings 记录</div>
        </div>
      </section>
    </div>
  </div>
</template>
