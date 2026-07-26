<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ChevronLeft, ChevronRight, CircleHelp, Lightbulb, X } from 'lucide-vue-next'
import type { ResearchGuide } from '../guides'

const props = defineProps<{
  guide: ResearchGuide
}>()

const storedCollapsed = localStorage.getItem('quant_guide_collapsed')
const collapsed = ref(storedCollapsed === null ? true : storedCollapsed === 'true')
const mobileOpen = ref(false)
const mobileTrigger = ref<HTMLButtonElement | null>(null)
const mobileDialog = ref<HTMLElement | null>(null)
const closeButton = ref<HTMLButtonElement | null>(null)
const mobileTitleId = 'research-assistant-mobile-title'

watch(collapsed, (value) => {
  localStorage.setItem('quant_guide_collapsed', String(value))
})

watch(() => props.guide.title, () => {
  closeMobile(false)
})

watch(mobileOpen, async (open) => {
  document.body.style.overflow = open ? 'hidden' : ''
  if (open) {
    await nextTick()
    const initialTarget = closeButton.value ?? mobileDialog.value
    initialTarget?.focus()
  }
})

function openMobile() {
  mobileOpen.value = true
}

function closeMobile(restoreFocus = true) {
  if (!mobileOpen.value) return
  mobileOpen.value = false
  if (restoreFocus) void nextTick(() => mobileTrigger.value?.focus())
}

function onDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    closeMobile()
    return
  }
  if (event.key !== 'Tab' || !mobileDialog.value) return

  const focusable = Array.from(mobileDialog.value.querySelectorAll<HTMLElement>(
    'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
  )).filter((element) => element.getClientRects().length > 0)
  if (!focusable.length) {
    event.preventDefault()
    mobileDialog.value.focus()
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
  <aside
    class="sticky top-16 hidden max-h-[calc(100vh-80px)] h-fit shrink-0 overflow-y-auto rounded border border-border bg-surface-raised xl:block"
    :class="collapsed ? 'w-10' : 'w-64'"
    aria-label="研究助手"
  >
    <button
      type="button"
      class="flex h-9 w-full items-center justify-center text-text-tertiary hover:bg-hover hover:text-text-primary focus-visible:outline-2 focus-visible:outline-accent"
      :title="collapsed ? '展开研究助手' : '收起研究助手'"
      @click="collapsed = !collapsed"
    >
      <ChevronLeft v-if="!collapsed" :size="17" />
      <ChevronRight v-else :size="17" />
      <span class="sr-only">{{ collapsed ? '展开研究助手' : '收起研究助手' }}</span>
    </button>

    <div v-if="!collapsed" class="border-t border-border px-3 py-3">
      <div class="mb-3 flex items-center gap-2">
        <Lightbulb :size="17" class="text-accent" />
        <h2 class="text-sm font-semibold">{{ guide.title }}</h2>
      </div>
      <p class="text-sm leading-6 text-text-secondary">{{ guide.summary }}</p>

      <dl v-if="guide.concepts?.length" class="mt-4 space-y-3">
        <div v-for="concept in guide.concepts" :key="concept.term">
          <dt class="text-xs font-medium text-text-primary">{{ concept.term }}</dt>
          <dd class="mt-0.5 text-xs leading-5 text-text-tertiary">{{ concept.explanation }}</dd>
        </div>
      </dl>

      <ol v-if="guide.steps?.length" class="mt-4 space-y-2.5">
        <li v-for="(step, index) in guide.steps" :key="step" class="flex gap-2.5 text-xs leading-5 text-text-secondary">
          <span class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-active text-[11px] font-medium text-accent">
            {{ index + 1 }}
          </span>
          <span>{{ step }}</span>
        </li>
      </ol>

      <p v-if="guide.note" class="mt-4 rounded bg-warning-soft px-3 py-2 text-xs leading-5 text-warning">
        {{ guide.note }}
      </p>
    </div>
  </aside>

  <button
    ref="mobileTrigger"
    type="button"
    class="fixed bottom-5 right-5 z-30 flex h-11 w-11 items-center justify-center rounded-full border border-border bg-surface-raised text-accent shadow-panel xl:hidden"
    title="打开研究助手"
    :aria-expanded="mobileOpen"
    aria-controls="research-assistant-mobile"
    @click="openMobile"
  >
    <CircleHelp :size="21" />
    <span class="sr-only">打开研究助手</span>
  </button>

  <Teleport to="body">
    <Transition name="drawer">
      <div v-if="mobileOpen" class="fixed inset-0 z-50 xl:hidden">
        <button class="absolute inset-0 bg-overlay" aria-label="关闭研究助手" tabindex="-1" @click="closeMobile()" />
        <aside
          id="research-assistant-mobile"
          ref="mobileDialog"
          role="dialog"
          aria-modal="true"
          :aria-labelledby="mobileTitleId"
          tabindex="-1"
          class="absolute inset-y-0 right-0 w-[min(88vw,340px)] overflow-y-auto bg-surface-raised shadow-drawer"
          @keydown="onDialogKeydown"
        >
          <div class="flex items-center justify-between border-b border-border px-4 py-3">
            <div class="flex items-center gap-2">
              <Lightbulb :size="17" class="text-accent" />
              <h2 :id="mobileTitleId" class="text-sm font-semibold">{{ guide.title }}</h2>
            </div>
            <button ref="closeButton" type="button" class="icon-button" title="关闭" @click="closeMobile()">
              <X :size="18" />
              <span class="sr-only">关闭</span>
            </button>
          </div>
          <div class="px-5 py-5">
            <p class="text-sm leading-6 text-text-secondary">{{ guide.summary }}</p>
            <dl v-if="guide.concepts?.length" class="mt-6 space-y-4">
              <div v-for="concept in guide.concepts" :key="concept.term">
                <dt class="text-sm font-medium">{{ concept.term }}</dt>
                <dd class="mt-1 text-sm leading-6 text-text-tertiary">{{ concept.explanation }}</dd>
              </div>
            </dl>
            <ol v-if="guide.steps?.length" class="mt-6 space-y-4">
              <li v-for="(step, index) in guide.steps" :key="step" class="flex gap-3 text-sm leading-6 text-text-secondary">
                <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-active text-xs font-medium text-accent">{{ index + 1 }}</span>
                <span>{{ step }}</span>
              </li>
            </ol>
            <p v-if="guide.note" class="mt-6 rounded-md bg-warning-soft px-3 py-2 text-sm leading-6 text-warning">{{ guide.note }}</p>
          </div>
        </aside>
      </div>
    </Transition>
  </Teleport>
</template>
