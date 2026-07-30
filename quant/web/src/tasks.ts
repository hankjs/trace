/** 全局异步任务 store:任务列表缓存 + 条件轮询 + waitForTask。
 *
 * 模式同 confirmDialog.ts(模块级 reactive,无 Pinia):
 * - 任何页面提交耗时任务后 track(task),store 在有进行中任务时每 2s 轮询
 *   /api/tasks,没有则停止(同 AdminJobs.vue 的 syncPolling 思路);
 * - 页面可用 waitForTask(id) 挂起等待终态,配合 AbortSignal 在卸载时取消;
 * - 顶栏 badge 读 activeTasks,任务中心页读 tasks。
 */
import { computed, reactive, readonly } from 'vue'

import { api, type QuantTask } from './api'

const POLL_INTERVAL = 2000

const state = reactive({
  tasks: [] as QuantTask[],
  loaded: false,
})

/** 供组件渲染的只读状态 */
export const taskState = readonly(state)

const activeTasks = computed(() =>
  state.tasks.filter((t) => t.status === 'pending' || t.status === 'running'),
)

let poller: ReturnType<typeof setInterval> | null = null
let refreshing: Promise<void> | null = null
// waitForTask 的等待者:任务到终态时按 id 通知
const waiters = new Map<number, Array<(task: QuantTask) => void>>()

function syncPolling() {
  const needPoll = activeTasks.value.length > 0 || waiters.size > 0
  if (needPoll && poller === null) {
    poller = setInterval(() => void refreshTasks(), POLL_INTERVAL)
  } else if (!needPoll && poller !== null) {
    clearInterval(poller)
    poller = null
  }
}

function notifyWaiters() {
  for (const [id, callbacks] of [...waiters]) {
    const task = state.tasks.find((t) => t.id === id)
    if (!task || task.status === 'pending' || task.status === 'running') continue
    waiters.delete(id)
    callbacks.forEach((cb) => cb(task))
  }
}

/** 拉取任务列表(新的在前)。并发去重,失败静默(下次轮询再试)。 */
export async function refreshTasks(): Promise<void> {
  if (refreshing) return refreshing
  refreshing = (async () => {
    try {
      const { tasks } = await api.listTasks()
      state.tasks = tasks
      state.loaded = true
      notifyWaiters()
    } catch {
      // 轮询场景下静默;页面主动刷新时错误由列表接口下次成功覆盖
    } finally {
      refreshing = null
      syncPolling()
    }
  })()
  return refreshing
}

/** 提交成功后登记任务并启动轮询 */
export function trackTask(task: QuantTask) {
  state.tasks = [task, ...state.tasks.filter((t) => t.id !== task.id)]
  syncPolling()
}

/**
 * 等待任务到终态。resolve 终态任务(含 failed/cancelled,由调用方判断);
 * signal 中止时 reject(页面卸载/重新提交场景)。
 */
export function waitForTask(
  taskId: number,
  opts?: { signal?: AbortSignal },
): Promise<QuantTask> {
  const signal = opts?.signal
  if (signal?.aborted) return Promise.reject(new Error('已取消'))
  return new Promise<QuantTask>((resolve, reject) => {
    const onAbort = () => {
      const callbacks = waiters.get(taskId)
      if (callbacks) waiters.set(taskId, callbacks.filter((cb) => cb !== done))
      reject(new Error('已取消'))
    }
    const done = (task: QuantTask) => {
      signal?.removeEventListener('abort', onAbort)
      resolve(task)
    }
    const callbacks = waiters.get(taskId) ?? []
    callbacks.push(done)
    waiters.set(taskId, callbacks)
    signal?.addEventListener('abort', onAbort, { once: true })
    // 任务可能在登记前就已完成(快任务),先检查一次现有缓存
    const cached = state.tasks.find((t) => t.id === taskId)
    if (cached && cached.status !== 'pending' && cached.status !== 'running') {
      waiters.set(taskId, (waiters.get(taskId) ?? []).filter((cb) => cb !== done))
      done(cached)
      return
    }
    syncPolling()
    void refreshTasks()
  })
}

/** 取消等待中的任务并刷新列表 */
export async function cancelTask(taskId: number): Promise<void> {
  const task = await api.cancelTask(taskId)
  trackTask(task)
  notifyWaiters()
}

/** 登出时清空(与 resetStrategies 等并列调用) */
export function resetTasks() {
  state.tasks = []
  state.loaded = false
  waiters.clear()
  if (poller !== null) {
    clearInterval(poller)
    poller = null
  }
}

export { activeTasks }
