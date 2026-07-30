<script setup lang="ts">
/**
 * 定时任务(admin 专属):查看调度状态、手动触发一次执行、翻看执行历史。
 *
 * 手动触发不绕过任务内部守卫:非交易日、盘中时间窗外等场景任务会自行
 * 跳过,表现为「已完成」但结果里 skipped/无输出。执行记录持久化在
 * quant_job_run,服务重启后保留;进程中途崩溃遗留的「执行中」会在
 * 下次触发时自动收尾为失败。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { ChevronDown, ChevronRight, Loader2, Play, RefreshCw, Timer } from 'lucide-vue-next'
import { api, type AdminJob, type AdminJobRun } from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import { useAsyncAction } from '../useAsyncAction'

const loading = ref(true)
const schedulerRunning = ref(false)
const jobs = ref<AdminJob[]>([])
const { busy, error, notice, fail, run } = useAsyncAction()

const POLL_MS = 3000
let pollTimer: ReturnType<typeof setInterval> | null = null

const expandedId = ref<string | null>(null)
const history = ref<AdminJobRun[]>([])
const historyLoading = ref(false)

const hasRunning = computed(() =>
  jobs.value.some((job) => job.manual_run?.status === 'running'),
)

async function load() {
  try {
    const res = await api.adminJobs()
    schedulerRunning.value = res.scheduler_running
    jobs.value = res.jobs
    // 展开中的历史随轮询一起刷新,手动执行完成后能立即看到新记录
    if (expandedId.value) await loadHistory(expandedId.value)
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  } finally {
    loading.value = false
  }
}

function syncPolling() {
  if (hasRunning.value && pollTimer === null) {
    pollTimer = setInterval(() => void load(), POLL_MS)
  } else if (!hasRunning.value && pollTimer !== null) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

async function loadHistory(jobId: string) {
  historyLoading.value = true
  try {
    history.value = await api.adminJobRuns(jobId)
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  } finally {
    historyLoading.value = false
  }
}

async function toggleHistory(job: AdminJob) {
  if (expandedId.value === job.id) {
    expandedId.value = null
    return
  }
  expandedId.value = job.id
  await loadHistory(job.id)
}

async function trigger(job: AdminJob) {
  const hint = job.id === 'evening_pipeline'
    ? '盘后流水线包含日线采集、选股与信号计算,可能耗时数十分钟。'
    : ''
  if (!window.confirm(`立即执行「${job.name}」一次？${hint}`)) return
  const ok = await run(async () => {
    await api.runAdminJob(job.id)
    await load()
    return true
  }, { success: `已启动「${job.name}」,执行状态见下表。` })
  if (ok === undefined) return
  syncPolling()
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '—'
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  return d.toLocaleString('zh-CN', { hour12: false })
}

function runSummary(record: AdminJobRun): string {
  if (record.status === 'running') return '执行中…'
  if (record.status === 'failed') return `失败: ${record.error ?? '未知错误'}`
  if (record.result == null) return '已完成'
  try {
    const text = JSON.stringify(record.result)
    return text.length > 120 ? `${text.slice(0, 120)}…` : text
  } catch {
    return '已完成'
  }
}

function statusClass(record: AdminJobRun): string {
  if (record.status === 'running') return 'text-text-secondary'
  if (record.status === 'failed') return 'text-up'
  return 'text-text-primary'
}

onMounted(async () => {
  await load()
  syncPolling()
})

onBeforeUnmount(() => {
  if (pollTimer !== null) clearInterval(pollTimer)
})
</script>

<template>
  <div class="space-y-4">
    <InlineFeedback :error="error" :notice="notice" />

    <section class="rounded border border-border bg-surface-raised">
      <div class="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <Timer :size="15" class="text-accent" />
        <h2 class="text-sm font-medium text-text-primary">调度状态</h2>
        <button
          type="button"
          class="icon-button ml-auto"
          title="刷新"
          :disabled="loading"
          @click="load"
        >
          <RefreshCw :size="14" :class="{ 'animate-spin': loading }" />
          <span class="sr-only">刷新</span>
        </button>
      </div>
      <div class="px-3 py-2.5 text-xs leading-5 text-text-secondary">
        <p v-if="schedulerRunning">
          本进程正在运行调度器,任务将按「调度」列的时间自动执行。
        </p>
        <p v-else class="text-warning">
          本进程未运行调度器(开发环境或调度由其他实例负责),「下次执行」不可用;手动执行不受影响。
        </p>
        <p class="mt-1 text-text-tertiary">
          手动执行不会绕过任务内部守卫:非交易日、盘中时间窗外等场景任务会自行跳过。
          执行记录持久化在数据库中,重启后保留;任务子阶段的详细失败以服务端日志为准。
        </p>
      </div>
    </section>

    <section class="overflow-hidden rounded border border-border bg-surface-raised">
      <div class="overflow-x-auto">
        <table class="w-full min-w-[900px] text-left text-xs">
          <thead>
            <tr class="border-b border-border text-text-tertiary">
              <th class="px-3 py-2 font-medium">任务</th>
              <th class="px-3 py-2 font-medium">调度</th>
              <th class="px-3 py-2 font-medium">下次执行</th>
              <th class="px-3 py-2 font-medium">最近一次系统执行</th>
              <th class="px-3 py-2 font-medium">最近一次手动执行</th>
              <th class="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody v-if="loading">
            <tr>
              <td colspan="6" class="px-3 py-6 text-text-tertiary">加载中…</td>
            </tr>
          </tbody>
          <tbody v-else>
            <template v-for="job in jobs" :key="job.id">
              <tr class="border-b border-border hover:bg-hover">
                <td class="px-3 py-2.5 align-top">
                  <div class="font-medium text-text-primary">{{ job.name }}</div>
                  <div class="mt-0.5 max-w-72 leading-4 text-text-tertiary">{{ job.description }}</div>
                  <div class="mt-0.5 font-mono text-[10px] text-text-tertiary">{{ job.id }}</div>
                </td>
                <td class="whitespace-nowrap px-3 py-2.5 align-top text-text-secondary">{{ job.schedule }}</td>
                <td class="whitespace-nowrap px-3 py-2.5 align-top text-text-secondary">
                  {{ formatTime(job.next_run_time) }}
                </td>
                <td class="px-3 py-2.5 align-top">
                  <template v-if="job.last_system_run">
                    <div :class="statusClass(job.last_system_run)">
                      {{ runSummary(job.last_system_run) }}
                    </div>
                    <div class="mt-0.5 text-[10px] text-text-tertiary">
                      {{ formatTime(job.last_system_run.started_at) }}
                    </div>
                  </template>
                  <span v-else class="text-text-tertiary">暂无记录</span>
                </td>
                <td class="px-3 py-2.5 align-top">
                  <template v-if="job.manual_run">
                    <div class="flex items-center gap-1.5">
                      <Loader2
                        v-if="job.manual_run.status === 'running'"
                        :size="12"
                        class="animate-spin text-accent"
                      />
                      <span :class="statusClass(job.manual_run)">
                        {{ runSummary(job.manual_run) }}
                      </span>
                    </div>
                    <div class="mt-0.5 text-[10px] text-text-tertiary">
                      {{ formatTime(job.manual_run.started_at) }}
                      <template v-if="job.manual_run.finished_at">
                        → {{ formatTime(job.manual_run.finished_at) }}
                      </template>
                      <template v-if="job.manual_run.operator">
                        · {{ job.manual_run.operator }}
                      </template>
                    </div>
                  </template>
                  <span v-else class="text-text-tertiary">尚未手动执行</span>
                </td>
                <td class="px-3 py-2.5 text-right align-top">
                  <div class="inline-flex items-center gap-1.5">
                    <button
                      type="button"
                      class="inline-flex h-7 items-center gap-1 rounded border border-border bg-surface px-2 text-xs text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
                      :title="expandedId === job.id ? '收起执行记录' : '查看执行记录'"
                      @click="toggleHistory(job)"
                    >
                      <ChevronDown v-if="expandedId === job.id" :size="12" />
                      <ChevronRight v-else :size="12" />
                      记录
                    </button>
                    <button
                      type="button"
                      class="inline-flex h-7 items-center gap-1 rounded border border-border bg-surface px-2 text-xs text-text-secondary transition-colors hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-50"
                      :disabled="busy || job.manual_run?.status === 'running'"
                      @click="trigger(job)"
                    >
                      <Play :size="12" />
                      立即执行
                    </button>
                  </div>
                </td>
              </tr>
              <tr v-if="expandedId === job.id" class="border-b border-border bg-surface">
                <td colspan="6" class="px-3 py-2.5">
                  <div v-if="historyLoading" class="py-2 text-text-tertiary">加载执行记录…</div>
                  <div v-else-if="history.length === 0" class="py-2 text-text-tertiary">暂无执行记录</div>
                  <table v-else class="w-full text-left text-[11px]">
                    <thead>
                      <tr class="border-b border-border text-text-tertiary">
                        <th class="py-1.5 pr-3 font-medium">开始时间</th>
                        <th class="py-1.5 pr-3 font-medium">结束时间</th>
                        <th class="py-1.5 pr-3 font-medium">触发</th>
                        <th class="py-1.5 pr-3 font-medium">状态</th>
                        <th class="py-1.5 font-medium">结果</th>
                      </tr>
                    </thead>
                    <tbody>
                      <tr
                        v-for="record in history"
                        :key="record.id"
                        class="border-b border-border last:border-b-0"
                      >
                        <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">
                          {{ formatTime(record.started_at) }}
                        </td>
                        <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">
                          {{ formatTime(record.finished_at) }}
                        </td>
                        <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">
                          {{ record.trigger === 'manual' ? `手动${record.operator ? ` · ${record.operator}` : ''}` : '系统调度' }}
                        </td>
                        <td class="whitespace-nowrap py-1.5 pr-3" :class="statusClass(record)">
                          {{ record.status === 'running' ? '执行中' : record.status === 'failed' ? '失败' : '完成' }}
                        </td>
                        <td class="max-w-md break-all py-1.5" :class="statusClass(record)">
                          {{ runSummary(record) }}
                        </td>
                      </tr>
                    </tbody>
                  </table>
                </td>
              </tr>
            </template>
          </tbody>
        </table>
      </div>
    </section>
  </div>
</template>
