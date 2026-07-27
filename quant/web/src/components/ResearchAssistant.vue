<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ChevronDown, ChevronUp, CircleHelp, Lightbulb, X } from 'lucide-vue-next'
import type { ResearchGuide } from '../guides'

const props = withDefaults(defineProps<{
  guide: ResearchGuide
  variant?: 'desktop' | 'mobile'
  sidebarCollapsed?: boolean
}>(), {
  variant: 'desktop',
  sidebarCollapsed: false,
})

const emit = defineEmits<{
  'expand-sidebar': []
}>()

const storedCollapsed = localStorage.getItem('quant_sidebar_guide_collapsed')
const detailsCollapsed = ref(storedCollapsed === 'true')
const mobileOpen = ref(false)
const mobileTrigger = ref<HTMLButtonElement | null>(null)
const mobileDialog = ref<HTMLElement | null>(null)
const closeButton = ref<HTMLButtonElement | null>(null)
const mobileTitleId = 'research-assistant-mobile-title'
const desktopPanelId = 'research-assistant-sidebar-panel'

watch(detailsCollapsed, (value) => {
  localStorage.setItem('quant_sidebar_guide_collapsed', String(value))
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

function openDesktop() {
  detailsCollapsed.value = false
  emit('expand-sidebar')
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
  <section
    v-if="variant === 'desktop'"
    class="shrink-0 border-t border-border bg-workbench"
    aria-label="研究助手"
  >
    <button
      v-if="sidebarCollapsed"
      type="button"
      class="flex h-10 w-full items-center justify-center text-accent transition-colors hover:bg-hover"
      title="展开研究提示"
      @click="openDesktop"
    >
      <Lightbulb :size="16" />
      <span class="sr-only">展开研究提示</span>
    </button>

    <template v-else>
      <button
        type="button"
        class="flex h-9 w-full items-center justify-between gap-2 px-3 text-left text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
        :aria-expanded="!detailsCollapsed"
        :aria-controls="desktopPanelId"
        :title="detailsCollapsed ? '展开研究提示' : '收起研究提示'"
        @click="detailsCollapsed = !detailsCollapsed"
      >
        <span class="flex min-w-0 items-center gap-2">
          <Lightbulb :size="14" class="shrink-0 text-accent" />
          <span class="truncate text-[11px] font-medium text-text-primary">{{ guide.title }}</span>
        </span>
        <ChevronDown v-if="detailsCollapsed" :size="14" class="shrink-0" />
        <ChevronUp v-else :size="14" class="shrink-0" />
      </button>

      <div
        v-if="!detailsCollapsed"
        :id="desktopPanelId"
        :key="guide.title"
        class="max-h-[36vh] overflow-y-auto border-t border-border-subtle px-3 py-3"
      >
        <p class="text-[11px] leading-5 text-text-secondary">{{ guide.summary }}</p>

        <dl v-if="guide.concepts?.length" class="mt-3 space-y-2.5">
          <div v-for="concept in guide.concepts" :key="concept.term">
            <dt class="text-[11px] font-medium text-text-primary">{{ concept.term }}</dt>
            <dd class="mt-0.5 text-[10px] leading-4 text-text-tertiary">{{ concept.explanation }}</dd>
          </div>
        </dl>

        <ol v-if="guide.steps?.length" class="mt-3 space-y-2">
          <li v-for="(step, index) in guide.steps" :key="step" class="flex gap-2 text-[10px] leading-4 text-text-secondary">
            <span class="flex h-4 w-4 shrink-0 items-center justify-center rounded bg-active text-[9px] font-medium text-accent">
              {{ index + 1 }}
            </span>
            <span>{{ step }}</span>
          </li>
        </ol>

        <p v-if="guide.note" class="mt-3 rounded bg-warning-soft px-2 py-1.5 text-[10px] leading-4 text-warning">
          {{ guide.note }}
        </p>
      </div>
    </template>
  </section>

  <template v-else>
    <button
      ref="mobileTrigger"
      type="button"
      class="fixed bottom-10 right-4 z-30 flex h-11 w-11 items-center justify-center rounded-full bg-surface-raised text-accent shadow-panel lg:hidden"
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
        <div v-if="mobileOpen" class="fixed inset-0 z-50 lg:hidden">
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
</template>
