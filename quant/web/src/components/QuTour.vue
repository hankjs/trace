<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ChevronLeft, ChevronRight, MousePointerClick, X } from 'lucide-vue-next'
import { next as advanceStep, prev, skip, useTour } from '../tour'
import type { TourStep } from '../tour'

const { active, currentStep, index, total } = useTour()

const RING_PADDING = 6
const BUBBLE_WIDTH = 288
const BUBBLE_GAP = 12
const WAIT_TIMEOUT = 3000
const POLL_INTERVAL = 150

const dialog = ref<HTMLElement | null>(null)
const targetRect = ref<DOMRect | null>(null)
const isNarrow = ref(window.innerWidth < 640)
const titleId = 'qu-tour-title'

let targetElement: Element | null = null
let restoreFocusTo: Element | null = null
let observer: MutationObserver | undefined
let pollTimer: ReturnType<typeof setInterval> | undefined
let waitTimer: ReturnType<typeof setTimeout> | undefined

const isFirst = computed(() => index.value === 0)
const isLast = computed(() => index.value >= total.value - 1)
const needsTargetClick = computed(() => currentStep.value?.advanceOn === 'target')

const ringStyle = computed(() => {
  const rect = targetRect.value
  if (!rect) return undefined
  return {
    top: `${rect.top - RING_PADDING}px`,
    left: `${rect.left - RING_PADDING}px`,
    width: `${rect.width + RING_PADDING * 2}px`,
    height: `${rect.height + RING_PADDING * 2}px`,
    // 巨大外阴影充当整页遮罩,自身 pointer-events-none,不拦截对目标的操作
    boxShadow: '0 0 0 9999px var(--color-overlay)',
  }
})

const bubbleStyle = computed(() => {
  if (isNarrow.value) return undefined
  const rect = targetRect.value
  const viewportWidth = window.innerWidth
  if (!rect) {
    // 等待目标元素期间气泡居中偏上
    return { top: '35%', left: `${Math.max(8, (viewportWidth - BUBBLE_WIDTH) / 2)}px`, width: `${BUBBLE_WIDTH}px` }
  }
  const placement = resolvePlacement(rect)
  const center = rect.left + rect.width / 2
  const left = Math.min(Math.max(center - BUBBLE_WIDTH / 2, 8), Math.max(8, viewportWidth - BUBBLE_WIDTH - 8))
  if (placement === 'top') {
    return { top: `${rect.top - BUBBLE_GAP}px`, left: `${left}px`, width: `${BUBBLE_WIDTH}px`, transform: 'translateY(-100%)' }
  }
  return { top: `${rect.bottom + BUBBLE_GAP}px`, left: `${left}px`, width: `${BUBBLE_WIDTH}px` }
})

function resolvePlacement(rect: DOMRect): 'top' | 'bottom' {
  const preferred = currentStep.value?.placement ?? 'auto'
  if (preferred === 'top' || preferred === 'bottom') return preferred
  // auto: 下方优先,下方空间不足翻上方
  return rect.bottom + 200 <= window.innerHeight ? 'bottom' : 'top'
}

watch(active, async (value) => {
  if (value) {
    restoreFocusTo = document.activeElement
    await nextTick()
    locateCurrent()
    dialog.value?.focus()
  } else {
    clearWaiters()
    targetElement = null
    targetRect.value = null
    if (restoreFocusTo instanceof HTMLElement) restoreFocusTo.focus()
    restoreFocusTo = null
  }
})

watch(currentStep, async (step) => {
  if (!active.value || !step) return
  clearWaiters()
  targetElement = null
  targetRect.value = null
  await nextTick()
  locateCurrent()
  dialog.value?.focus()
})

function locateCurrent() {
  const step = currentStep.value
  if (!step) return
  const el = document.querySelector(`[data-tour="${step.target}"]`)
  if (el) {
    attach(el)
  } else {
    waitForElement(step)
  }
}

function attach(el: Element) {
  targetElement = el
  // jsdom 未实现 scrollIntoView,做能力判断
  if (typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'center' })
  measure()
}

function measure() {
  if (targetElement) targetRect.value = targetElement.getBoundingClientRect()
}

/** 元素可能因数据加载尚未渲染:MutationObserver + 定时复查,超时自动跳过该步 */
function waitForElement(step: TourStep) {
  clearWaiters()
  observer = new MutationObserver(() => retry(step))
  observer.observe(document.body, { childList: true, subtree: true })
  pollTimer = setInterval(() => retry(step), POLL_INTERVAL)
  waitTimer = setTimeout(() => {
    clearWaiters()
    advanceStep()
  }, WAIT_TIMEOUT)
}

