import { computed, nextTick, ref } from 'vue'
import { api } from './api'
import { ONBOARDING_TOURS } from './onboardingTours'
import { startTour } from './tour'

export interface OnboardingTask {
  id: string
  title: string
  desc: string
  target: { name: string; query?: Record<string, string> }
  completion:
    | { type: 'visit'; name: string; query?: Record<string, string> }
    | { type: 'tour' }
    | { type: 'probe'; key: 'watchlist' | 'strategies' | 'trades' }
    | { type: 'event' }
}

export const ONBOARDING_TASKS: OnboardingTask[] = [
  {
    id: 'visit_dashboard',
    title: '打开「今日研究」确认数据日期',
    desc: '行情总览展示最新数据日期,先确认数据已更新。「前往」后自动开启页面引导,看完或跳过即完成。',
    target: { name: 'dashboard' },
    completion: { type: 'tour' },
  },
  {
    id: 'add_watch',
    title: '添加第一只自选股',
    desc: '自选股决定行情总览与盘中快照的展示范围。「前往」后按页面引导实际操作即完成。',
    target: { name: 'watchlist' },
    completion: { type: 'probe', key: 'watchlist' },
  },
  {
    id: 'view_picks',
    title: '查看每日 Top 30 候选',
    desc: '系统按固定评分流程给出的每日候选名单。「前往」后自动开启页面引导,看完或跳过即完成。',
    target: { name: 'selection', query: { tab: 'picks' } },
    completion: { type: 'tour' },
  },
  {
    id: 'run_screener',
    title: '用组合筛选器筛一次股票',
    desc: '按技术面、估值和财务条件独立组合筛选。「前往」后自动开启页面引导,看完或跳过即完成。',
    target: { name: 'selection', query: { tab: 'screener' } },
    completion: { type: 'tour' },
  },
  {
    id: 'view_signals',
    title: '查看策略信号提醒',
    desc: '信号是策略状态变化的提醒,不是买卖指令。「前往」后自动开启页面引导,看完或跳过即完成。',
    target: { name: 'signals' },
    completion: { type: 'tour' },
  },
  {
    id: 'duplicate_strategy',
    title: '把公共策略另存为我的策略',
    desc: '公共策略只读,另存后即可调参并用于回测。「前往」后按页面引导实际操作即完成。',
    target: { name: 'strategies-manage' },
    completion: { type: 'probe', key: 'strategies' },
  },
  {
    id: 'run_backtest',
    title: '跑一次历史回测',
    desc: '按历史日线模拟策略表现,用于验证规则。「前往」后按页面引导实际操作即完成。',
    target: { name: 'strategies-backtest' },
    completion: { type: 'event' },
  },
  {
    id: 'add_trade',
    title: '记一笔手工成交',
    desc: '记录已在外部交易软件中完成的真实成交。「前往」后按页面引导实际操作即完成。',
    target: { name: 'portfolio' },
    completion: { type: 'probe', key: 'trades' },
  },
]

const STORAGE_KEY = 'quant_onboarding_v1'

interface StoredOnboarding {
  done: Record<string, string>
  seen: boolean
  hidden: boolean
}

/** 防御性读取,手工改坏或旧格式一律视为无进度 */
function readStored(): StoredOnboarding {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null')
    if (!parsed || typeof parsed !== 'object') return { done: {}, seen: false, hidden: false }
    const record = parsed as Record<string, unknown>
    const done: Record<string, string> = {}
    if (record.done && typeof record.done === 'object') {
      for (const [key, value] of Object.entries(record.done as Record<string, unknown>)) {
        if (typeof value === 'string' && ONBOARDING_TASKS.some((task) => task.id === key)) {
          done[key] = value
        }
      }
    }
    return { done, seen: record.seen === true, hidden: record.hidden === true }
  } catch {
    return { done: {}, seen: false, hidden: false }
  }
}

const stored = readStored()
const done = ref<Record<string, string>>({ ...stored.done })
const panelOpen = ref(false)
const hasInteracted = ref(stored.seen)
/** 用户在面板里选择「永远隐藏」后,浮动入口与面板都不再渲染,只能在设置页恢复 */
const dismissed = ref(stored.hidden)

