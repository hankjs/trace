<script setup lang="ts">
/**
 * 任务详情：时间轴 + 角色轮次 + 分析全文 + 取消/重试。
 * 闸门应答不在看板做——需要交互单 id，引导去飞书。
 */
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  api,
  type TeamTask,
  type TeamTaskEvent,
  type TeamTaskRun,
} from '../composables/api'

const route = useRoute()
const router = useRouter()

const loading = ref(true)
const error = ref('')
const notice = ref('')
const acting = ref(false)

const task = ref<TeamTask | null>(null)
const runs = ref<TeamTaskRun[]>([])
const events = ref<TeamTaskEvent[]>([])

const taskNo = computed(() => String(route.params.taskNo || ''))

const isRunning = computed(() =>
  !!task.value?.status.startsWith('running_'),
)
const isFailed = computed(() => task.value?.status === 'failed')
const isPendingGate = computed(() => {
  const s = task.value?.status
  return (
    s === 'pending_confirm' ||
    s === 'pending_review_gate' ||
    s === 'pending_dev_gate' ||
    s === 'pending_test_gate'
  )
})

const ROLE_LABEL: Record<string, string> = {
  developer: '开发',
  reviewer: '评审',
  tester: '测试',
}

const STATUS_LABEL: Record<string, string> = {
  pending_confirm: '待确认',
  running_developer: '开发中',
  pending_review_gate: '待进入评审',
  running_reviewer: '评审中',
  pending_dev_gate: '待重新开发',
  pending_test_gate: '待进入测试',
  running_tester: '测试中',
  done: '已完成',
  failed: '失败',
  cancelled: '已取消',
}

const EVENT_KIND_LABEL: Record<string, string> = {
  role_started: '角色启动',
  role_finished: '角色结束',
  gate_opened: '打开闸门',
  gate_answered: '闸门应答',
  rejected: '打回',
  status_changed: '状态变更',
  cancelled: '取消',
}

function statusLabel(s: string): string {
  return STATUS_LABEL[s] ?? s
}

function roleLabel(role: string | null | undefined): string {
  if (!role) return '—'
  return ROLE_LABEL[role] ?? role
}

function eventKindLabel(kind: string): string {
  return EVENT_KIND_LABEL[kind] ?? kind
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '—'
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  return d.toLocaleString('zh-CN', { hour12: false })
}

