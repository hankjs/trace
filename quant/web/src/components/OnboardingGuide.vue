<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Check, CheckCircle2, ListChecks, RotateCcw, X } from 'lucide-vue-next'
import { completeByRoute, hideForever, markInteracted, refreshProbes, resetProgress, resetTask, startTaskTour, useOnboarding } from '../onboarding'
import type { OnboardingTask } from '../onboarding'

const route = useRoute()
const router = useRouter()
const { tasks, panelOpen, hasInteracted, dismissed, doneCount, allDone, isDone } = useOnboarding()

const trigger = ref<HTMLButtonElement | null>(null)
const dialog = ref<HTMLElement | null>(null)
const closeButton = ref<HTMLButtonElement | null>(null)
const titleId = 'onboarding-guide-title'

// 访问类任务在任何页面跳转时都能完成,immediate 覆盖首次落地页
watch(() => route.fullPath, () => {
  completeByRoute(route)
}, { immediate: true })

watch(panelOpen, async (open) => {
  // body 锁定仅在移动宽度下(面板带遮罩);桌面上面板是局部弹层不锁定滚动
  const isMobile = typeof window.matchMedia === 'function' && window.matchMedia('(max-width: 1023px)').matches
  document.body.style.overflow = open && isMobile ? 'hidden' : ''
  if (open) {
    markInteracted()
    void refreshProbes()
    await nextTick()
    ;(closeButton.value ?? dialog.value)?.focus()
  }
})

onMounted(() => {
  // 首次使用自动展开一次面板,之后记住用户的折叠选择;已「永远隐藏」则不展开
  if (!hasInteracted.value && !dismissed.value) panelOpen.value = true
})

function openPanel() {
  panelOpen.value = true
}

function closePanel(restoreFocus = true) {
  if (!panelOpen.value) return
  panelOpen.value = false
  if (restoreFocus) void nextTick(() => trigger.value?.focus())
}

function goTo(task: OnboardingTask) {
  closePanel(false)
  void startTaskTour(task.id, (target) => router.push(target))
}

function onDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    closePanel()
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

onBeforeUnmount(() => {
  document.body.style.overflow = ''
})
</script>

<template>
  <button
    v-if="!dismissed"
    ref="trigger"
    type="button"
    class="fixed bottom-24 right-4 z-30 flex h-11 w-11 items-center justify-center rounded-full bg-surface-raised text-accent shadow-panel lg:bottom-10"
    :class="{ 'opacity-60': allDone }"
    title="新手上路"
    :aria-expanded="panelOpen"
    aria-controls="onboarding-guide"
    @click="openPanel"
  >
    <Check v-if="allDone" :size="21" />
    <ListChecks v-else :size="21" />
    <span
      class="absolute -right-1 -top-1 flex h-5 min-w-5 items-center justify-center rounded-full bg-accent px-1 text-[10px] font-medium text-on-accent"
    >{{ doneCount }}/{{ tasks.length }}</span>
    <span class="sr-only">打开新手引导</span>
  </button>

  <Teleport to="body">
    <div v-if="panelOpen && !dismissed" class="fixed inset-0 z-50">
      <button class="absolute inset-0 bg-overlay lg:hidden" aria-label="关闭新手引导" tabindex="-1" @click="closePanel()" />
      <aside
        id="onboarding-guide"
        ref="dialog"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        tabindex="-1"
        class="absolute bottom-24 right-4 flex max-h-[70vh] w-[min(92vw,340px)] flex-col overflow-hidden rounded-lg border border-border bg-surface-raised shadow-panel lg:bottom-10"
        @keydown="onDialogKeydown"
      >
        <div class="flex items-center justify-between border-b border-border px-4 py-3">
          <div class="flex items-center gap-2">
            <ListChecks :size="17" class="text-accent" />
            <h2 :id="titleId" class="text-sm font-semibold">新手上路</h2>
          </div>
          <button ref="closeButton" type="button" class="icon-button" title="关闭" @click="closePanel()">
            <X :size="18" />
            <span class="sr-only">关闭</span>
          </button>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-4">
          <div class="flex items-center gap-2">
            <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-muted">
              <div class="h-full rounded-full bg-accent transition-[width]" :style="{ width: `${(doneCount / tasks.length) * 100}%` }" />
            </div>
            <span class="shrink-0 text-[11px] text-text-tertiary">{{ doneCount }}/{{ tasks.length }}</span>
          </div>
          <p v-if="allDone" class="mt-3 rounded-md bg-active px-3 py-2 text-xs leading-5 text-accent">
            全部完成！已经走完「研究 → 选股 → 信号 → 回测 → 记账」的完整流程。
          </p>

          <ol class="mt-4 space-y-3">
            <li v-for="(task, index) in tasks" :key="task.id" class="flex items-start gap-2.5">
              <CheckCircle2 v-if="isDone(task.id)" :size="18" class="mt-0.5 shrink-0 text-accent" />
              <span
                v-else
                class="mt-0.5 flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full bg-active text-[10px] font-medium text-accent"
              >{{ index + 1 }}</span>
              <div class="min-w-0 flex-1">
                <p class="text-[13px] font-medium leading-5" :class="isDone(task.id) ? 'text-text-tertiary' : 'text-text-primary'">
                  {{ task.title }}
                </p>
                <p class="mt-0.5 text-[11px] leading-4 text-text-tertiary">{{ task.desc }}</p>
              </div>
              <span v-if="isDone(task.id)" class="flex shrink-0 items-center gap-2">
                <button
                  type="button"
                  class="inline-flex items-center gap-0.5 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-tertiary transition-colors hover:bg-hover hover:text-accent"
                  :title="`重看引导:${task.title}`"
                  @click="goTo(task)"
                >
                  <RotateCcw :size="10" />
                  重看
                </button>
                <button
                  type="button"
                  class="inline-flex items-center gap-0.5 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-tertiary transition-colors hover:bg-hover hover:text-accent"
                  :title="`重置此任务:${task.title}`"
                  @click="resetTask(task.id)"
                >
                  重置
                </button>
              </span>
              <button
                v-else
                type="button"
                class="shrink-0 rounded border border-border px-2 py-0.5 text-[11px] text-accent transition-colors hover:bg-hover"
                @click="goTo(task)"
              >前往</button>
            </li>
          </ol>
        </div>

        <div class="flex items-center justify-between border-t border-border px-4 py-2.5">
          <router-link
            :to="{ name: 'catalog' }"
            class="text-[11px] text-text-tertiary transition-colors hover:text-text-primary"
            @click="closePanel(false)"
          >研究词典</router-link>
          <span class="flex items-center gap-3">
            <button
              type="button"
              class="text-[11px] text-text-tertiary transition-colors hover:text-text-primary"
              @click="resetProgress()"
            >重置进度</button>
            <button
              type="button"
              class="text-[11px] text-text-tertiary transition-colors hover:text-text-primary"
              title="隐藏后可在「设置」页重新显示"
              @click="hideForever()"
            >永远隐藏</button>
          </span>
        </div>
      </aside>
    </div>
  </Teleport>
</template>
