<script setup lang="ts">
/**
 * 任务中心:当前用户的异步任务(回测/参数扫描/成本敏感性)。
 *
 * 数据来自全局任务 store(tasks.ts):有进行中任务时 store 每 2s 轮询,
 * 本页只渲染只读状态;取消操作经 confirmDialog 确认后调 cancelTask。
 * 每个用户同一时刻只能运行一个任务(提交冲突会被服务端 409 拒绝)。
 */
import { computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ListChecks, Loader2, RefreshCw, XCircle } from 'lucide-vue-next'
import type { QuantTask, TaskStatus, TaskType } from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import { confirmDialog } from '../confirmDialog'
import { cancelTask, refreshTasks, taskState } from '../tasks'
import { useAsyncAction } from '../useAsyncAction'

const router = useRouter()
const { busy, error, notice, fail, run } = useAsyncAction()

const tasks = computed(() => taskState.tasks)
const loaded = computed(() => taskState.loaded)

const TYPE_LABELS: Record<TaskType, string> = {
  backtest: '回测',
  sweep: '参数扫描',
  sensitivity: '成本敏感性',
  factor_backfill: '因子回填',
}

const STATUS_LABELS: Record<TaskStatus, string> = {
  pending: '等待中',
  running: '运行中',
  done: '已完成',
  failed: '失败',
  cancelled: '已取消',
}

function statusClass(status: TaskStatus): string {
  switch (status) {
    case 'running':
      return 'text-accent'
    case 'done':
      return 'text-down'
    case 'failed':
      return 'text-up'
    default:
      return 'text-text-secondary'
  }
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '—'
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  return d.toLocaleString('zh-CN', { hour12: false })
}

function duration(task: QuantTask): string {
  if (!task.started_at) return '—'
  const start = new Date(task.started_at).getTime()
  const end = task.finished_at ? new Date(task.finished_at).getTime() : Date.now()
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return '—'
  const seconds = Math.round((end - start) / 1000)
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`
}

function viewResult(task: QuantTask) {
  if (task.type === 'backtest' && task.ref_id) {
    void router.push({ name: 'strategies-backtest', query: { run: task.ref_id } })
  } else if (task.type === 'sweep') {
    void router.push({ name: 'strategies-backtest', query: { sweep_task: task.id } })
  }
}

async function cancel(task: QuantTask) {
  const confirmed = await confirmDialog(`取消任务「${task.title}」?`, {
    title: '取消任务',
    tone: 'danger',
    confirmText: '取消任务',
  })
  if (!confirmed) return
  await run(async () => {
    await cancelTask(task.id)
    return true
  }, { success: `已取消「${task.title}」。` })
}

async function refresh() {
  try {
    await refreshTasks()
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  }
}

onMounted(refresh)
</script>

<template>
  <div class="space-y-4">
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-else-if="notice">{{ notice }}</InlineFeedback>

    <section class="overflow-hidden rounded border border-border bg-surface-raised">
      <div class="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <ListChecks :size="15" class="text-accent" />
        <h2 class="text-sm font-medium text-text-primary">我的任务</h2>
        <span class="text-[10px] text-text-tertiary">同一时刻只能运行一个任务</span>
        <button
          type="button"
          class="icon-button ml-auto"
          title="刷新"
          @click="refresh"
        >
          <RefreshCw :size="14" />
          <span class="sr-only">刷新</span>
        </button>
      </div>
      <div class="overflow-x-auto">
        <table class="w-full min-w-[860px] text-left text-xs">
          <thead>
            <tr class="border-b border-border text-text-tertiary">
              <th class="px-3 py-2 font-medium">编号</th>
              <th class="px-3 py-2 font-medium">类型</th>
              <th class="px-3 py-2 font-medium">任务</th>
              <th class="px-3 py-2 font-medium">状态</th>
              <th class="px-3 py-2 font-medium">提交时间</th>
              <th class="px-3 py-2 font-medium">耗时</th>
              <th class="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody v-if="!loaded">
            <tr>
              <td colspan="7" class="px-3 py-6 text-text-tertiary">加载中…</td>
            </tr>
          </tbody>
          <tbody v-else-if="tasks.length === 0">
            <tr>
              <td colspan="7" class="px-3 py-6 text-text-tertiary">
                暂无任务。在回测验证页提交回测或参数扫描后,可在这里查看进度。
              </td>
            </tr>
          </tbody>
          <tbody v-else>
            <tr
              v-for="task in tasks"
              :key="task.id"
              class="border-b border-border last:border-b-0 hover:bg-hover"
            >
              <td class="whitespace-nowrap px-3 py-2.5 align-top font-mono text-text-tertiary">
                #{{ task.id }}
              </td>
              <td class="whitespace-nowrap px-3 py-2.5 align-top text-text-secondary">
                {{ TYPE_LABELS[task.type] ?? task.type }}
              </td>
              <td class="max-w-72 px-3 py-2.5 align-top">
                <div class="truncate font-medium text-text-primary" :title="task.title">
                  {{ task.title }}
                </div>
                <div
                  v-if="task.status === 'failed' && task.error"
                  class="mt-0.5 line-clamp-2 break-all leading-4 text-up"
                  :title="task.error"
                >{{ task.error }}</div>
              </td>
              <td class="whitespace-nowrap px-3 py-2.5 align-top">
                <span class="inline-flex items-center gap-1.5" :class="statusClass(task.status)">
                  <Loader2
                    v-if="task.status === 'pending' || task.status === 'running'"
                    :size="12"
                    class="animate-spin"
                  />
                  {{ STATUS_LABELS[task.status] ?? task.status }}
                </span>
              </td>
              <td class="whitespace-nowrap px-3 py-2.5 align-top text-text-secondary">
                {{ formatTime(task.created_at) }}
              </td>
              <td class="whitespace-nowrap px-3 py-2.5 align-top text-text-secondary">
                {{ duration(task) }}
              </td>
              <td class="whitespace-nowrap px-3 py-2.5 text-right align-top">
                <div class="inline-flex items-center gap-1.5">
                  <button
                    v-if="task.status === 'done' && (task.type === 'backtest' || task.type === 'sweep')"
                    type="button"
                    class="inline-flex h-7 items-center gap-1 rounded border border-border bg-surface px-2 text-xs text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
                    @click="viewResult(task)"
                  >
                    查看结果
                  </button>
                  <button
                    v-if="task.status === 'pending'"
                    type="button"
                    class="inline-flex h-7 items-center gap-1 rounded border border-border bg-surface px-2 text-xs text-up transition-colors hover:bg-hover disabled:cursor-not-allowed disabled:opacity-50"
                    :disabled="busy"
                    @click="cancel(task)"
                  >
                    <XCircle :size="12" />
                    取消
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </div>
</template>
