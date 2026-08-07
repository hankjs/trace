<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { api, type ClientAgentInfo, type TermInfo } from '../composables/api'
import { useTermChannel, type TermChannel } from '../composables/useTermChannel'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'

const clients = ref<ClientAgentInfo[]>([])
const selectedClientId = ref('')
const terminals = ref<TermInfo[]>([])
const selectedTermId = ref('')
const input = ref('')
const autoRefresh = ref(true)
const error = ref('')
const channelMode = ref<'rtc' | 'relay' | ''>('')

const selectedTerm = computed(() =>
  terminals.value.find((t) => t.id === selectedTermId.value),
)

const termEl = ref<HTMLElement | null>(null)
let xterm: Terminal | null = null
let channel: TermChannel | null = null
let unwatchMode: (() => void) | null = null

function selectedTermSize() {
  const t = selectedTerm.value
  return { cols: t?.cols || 80, rows: t?.rows || 24 }
}

function relTime(value: string | null | undefined): string {
  if (!value) return '—'
  const t = Date.parse(value)
  if (Number.isNaN(t)) return '—'
  const sec = Math.max(0, Math.floor((Date.now() - t) / 1000))
  if (sec < 60) return '刚刚'
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min}分钟前`
  const hr = Math.floor(min / 60)
  if (hr < 24) return `${hr}小时前`
  const day = Math.floor(hr / 24)
  return `${day}天前`
}

async function toggleClientEnabled(c: ClientAgentInfo) {
  try {
    await api.clientSetEnabled(c.id, !c.enabled)
    await loadClients()
  } catch (e: any) {
    error.value = e.message
  }
}

function ensureXterm() {
  if (xterm || !termEl.value) return
  const { cols, rows } = selectedTermSize()
  xterm = new Terminal({
    cols,
    rows,
    fontSize: 15,
    lineHeight: 1.15,
    fontFamily: "'Fira Code', Menlo, 'Symbols Nerd Font Mono', Monaco, monospace",
    cursorBlink: true,
    disableStdin: false,
    scrollback: 10000,
    theme: { background: '#0d1117', foreground: '#e6edf3' },
  })
  xterm.open(termEl.value)
  // 交互式按键直写通道（P2P 或中转）
  xterm.onData((data) => {
    channel?.write(new TextEncoder().encode(data))
  })
}

function syncXtermSize() {
  if (!xterm) return
  const { cols, rows } = selectedTermSize()
  if (xterm.cols !== cols || xterm.rows !== rows) {
    xterm.resize(cols, rows)
    channel?.resize(cols, rows)
  }
}

function stopChannel() {
  unwatchMode?.()
  unwatchMode = null
  channel?.close()
  channel = null
  channelMode.value = ''
}

function startChannel() {
  stopChannel()
  if (!selectedClientId.value || !selectedTermId.value || !xterm) return
  xterm.reset()
  const ch = useTermChannel(
    selectedClientId.value,
    selectedTermId.value,
    {
      onData(data) {
        xterm?.write(data)
      },
      onError(msg) {
        error.value = msg
      },
    },
    {
      getSize: () => selectedTermSize(),
    },
  )
  channel = ch
  channelMode.value = ch.mode.value
  // 跟踪 mode 变化（P2P → 中转）
  const stop = watch(ch.mode, (m) => {
    channelMode.value = m
  })
  unwatchMode = () => stop()
}

let listTimer: ReturnType<typeof setInterval> | null = null

async function loadClients() {
  try {
    clients.value = await api.listClients()
    if (!selectedClientId.value && clients.value.length > 0) {
      const online = clients.value.find((c) => c.online)
      selectedClientId.value = (online || clients.value[0]).id
    }
  } catch (e: any) {
    error.value = e.message
  }
}

async function loadTerminals() {
  if (!selectedClientId.value) return
  try {
    terminals.value = await api.listClientTerminals(selectedClientId.value)
    error.value = ''
    syncXtermSize()
  } catch (e: any) {
    terminals.value = []
    error.value = e.message
  }
}

async function send() {
  const data = input.value
  if (!selectedClientId.value || !selectedTermId.value || !data) return
  // TUI 把回车当 \r
  channel?.write(new TextEncoder().encode(data + '\r'))
  input.value = ''
}

watch(selectedClientId, () => {
  selectedTermId.value = ''
  stopChannel()
  xterm?.reset()
  loadTerminals()
})

watch(selectedTermId, async (id) => {
  stopChannel()
  xterm?.reset()
  if (id) {
    await nextTick()
    ensureXterm()
    syncXtermSize()
    startChannel()
  }
})

watch(autoRefresh, (on) => {
  if (listTimer) clearInterval(listTimer)
  listTimer = null
  if (on) {
    listTimer = setInterval(() => {
      loadClients()
      loadTerminals()
    }, 5000)
  }
})

function shortId(id: string) {
  return id.slice(0, 8)
}

function homeCwd(cwd: string) {
  return cwd.replace(/^\/Users\/[^/]+/, '~')
}

onMounted(async () => {
  await loadClients()
  await loadTerminals()
  if (autoRefresh.value) {
    listTimer = setInterval(() => {
      loadClients()
      loadTerminals()
    }, 5000)
  }
})

onUnmounted(() => {
  if (listTimer) clearInterval(listTimer)
  stopChannel()
  xterm?.dispose()
  xterm = null
})
</script>

<template>
  <div class="flex flex-col h-full min-h-0">
    <div class="flex items-center justify-between mb-6 shrink-0">
      <div class="flex items-center gap-3">
        <h1 class="text-lg font-semibold text-text-primary">终端</h1>
        <span
          v-if="channelMode"
          class="text-[11px] px-2 py-0.5 rounded-full border"
          :class="
            channelMode === 'rtc'
              ? 'border-green-500/40 text-green-400 bg-green-500/10'
              : 'border-border-subtle text-text-tertiary'
          "
        >
          {{ channelMode === 'rtc' ? 'P2P 直连' : '中转' }}
        </span>
      </div>
      <label class="flex items-center gap-1.5 text-[13px] text-text-secondary cursor-pointer">
        <input v-model="autoRefresh" type="checkbox" class="rounded" />
        自动刷新列表
      </label>
    </div>

    <p v-if="error" class="mb-4 text-[13px] text-red-400 shrink-0">{{ error }}</p>

    <div class="grid grid-cols-[260px_1fr] gap-6 flex-1 min-h-0">
      <div class="space-y-6 overflow-y-auto min-h-0">
        <div>
          <div class="text-[12px] text-text-tertiary font-medium mb-2 px-1">桌面 CLIENT</div>
          <div
            v-for="c in clients"
            :key="c.id"
            class="px-2 py-1.5 rounded-md cursor-pointer text-[13px] transition-colors"
            :class="[
              c.id === selectedClientId
                ? 'bg-surface-raised text-text-primary'
                : 'text-text-secondary hover:bg-surface-raised/50',
              c.enabled === false ? 'opacity-60' : '',
            ]"
            @click="selectedClientId = c.id"
          >
            <div class="flex items-center gap-2">
              <span
                class="w-1.5 h-1.5 rounded-full shrink-0"
                :class="c.online ? 'bg-green-400' : 'bg-text-tertiary'"
              ></span>
              <span class="truncate">{{ c.hostname || shortId(c.id) }}</span>
              <span
                v-if="c.enabled === false"
                class="text-[10px] text-text-tertiary shrink-0 px-1 rounded bg-surface-raised"
              >已停用</span>
              <button
                class="flex items-center shrink-0 ml-auto"
                title="停用/启用节点"
                @click.stop="toggleClientEnabled(c)"
              >
                <span
                  class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors"
                  :class="c.enabled !== false ? 'bg-green-500' : 'bg-border-subtle'"
                >
                  <span
                    class="inline-block h-3 w-3 rounded-full bg-white transition-transform"
                    :class="c.enabled !== false ? 'translate-x-3.5' : 'translate-x-0.5'"
                  ></span>
                </span>
              </button>
            </div>
            <div class="text-[11px] text-text-tertiary truncate pl-3.5">
              最后运行 {{ relTime(c.last_active_at) }} · 最后在线 {{ relTime(c.last_seen_at) }}
            </div>
          </div>
          <div v-if="clients.length === 0" class="text-[12px] text-text-tertiary px-1">
            暂无 client 注册（桌面端设置里开启「允许远程终端」）
          </div>
        </div>

        <div v-if="selectedClientId">
          <div class="text-[12px] text-text-tertiary font-medium mb-2 px-1">终端会话</div>
          <div
            v-for="t in terminals"
            :key="t.id"
            class="px-2 py-1.5 rounded-md cursor-pointer text-[13px] transition-colors"
            :class="
              t.id === selectedTermId
                ? 'bg-surface-raised text-text-primary'
                : 'text-text-secondary hover:bg-surface-raised/50'
            "
            @click="selectedTermId = t.id"
          >
            <div class="flex items-center gap-2">
              <span
                class="w-1.5 h-1.5 rounded-full shrink-0"
                :class="t.alive ? 'bg-green-400' : 'bg-text-tertiary'"
              ></span>
              <span class="truncate">{{ t.foreground_cmd || t.shell }}</span>
              <span class="text-[11px] text-text-tertiary shrink-0 ml-auto">{{ shortId(t.id) }}</span>
            </div>
            <div class="text-[11px] text-text-tertiary truncate pl-3.5">{{ homeCwd(t.cwd) }}</div>
          </div>
          <div v-if="terminals.length === 0" class="text-[12px] text-text-tertiary px-1">
            该 client 没有终端会话
          </div>
        </div>
      </div>

      <div v-if="selectedTermId" class="flex flex-col min-h-0">
        <div
          ref="termEl"
          class="flex-1 min-h-0 bg-[#0d1117] border border-border-subtle rounded-md overflow-hidden p-1"
        ></div>
        <div class="flex gap-2 mt-3">
          <input
            v-model="input"
            type="text"
            placeholder="输入命令，回车发送（也可直接在上方终端打字）"
            class="flex-1 bg-transparent border border-border rounded-md px-3 py-2 text-[13px] font-mono placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
            @keydown.enter="send"
          />
          <button
            class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 transition-opacity"
            @click="send"
          >
            发送
          </button>
        </div>
      </div>
      <div
        v-else
        class="flex items-center justify-center min-h-0 h-full text-[13px] text-text-tertiary border border-dashed border-border-subtle rounded-md"
      >
        选择一个终端会话查看输出
      </div>
    </div>
  </div>
</template>