function persist() {
  localStorage.setItem(
    STORAGE_KEY,
    JSON.stringify({ done: done.value, seen: hasInteracted.value, hidden: dismissed.value }),
  )
}

function markDone(id: string) {
  if (done.value[id]) return
  done.value = { ...done.value, [id]: new Date().toISOString() }
  persist()
}

interface RouteLike {
  name?: unknown
  query?: Record<string, unknown>
}

/** 路由跳转时检查访问类任务;带 query 的 completion 需同时匹配 query.tab */
export function completeByRoute(route: RouteLike) {
  const name = typeof route.name === 'string' ? route.name : ''
  for (const task of ONBOARDING_TASKS) {
    if (task.completion.type !== 'visit') continue
    if (task.completion.name !== name) continue
    if (task.completion.query) {
      const matches = Object.entries(task.completion.query).every(
        ([key, value]) => String(route.query?.[key] ?? '') === value,
      )
      if (!matches) continue
    }
    markDone(task.id)
  }
}

type RouterPush = (target: OnboardingTask['target']) => unknown

/** 跳转任务目标页并自动开播该任务的聚焦引导;引导看完或主动跳过即视为 tour 类任务完成 */
export async function startTaskTour(id: string, routerPush: RouterPush) {
  const task = ONBOARDING_TASKS.find((item) => item.id === id)
  if (!task) return
  await routerPush(task.target)
  const steps = ONBOARDING_TOURS[id]
  if (!steps?.length) return
  // 等路由组件挂载与首屏数据开始渲染
  await nextTick()
  await new Promise((resolve) => setTimeout(resolve, 300))
  startTour(steps, {
    onFinish: () => {
      // 看完或主动跳过都算完成——用户已明确看过或选择不看;探测与事件类任务完成条件不变
      if (task.completion.type === 'tour') markDone(id)
    },
  })
}

/** 面板打开时探测数据类任务;未登录或网络错误静默忽略 */
export async function refreshProbes() {
  const pending = ONBOARDING_TASKS.filter((task) => task.completion.type === 'probe' && !done.value[task.id])
  await Promise.all(pending.map(async (task) => {
    const key = (task.completion as { type: 'probe'; key: string }).key
    try {
      if (key === 'watchlist') {
        const payload = await api.watchlist()
        if (payload.count > 0) markDone(task.id)
      } else if (key === 'strategies') {
        const payload = await api.strategies()
        if (payload.items.some((item) => !item.is_system)) markDone(task.id)
      } else if (key === 'trades') {
        const payload = await api.trades()
        if (payload.count > 0) markDone(task.id)
      }
    } catch {
      // 探测失败不影响页面,下次打开面板再试
    }
  }))
}

/** 事件类任务(如回测成功)由页面回调标记 */
export function markEventDone(id: string) {
  const task = ONBOARDING_TASKS.find((item) => item.id === id)
  if (task?.completion.type !== 'event') return
  markDone(id)
}

export function resetProgress() {
  done.value = {}
  persist()
}

/** 重置单条任务,其它任务进度不受影响 */
export function resetTask(id: string) {
  if (!done.value[id]) return
  const next = { ...done.value }
  delete next[id]
  done.value = next
  persist()
}

/** 永远隐藏浮动入口(仅本浏览器),由设置页 showGuide 恢复 */
export function hideForever() {
  dismissed.value = true
  panelOpen.value = false
  persist()
}

export function showGuide() {
  dismissed.value = false
  persist()
}

/** 首次使用自动展开一次面板,之后记忆用户的折叠选择 */
export function markInteracted() {
  if (hasInteracted.value) return
  hasInteracted.value = true
  persist()
}

const doneCount = computed(() => ONBOARDING_TASKS.filter((task) => done.value[task.id]).length)
const allDone = computed(() => doneCount.value === ONBOARDING_TASKS.length)

export function useOnboarding() {
  return {
    tasks: ONBOARDING_TASKS,
    done,
    panelOpen,
    hasInteracted,
    dismissed,
    doneCount,
    allDone,
    isDone: (id: string) => Boolean(done.value[id]),
    completeByRoute,
    startTaskTour,
    refreshProbes,
    markEventDone,
    resetProgress,
    resetTask,
    hideForever,
    showGuide,
    markInteracted,
  }
}