function formatDuration(t: TeamTask): string {
  const start = new Date(t.created_at).getTime()
  if (Number.isNaN(start)) return '—'
  const end = t.finished_at ? new Date(t.finished_at).getTime() : Date.now()
  if (Number.isNaN(end)) return '—'
  const sec = Math.max(0, Math.floor((end - start) / 1000))
  if (sec < 60) return `${sec} 秒`
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min} 分 ${sec % 60} 秒`
  const hr = Math.floor(min / 60)
  return `${hr} 小时 ${min % 60} 分`
}

function runTitle(run: TeamTaskRun): string {
  return `${roleLabel(run.role)} · 第 ${run.round} 轮 · ${run.status}`
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    const detail = await api.getTask(taskNo.value)
    task.value = detail.task
    runs.value = detail.runs
    events.value = detail.events
  } catch (e: unknown) {
    error.value = e instanceof Error ? e.message : '加载失败'
    task.value = null
  } finally {
    loading.value = false
  }
}

async function doCancel() {
  if (!task.value) return
  if (!confirm(`取消任务 ${task.value.task_no}？`)) return
  acting.value = true
  error.value = ''
  notice.value = ''
  try {
    await api.cancelTask(task.value.task_no)
    notice.value = '已提交取消'
    await load()
  } catch (e: unknown) {
    error.value = e instanceof Error ? e.message : '取消失败'
  } finally {
    acting.value = false
  }
}

async function doRetry() {
  if (!task.value) return
  if (!confirm(`从当前角色重试 ${task.value.task_no}？`)) return
  acting.value = true
  error.value = ''
  notice.value = ''
  try {
    await api.retryTask(task.value.task_no)
    notice.value = '已提交重试'
    await load()
  } catch (e: unknown) {
    error.value = e instanceof Error ? e.message : '重试失败'
  } finally {
    acting.value = false
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <div>
    <button
      type="button"
      class="text-[12px] text-text-tertiary hover:text-text-secondary mb-4"
      @click="router.push('/')"
    >← 返回看板</button>

    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>
    <p v-else-if="error && !task" class="text-sm text-red-400">{{ error }}</p>

    <template v-else-if="task">
      <!-- 顶部摘要 -->
      <div class="mb-6">
        <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1 mb-2">
          <h1 class="text-lg font-semibold font-mono text-text-primary">{{ task.task_no }}</h1>
          <span class="text-[12px] px-2 py-0.5 rounded bg-active text-text-secondary">
            {{ statusLabel(task.status) }}
          </span>
          <span v-if="task.issue_key" class="text-[12px] text-text-tertiary">
            Issue {{ task.issue_key }}
          </span>
        </div>
        <p class="text-[13px] text-text-primary leading-relaxed mb-3 whitespace-pre-wrap">
          {{ task.goal || task.title || '（无目标）' }}
        </p>
        <div class="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-tertiary">
          <span>后端 {{ task.backend }}</span>
          <span>来源 {{ task.source }}</span>
          <span>角色 {{ roleLabel(task.current_role) }}</span>
          <span>开发轮次 {{ task.dev_rounds }}</span>
          <span>耗时 {{ formatDuration(task) }}</span>
          <span>创建 {{ formatTime(task.created_at) }}</span>
          <span>更新 {{ formatTime(task.updated_at) }}</span>
          <span v-if="task.finished_at">结束 {{ formatTime(task.finished_at) }}</span>
        </div>
        <p v-if="task.error" class="mt-2 text-[12px] text-red-400">{{ task.error }}</p>
      </div>

      <p v-if="error" class="mb-3 text-xs text-red-400">{{ error }}</p>
      <p v-else-if="notice" class="mb-3 text-xs text-green-600">{{ notice }}</p>

      <!-- 操作区 -->
      <div class="mb-6 flex flex-wrap items-center gap-2">
        <button
          v-if="isRunning"
          type="button"
          :disabled="acting"
          class="px-3 py-1.5 text-[12px] rounded-md border border-border text-text-secondary hover:bg-hover disabled:opacity-40"
          @click="doCancel"
        >取消</button>
        <button
          v-if="isFailed"
          type="button"
          :disabled="acting"
          class="px-3 py-1.5 text-[12px] rounded-md bg-text-primary text-surface-raised hover:opacity-80 disabled:opacity-40"
          @click="doRetry"
        >从当前角色重试</button>
        <p
          v-if="isPendingGate"
          class="text-[12px] text-text-secondary leading-relaxed"
        >
          请在飞书卡片上确认闸门。看板本期不做闸门应答（需要交互单 id，属额外链路）。
        </p>
      </div>

      <div class="grid grid-cols-1 lg:grid-cols-[240px_1fr] gap-6">
        <!-- 左侧时间轴 -->
        <aside>
          <h2 class="text-[12px] font-medium text-text-secondary mb-3">时间轴</h2>
          <ul v-if="events.length" class="space-y-3 border-l border-border-subtle pl-3">
            <li v-for="ev in events" :key="ev.id" class="relative">
              <span class="absolute -left-[17px] top-1 w-2 h-2 rounded-full bg-border" />
              <div class="text-[11px] text-text-tertiary">{{ formatTime(ev.created_at) }}</div>
              <div class="text-[12px] text-text-primary">
                {{ eventKindLabel(ev.kind) }}
                <span v-if="ev.role" class="text-text-tertiary">
                  · {{ roleLabel(ev.role) }}
                  <template v-if="ev.round != null">#{{ ev.round }}</template>
                </span>
              </div>
              <div v-if="ev.operator" class="text-[11px] text-text-tertiary">
                操作者 {{ ev.operator }}
              </div>
              <div v-if="ev.detail" class="text-[11px] text-text-secondary mt-0.5 break-words">
                {{ ev.detail }}
              </div>
            </li>
          </ul>
          <p v-else class="text-[12px] text-text-tertiary">暂无事件</p>
        </aside>

        <!-- 右侧 runs + analysis -->
        <div class="space-y-5 min-w-0">
          <section>
            <h2 class="text-[12px] font-medium text-text-secondary mb-3">角色轮次</h2>
            <div v-if="runs.length === 0" class="text-[12px] text-text-tertiary">暂无轮次</div>
            <details
              v-for="run in runs"
              :key="run.id"
              class="mb-2 border border-border-subtle rounded-md overflow-hidden"
              :open="run.status === 'running' || run.status === 'failed'"
            >
              <summary class="cursor-pointer px-3 py-2 text-[12px] bg-hover/50 hover:bg-hover list-none flex items-center justify-between gap-2">
                <span class="font-medium text-text-primary">{{ runTitle(run) }}</span>
                <span class="text-[11px] text-text-tertiary shrink-0">
                  <template v-if="run.verdict">verdict={{ run.verdict }}</template>
                  <template v-if="run.dirty_files != null"> · 改动 {{ run.dirty_files }}</template>
                </span>
              </summary>
              <div class="px-3 py-2 space-y-2 text-[12px] border-t border-border-subtle">
                <div v-if="run.summary">
                  <div class="text-[11px] text-text-tertiary mb-0.5">摘要</div>
                  <p class="text-text-primary whitespace-pre-wrap">{{ run.summary }}</p>
                </div>
                <div v-if="run.handoff">
                  <div class="text-[11px] text-text-tertiary mb-0.5">交接</div>
                  <pre class="text-[11px] font-mono text-text-secondary whitespace-pre-wrap break-words bg-hover/40 rounded p-2">{{ run.handoff }}</pre>
                </div>
                <div v-if="run.error" class="text-red-400">{{ run.error }}</div>
                <div class="text-[11px] text-text-tertiary">
                  开始 {{ formatTime(run.started_at) }}
                  <template v-if="run.finished_at"> · 结束 {{ formatTime(run.finished_at) }}</template>
                  <template v-if="run.thread_id"> · thread {{ run.thread_id }}</template>
                </div>
              </div>
            </details>
          </section>

          <section>
            <h2 class="text-[12px] font-medium text-text-secondary mb-3">分析全文</h2>
            <!-- 纯文本 pre，不引入 markdown 库（与 admin 交互单同一取舍） -->
            <pre
              v-if="task.analysis"
              class="text-[12px] font-mono text-text-secondary whitespace-pre-wrap break-words bg-hover/40 border border-border-subtle rounded-md p-3 max-h-[480px] overflow-y-auto leading-relaxed"
            >{{ task.analysis }}</pre>
            <p v-else class="text-[12px] text-text-tertiary">无分析内容</p>
          </section>
        </div>
      </div>
    </template>
  </div>
</template>
