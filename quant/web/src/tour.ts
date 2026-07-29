import { computed, ref } from 'vue'

export interface TourStep {
  /** data-tour 锚点名 */
  target: string
  title: string
  content: string
  /** 气泡相对目标的位置,默认 auto(下方优先,空间不足翻上方) */
  placement?: 'top' | 'bottom' | 'auto'
  /** 默认 button(点气泡按钮前进);target = 点击高亮元素本身才前进 */
  advanceOn?: 'button' | 'target'
}

export interface TourStartOptions {
  /** completed=false 表示被主动跳过 */
  onFinish?: (completed: boolean) => void
}

const active = ref(false)
const steps = ref<TourStep[]>([])
const index = ref(0)
let onFinish: ((completed: boolean) => void) | undefined

const currentStep = computed(() => (active.value ? steps.value[index.value] : undefined))
const total = computed(() => steps.value.length)

function finish(completed: boolean) {
  const callback = onFinish
  active.value = false
  steps.value = []
  index.value = 0
  onFinish = undefined
  callback?.(completed)
}

/** 空 steps 直接忽略;同时只允许一个 tour,重复调用会先静默终止旧 tour(不触发其回调) */
export function startTour(nextSteps: TourStep[], opts?: TourStartOptions) {
  if (!nextSteps.length) return
  if (active.value) {
    active.value = false
    steps.value = []
    index.value = 0
    onFinish = undefined
  }
  steps.value = nextSteps
  index.value = 0
  onFinish = opts?.onFinish
  active.value = true
}

/** 前进到最后一步之后视为看完,触发 onFinish(true) */
export function next() {
  if (!active.value) return
  if (index.value >= steps.value.length - 1) {
    finish(true)
  } else {
    index.value += 1
  }
}

export function prev() {
  if (!active.value || index.value === 0) return
  index.value -= 1
}

export function skip() {
  if (!active.value) return
  finish(false)
}

export function useTour() {
  return { active, steps, index, total, currentStep, next, prev, skip }
}
