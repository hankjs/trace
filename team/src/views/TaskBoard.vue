<script setup lang="ts">
/**
 * 任务泳道看板。按 status 分列，5s 轮询（仅非终态时开启）。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api, type TeamTask } from '../composables/api'

const router = useRouter()
const loading = ref(true)
const error = ref('')
const tasks = ref<TeamTask[]>([])

const POLL_MS = 5000
let pollTimer: ReturnType<typeof setInterval> | null = null

/** 泳道定义：待放行合并三个 pending_*_gate */
const LANES = [
  { id: 'pending_confirm', label: '待确认', statuses: ['pending_confirm'] },
  { id: 'running_developer', label: '开发中', statuses: ['running_developer'] },
  {
    id: 'pending_gate',
    label: '待放行',
    statuses: ['pending_review_gate', 'pending_dev_gate', 'pending_test_gate'],
  },
  { id: 'running_reviewer', label: '评审中', statuses: ['running_reviewer'] },
  { id: 'running_tester', label: '测试中', statuses: ['running_tester'] },
  { id: 'done', label: '已完成', statuses: ['done'] },
  { id: 'failed', label: '失败', statuses: ['failed'] },
  { id: 'cancelled', label: '已取消', statuses: ['cancelled'] },
] as const

const TERMINAL = new Set(['done', 'failed', 'cancelled'])

const hasNonTerminal = computed(() =>
  tasks.value.some((t) => !TERMINAL.has(t.status)),
)

function tasksInLane(statuses: readonly string[]): TeamTask[] {
  return tasks.value.filter((t) => statuses.includes(t.status))
}

function goalFirstLine(task: TeamTask): string {
  const raw = (task.goal || task.title || '').trim()
  if (!raw) return '（无目标）'
  const line = raw.split('\n')[0] ?? raw
  return line.length > 80 ? `${line.slice(0, 80)}…` : line
}

function roleLabel(role: string | null): string {
  if (!role) return '—'
  const map: Record<string, string> = {
    developer: '开发',
    reviewer: '评审',
    tester: '测试',
  }
  return map[role] ?? role
}

function formatDuration(task: TeamTask): string {
  const start = new Date(task.created_at).getTime()
  if (Number.isNaN(start)) return '—'
  const end = task.finished_at
    ? new Date(task.finished_at).getTime()
    : Date.now()
  if (Number.isNaN(end)) return '—'
  const sec = Math.max(0, Math.floor((end - start) / 1000))
  if (sec < 60) return `${sec}s`
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min}m ${sec % 60}s`
  const hr = Math.floor(min / 60)
  return `${hr}h ${min % 60}m`
}

function statusClass(status: string): string {
  if (status.startsWith('running_')) return 'border-l-blue-400'
  if (status.startsWith('pending_')) return 'border-l-amber-400'
  if (status === 'done') return 'border-l-green-500'
  if (status === 'failed') return 'border-l-red-400'
  if (status === 'cancelled') return 'border-l-gray-400'
  return 'border-l-border'
}

async function load() {
  try {
    const res = await api.listTasks({ per_page: 100 })
    tasks.value = res.data
    error.value = ''
  } catch (e: unknown) {
    error.value = e instanceof Error ? e.message : '加载失败'
  } finally {
    loading.value = false
  }
}

function syncPolling() {
  if (hasNonTerminal.value && pollTimer === null) {
    pollTimer = setInterval(() => void load(), POLL_MS)
  } else if (!hasNonTerminal.value && pollTimer !== null) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

function openTask(task: TeamTask) {
  router.push(`/team/${task.task_no}`)
}

onMounted(async () => {
  await load()
  syncPolling()
})

watch(hasNonTerminal, () => syncPolling())

onBeforeUnmount(() => {
  if (pollTimer !== null) clearInterval(pollTimer)
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-5">
      <h1 class="text-lg font-semibold text-text-primary">任务看板</h1>
      <button
        type="button"
        class="text-[12px] text-text-tertiary hover:text-text-secondary"
        @click="load"
      >刷新</button>
    </div>

    <p v-if="error" class="mb-4 text-xs text-red-400">{{ error }}</p>
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>

    <div
      v-else
      class="flex gap-3 overflow-x-auto pb-4"
      style="min-height: 60vh"
    >
      <div
        v-for="lane in LANES"
        :key="lane.id"
        class="w-56 shrink-0 flex flex-col"
      >
        <div class="flex items-center justify-between mb-2 px-1">
          <span class="text-[12px] font-medium text-text-secondary">{{ lane.label }}</span>
          <span class="text-[11px] text-text-tertiary">{{ tasksInLane(lane.statuses).length }}</span>
        </div>
        <div class="flex-1 space-y-2 bg-hover/40 rounded-lg p-2 min-h-[200px]">
          <button
            v-for="task in tasksInLane(lane.statuses)"
            :key="task.id"
            type="button"
            class="w-full text-left bg-surface-raised border border-border-subtle rounded-md p-2.5 border-l-2 hover:border-border transition-colors"
            :class="statusClass(task.status)"
            @click="openTask(task)"
          >
            <div class="text-[11px] font-mono text-text-tertiary mb-1">{{ task.task_no }}</div>
            <div class="text-[12px] text-text-primary leading-snug mb-1.5 line-clamp-2">
              {{ goalFirstLine(task) }}
            </div>
            <div class="flex flex-wrap gap-x-2 gap-y-0.5 text-[10px] text-text-tertiary">
              <span v-if="task.issue_key">#{{ task.issue_key }}</span>
              <span>{{ roleLabel(task.current_role) }}</span>
              <span>{{ formatDuration(task) }}</span>
              <span v-if="task.dev_rounds > 0">dev×{{ task.dev_rounds }}</span>
            </div>
          </button>
          <div
            v-if="tasksInLane(lane.statuses).length === 0"
            class="text-[11px] text-text-tertiary px-1 py-4 text-center"
          >空</div>
        </div>
      </div>
    </div>
  </div>
</template>
