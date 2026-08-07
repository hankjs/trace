<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { TERM_FONT, TERM_THEME } from '../terminalTheme'
import { useTermChannel, type TermChannel } from '../composables/useTermChannel'

const props = defineProps<{
  clientId: string
  termId: string
  fill?: boolean
}>()

const emit = defineEmits<{ mode: ['rtc' | 'relay'] }>()

const container = ref<HTMLDivElement | null>(null)
const error = ref('')

const encoder = new TextEncoder()
let term: Terminal | null = null
let fitAddon: FitAddon | null = null
let channel: TermChannel | null = null
let stopModeWatch: (() => void) | null = null
let resizeObserver: ResizeObserver | null = null

function fit() {
  try {
    fitAddon?.fit()
  } catch {
    // 容器还没尺寸
  }
}

function onWindowResize() {
  fit()
}

function focusTerm() {
  term?.focus()
}

onMounted(async () => {
  await (document.fonts
    ?.load('13px "Symbols Nerd Font Mono"', '')
    .catch(() => {}) ?? Promise.resolve())

  term = new Terminal({
    cursorBlink: true,
    fontSize: 13,
    fontFamily: TERM_FONT,
    theme: TERM_THEME,
  })
  fitAddon = new FitAddon()
  term.loadAddon(fitAddon)
  term.open(container.value!)
  fit()
  requestAnimationFrame(fit)
  document.fonts?.ready.then(fit).catch(() => {})
  resizeObserver = new ResizeObserver(fit)
  resizeObserver.observe(container.value!)
  term.focus()

  channel = useTermChannel(
    props.clientId,
    props.termId,
    {
      onData: (data) => term?.write(data),
      onError: (message) => {
        error.value = message
      },
    },
    {
      getSize: () => ({ cols: term?.cols ?? 80, rows: term?.rows ?? 24 }),
    },
  )
  stopModeWatch = watch(channel.mode, (m) => emit('mode', m), { immediate: true })

  term.onData((data) => channel?.write(encoder.encode(data)))
  term.onResize(({ cols, rows }) => channel?.resize(cols, rows))

  // 强制把当前 fit 尺寸推到真实 PTY（中转路径 onResize 有时不会二次触发）
  const pushSize = () => {
    if (!term || !channel) return
    channel.resize(term.cols, term.rows)
  }
  requestAnimationFrame(pushSize)
  setTimeout(pushSize, 100)

  window.addEventListener('resize', onWindowResize)
})

onUnmounted(() => {
  window.removeEventListener('resize', onWindowResize)
  resizeObserver?.disconnect()
  resizeObserver = null
  stopModeWatch?.()
  channel?.close()
  channel = null
  term?.dispose()
  term = null
})
</script>

<template>
  <div class="relative" :class="fill ? 'h-full min-h-0' : ''">
    <div
      ref="container"
      class="term-block overflow-hidden"
      :class="fill ? 'h-full' : 'h-[min(70dvh,32rem)]'"
      @click="focusTerm"
    />
    <p
      v-if="error"
      class="absolute bottom-2 left-2 right-2 rounded bg-black/60 px-2 py-1 text-xs text-danger"
    >
      {{ error }}
    </p>
  </div>
</template>