function retry(step: TourStep) {
  if (currentStep.value !== step || targetElement) return
  const el = document.querySelector(`[data-tour="${step.target}"]`)
  if (el) {
    clearWaiters()
    attach(el)
  }
}

function clearWaiters() {
  observer?.disconnect()
  observer = undefined
  if (pollTimer !== undefined) {
    clearInterval(pollTimer)
    pollTimer = undefined
  }
  if (waitTimer !== undefined) {
    clearTimeout(waitTimer)
    waitTimer = undefined
  }
}

function onViewportChange() {
  isNarrow.value = window.innerWidth < 640
  if (active.value) measure()
}

/** 操作型步骤:点击落在高亮目标范围内才前进(capture 阶段判断,不拦截原有点击行为) */
function onDocumentClick(event: MouseEvent) {
  if (!active.value || !needsTargetClick.value) return
  const rect = targetRect.value
  if (!rect) return
  const withinX = event.clientX >= rect.left - RING_PADDING && event.clientX <= rect.right + RING_PADDING
  const withinY = event.clientY >= rect.top - RING_PADDING && event.clientY <= rect.bottom + RING_PADDING
  if (withinX && withinY) advanceStep()
}

function onDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    skip()
    return
  }
  if (event.key !== 'Tab' || !dialog.value) return

  const focusable = Array.from(dialog.value.querySelectorAll<HTMLElement>(
    'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
  )).filter((element) => element.getClientRects().length > 0)
  if (!focusable.length) {
    event.preventDefault()
    dialog.value.focus()
    return
  }
  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

onMounted(() => {
  window.addEventListener('scroll', onViewportChange, true)
  window.addEventListener('resize', onViewportChange)
  document.addEventListener('click', onDocumentClick, true)
})

onBeforeUnmount(() => {
  clearWaiters()
  window.removeEventListener('scroll', onViewportChange, true)
  window.removeEventListener('resize', onViewportChange)
  document.removeEventListener('click', onDocumentClick, true)
})
</script>

<template>
  <Teleport to="body">
    <template v-if="active && currentStep">
      <div
        v-if="targetRect"
        data-testid="tour-highlight"
        class="pointer-events-none fixed z-[60] rounded-md ring-2 ring-accent"
        :style="ringStyle"
        aria-hidden="true"
      />
      <div v-else class="pointer-events-none fixed inset-0 z-[60] bg-overlay" aria-hidden="true" />

      <div
        ref="dialog"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        tabindex="-1"
        data-testid="tour-bubble"
        class="fixed z-[61] rounded-lg border border-border bg-surface-raised p-4 shadow-panel max-sm:inset-x-3 max-sm:bottom-3"
        :style="bubbleStyle"
        @keydown="onDialogKeydown"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <h2 :id="titleId" class="text-sm font-semibold text-text-primary">{{ currentStep.title }}</h2>
            <p class="mt-0.5 text-[11px] text-text-tertiary">第 {{ index + 1 }}/{{ total }} 步</p>
          </div>
          <button type="button" class="icon-button -mr-1.5 -mt-1.5 !h-7 !w-7 shrink-0" title="跳过引导" @click="skip()">
            <X :size="15" />
            <span class="sr-only">跳过引导</span>
          </button>
        </div>

        <p class="mt-2 text-xs leading-5 text-text-secondary">{{ currentStep.content }}</p>

        <p v-if="needsTargetClick" class="mt-2 flex items-center gap-1.5 rounded bg-active px-2 py-1.5 text-[11px] leading-4 text-accent">
          <MousePointerClick :size="13" class="shrink-0" />
          点击页面上高亮的元素继续
        </p>

        <div class="mt-3 flex items-center justify-between">
          <button
            type="button"
            class="btn btn-ghost btn-sm"
            :disabled="isFirst"
            @click="prev()"
          >
            <ChevronLeft :size="14" />
            上一步
          </button>
          <div class="flex items-center gap-2">
            <button type="button" class="btn btn-ghost btn-sm" @click="skip()">跳过</button>
            <button v-if="!needsTargetClick" type="button" class="btn btn-primary btn-sm" @click="advanceStep()">
              {{ isLast ? '完成' : '下一步' }}
              <ChevronRight v-if="!isLast" :size="14" />
            </button>
          </div>
        </div>
      </div>
    </template>
  </Teleport>
</template>
