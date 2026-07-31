<script setup lang="ts">
/**
 * 定时任务：查看调度状态、启停、手动触发、翻看执行记录。
 *
 * 参考 quant 的 AdminJobs 页面。执行记录持久化在 job_runs（重启保留）；
 * 进程重启遗留的「执行中」启动时自动收尾为失败。手动触发不绕过任务
 * 内部守卫（比如无信号时简报任务保持安静，记录里体现为 quiet）。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api, type AdminJob, type JobRun } from '../composables/api'

const loading = ref(true)
const schedulerRunning = ref(false)
const jobs = ref<AdminJob[]>([])
const actionError = ref('')
const notice = ref('')

const POLL_MS = 3000
let pollTimer: ReturnType<typeof setInterval> | null = null

const expandedId = ref<string | null>(null)
const history = ref<JobRun[]>([])
const historyLoading = ref(false)

const hasRunning = computed(() =>
  jobs.value.some((job) => job.manual_run?.status === 'running' || job.last_system_run?.status === 'running'),
)

async function load() {
  try {
    const res = await api.listJobs()
    schedulerRunning.value = res.scheduler_running
    jobs.value = res.jobs
    if (expandedId.value) await loadHistory(expandedId.value)
  } catch (e: any) {
    actionError.value = e?.message || '加载失败'
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
    history.value = await api.jobRuns(jobId)
  } catch (e: any) {
    actionError.value = e?.message || '加载执行记录失败'
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
  if (!confirm(`立即执行「${job.name}」一次？`)) return
  actionError.value = ''
  notice.value = ''
  try {
    await api.runJob(job.id)
    notice.value = `已启动「${job.name}」，执行状态见下表。`
    await load()
    syncPolling()
  } catch (e: any) {
    actionError.value = e?.message || '触发失败'
  }
}

async function toggleEnabled(job: AdminJob) {
  try {
    await api.updateJob(job.id, { enabled: !job.enabled })
    await load()
  } catch (e: any) {
    actionError.value = e?.message || '更新失败'
  }
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '—'
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  return d.toLocaleString('zh-CN', { hour12: false })
}

function runSummary(record: JobRun): string {
  if (record.status === 'running') return '执行中…'
  if (record.status === 'failed') return `失败: ${record.error ?? '未知错误'}`
  if (record.result == null) return '已完成'
  const text = record.result
  return text.length > 120 ? `${text.slice(0, 120)}…` : text
}

function statusClass(record: JobRun): string {
  if (record.status === 'running') return 'text-text-secondary'
  if (record.status === 'failed') return 'text-red-400'
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
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">定时任务</h1>
    </div>

    <p v-if="actionError" class="mb-4 text-xs text-red-400">{{ actionError }}</p>
    <p v-else-if="notice" class="mb-4 text-xs text-green-500">{{ notice }}</p>

    <!-- 调度状态说明 -->
    <div class="mb-6 p-4 border border-border-subtle rounded-lg text-xs leading-5 text-text-secondary">
      <p v-if="schedulerRunning">调度器运行中，任务将按「调度」列的时间自动执行（上海时区）。</p>
      <p v-else class="text-yellow-500">调度器未开启（server.scheduler_enabled=false 或其他实例负责）；手动执行不受影响。</p>
      <p class="mt-1 text-text-tertiary">
        手动执行不绕过任务内部守卫：例如盘后信号简报在无信号时保持安静（记录里体现为 quiet）。
        执行记录持久化在 job_runs 表，重启后保留。
      </p>
    </div>

    <!-- 任务表 -->
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="jobs.length === 0" class="text-sm text-text-tertiary">暂无注册的定时任务。</div>
    <table v-else class="w-full text-sm">
      <thead>
        <tr class="text-left text-xs text-text-tertiary border-b border-border-subtle">
          <th class="py-2 pr-3">任务</th>
          <th class="py-2 pr-3">调度</th>
          <th class="py-2 pr-3">启用</th>
          <th class="py-2 pr-3">下次执行</th>
          <th class="py-2 pr-3">最近系统执行</th>
          <th class="py-2 pr-3">最近手动执行</th>
          <th class="py-2">操作</th>
        </tr>
      </thead>
      <tbody>
        <template v-for="job in jobs" :key="job.id">
          <tr class="border-b border-border-subtle">
            <td class="py-2 pr-3 align-top">
              <div class="text-text-primary font-medium">{{ job.name }}</div>
              <div class="mt-0.5 max-w-64 leading-4 text-xs text-text-tertiary">{{ job.description }}</div>
              <div class="mt-0.5 font-mono text-[10px] text-text-tertiary">{{ job.id }}</div>
            </td>
            <td class="py-2 pr-3 align-top text-text-secondary text-xs whitespace-nowrap">{{ job.schedule }}</td>
            <td class="py-2 pr-3 align-top">
              <button @click="toggleEnabled(job)" class="flex items-center gap-2">
                <span class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors" :class="job.enabled ? 'bg-green-500' : 'bg-border-subtle'">
                  <span class="inline-block h-3 w-3 rounded-full bg-white transition-transform" :class="job.enabled ? 'translate-x-3.5' : 'translate-x-0.5'"></span>
                </span>
              </button>
            </td>
            <td class="py-2 pr-3 align-top text-text-secondary text-xs whitespace-nowrap">
              {{ job.enabled ? formatTime(job.next_run_time) : '—' }}
            </td>
            <td class="py-2 pr-3 align-top">
              <template v-if="job.last_system_run">
                <div class="text-xs" :class="statusClass(job.last_system_run)">{{ runSummary(job.last_system_run) }}</div>
                <div class="mt-0.5 text-[10px] text-text-tertiary">{{ formatTime(job.last_system_run.started_at) }}</div>
              </template>
              <span v-else class="text-xs text-text-tertiary">暂无记录</span>
            </td>
            <td class="py-2 pr-3 align-top">
              <template v-if="job.manual_run">
                <div class="text-xs" :class="statusClass(job.manual_run)">{{ runSummary(job.manual_run) }}</div>
                <div class="mt-0.5 text-[10px] text-text-tertiary">
                  {{ formatTime(job.manual_run.started_at) }}
                  <template v-if="job.manual_run.finished_at">→ {{ formatTime(job.manual_run.finished_at) }}</template>
                </div>
              </template>
              <span v-else class="text-xs text-text-tertiary">尚未手动执行</span>
            </td>
            <td class="py-2 align-top whitespace-nowrap">
              <button @click="toggleHistory(job)" class="text-xs text-text-secondary hover:underline mr-3">
                {{ expandedId === job.id ? '收起' : '记录' }}
              </button>
              <button
                @click="trigger(job)"
                :disabled="job.manual_run?.status === 'running'"
                class="text-xs text-accent hover:underline disabled:opacity-40 disabled:cursor-not-allowed"
              >立即执行</button>
            </td>
          </tr>
          <!-- 执行记录展开 -->
          <tr v-if="expandedId === job.id" class="border-b border-border-subtle bg-surface">
            <td colspan="7" class="px-3 py-2.5">
              <div v-if="historyLoading" class="py-2 text-xs text-text-tertiary">加载执行记录…</div>
              <div v-else-if="history.length === 0" class="py-2 text-xs text-text-tertiary">暂无执行记录</div>
              <table v-else class="w-full text-left text-[11px]">
                <thead>
                  <tr class="border-b border-border-subtle text-text-tertiary">
                    <th class="py-1.5 pr-3 font-medium">开始时间</th>
                    <th class="py-1.5 pr-3 font-medium">结束时间</th>
                    <th class="py-1.5 pr-3 font-medium">触发</th>
                    <th class="py-1.5 pr-3 font-medium">状态</th>
                    <th class="py-1.5 font-medium">结果</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="record in history" :key="record.id" class="border-b border-border-subtle last:border-b-0">
                    <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">{{ formatTime(record.started_at) }}</td>
                    <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">{{ formatTime(record.finished_at) }}</td>
                    <td class="whitespace-nowrap py-1.5 pr-3 text-text-secondary">
                      {{ record.trigger === 'manual' ? `手动${record.operator ? ` · ${record.operator.slice(0, 8)}` : ''}` : '系统调度' }}
                    </td>
                    <td class="whitespace-nowrap py-1.5 pr-3" :class="statusClass(record)">
                      {{ record.status === 'running' ? '执行中' : record.status === 'failed' ? '失败' : '完成' }}
                    </td>
                    <td class="max-w-md break-all py-1.5" :class="statusClass(record)">{{ runSummary(record) }}</td>
                  </tr>
                </tbody>
              </table>
            </td>
          </tr>
        </template>
      </tbody>
    </table>
  </div>
</template>
