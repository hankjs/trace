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
  <div class="flex h-full min-h-0 flex-col">
    <div class="mb-4 flex shrink-0 flex-col gap-2 sm:mb-6 sm:flex-row sm:items-center sm:justify-between">
      <div class="flex flex-wrap items-center gap-2 sm:gap-3">
        <h1 class="text-lg font-semibold text-text-primary">终端</h1>
        <span
          v-if="channelMode"
          class="rounded-full border px-2 py-0.5 text-[11px]"
          :class="
            channelMode === 'rtc'
              ? 'border-green-500/40 bg-green-500/10 text-green-400'
              : 'border-border-subtle text-text-tertiary'
          "
        >
          {{ channelMode === 'rtc' ? 'P2P 直连' : '中转' }}
        </span>
      </div>
      <label class="flex min-h-10 cursor-pointer items-center gap-1.5 text-[13px] text-text-secondary sm:min-h-0">
        <input v-model="autoRefresh" type="checkbox" class="rounded" />
        自动刷新列表
      </label>
    </div>

    <p v-if="error" class="mb-4 shrink-0 text-[13px] text-red-400">{{ error }}</p>

    <div class="flex min-h-0 flex-1 flex-col gap-4 lg:grid lg:grid-cols-[minmax(200px,260px)_1fr] lg:gap-6">
      <!-- 列表区：移动端限高可滚；大屏占左栏 -->
      <div class="max-h-[40vh] min-h-0 shrink-0 space-y-4 overflow-y-auto thin-scrollbar lg:max-h-none lg:space-y-6">
        <div>
          <div class="mb-2 px-1 text-[12px] font-medium text-text-tertiary">桌面 CLIENT</div>
          <div
            v-for="c in clients"
            :key="c.id"
            class="cursor-pointer rounded-md px-2 py-2 text-[13px] transition-colors lg:py-1.5"
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
                class="h-1.5 w-1.5 shrink-0 rounded-full"
                :class="c.online ? 'bg-green-400' : 'bg-text-tertiary'"
              ></span>
              <span class="truncate">{{ c.hostname || shortId(c.id) }}</span>
              <span
                v-if="c.enabled === false"
                class="shrink-0 rounded bg-surface-raised px-1 text-[10px] text-text-tertiary"
              >已停用</span>
              <button
                type="button"
                class="ml-auto flex min-h-9 shrink-0 items-center lg:min-h-0"
                title="停用/启用节点"
                @click.stop="toggleClientEnabled(c)"
              >
                <span
                  class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors"
                  :class="c.enabled !== false ? 'bg-green-500' : 'bg-border-subtle'"
                >
                  <span
                    class="inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform"
                    :class="c.enabled !== false ? 'translate-x-4' : 'translate-x-0.5'"
                  ></span>
                </span>
              </button>
            </div>
            <div class="truncate pl-3.5 text-[11px] text-text-tertiary">
              最后运行 {{ relTime(c.last_active_at) }} · 最后在线 {{ relTime(c.last_seen_at) }}
            </div>
          </div>
          <div v-if="clients.length === 0" class="px-1 text-[12px] text-text-tertiary">
            暂无 client 注册（桌面端设置里开启「允许远程终端」）
          </div>
        </div>

        <div v-if="selectedClientId">
          <div class="mb-2 px-1 text-[12px] font-medium text-text-tertiary">终端会话</div>
          <div
            v-for="t in terminals"
            :key="t.id"
            class="cursor-pointer rounded-md px-2 py-2 text-[13px] transition-colors lg:py-1.5"
            :class="
              t.id === selectedTermId
                ? 'bg-surface-raised text-text-primary'
                : 'text-text-secondary hover:bg-surface-raised/50'
            "
            @click="selectedTermId = t.id"
          >
            <div class="flex items-center gap-2">
              <span
                class="h-1.5 w-1.5 shrink-0 rounded-full"
                :class="t.alive ? 'bg-green-400' : 'bg-text-tertiary'"
              ></span>
              <span class="truncate">{{ t.foreground_cmd || t.shell }}</span>
              <span class="ml-auto shrink-0 text-[11px] text-text-tertiary">{{ shortId(t.id) }}</span>
            </div>
            <div class="truncate pl-3.5 text-[11px] text-text-tertiary">{{ homeCwd(t.cwd) }}</div>
          </div>
          <div v-if="terminals.length === 0" class="px-1 text-[12px] text-text-tertiary">
            该 client 没有终端会话
          </div>
        </div>
      </div>

      <div v-if="selectedTermId" class="flex min-h-[50vh] min-w-0 flex-1 flex-col lg:min-h-0">
        <div
          ref="termEl"
          class="min-h-[240px] flex-1 overflow-hidden rounded-md border border-border-subtle bg-[#0d1117] p-1"
        ></div>
        <div class="mt-3 flex flex-col gap-2 sm:flex-row">
          <input
            v-model="input"
            type="text"
            placeholder="输入命令，回车发送（也可直接在上方终端打字）"
            class="min-w-0 flex-1 rounded-md border border-border bg-transparent px-3 py-2 font-mono text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
            @keydown.enter="send"
          />
          <button
            type="button"
            class="min-h-10 shrink-0 rounded-md bg-text-primary px-3.5 py-2 text-[13px] text-surface-raised transition-opacity hover:opacity-80 sm:min-h-0 sm:py-1.5"
            @click="send"
          >
            发送
          </button>
        </div>
      </div>
      <div
        v-else
        class="flex min-h-[200px] flex-1 items-center justify-center rounded-md border border-dashed border-border-subtle px-4 text-center text-[13px] text-text-tertiary lg:min-h-0 lg:h-full"
      >
        选择一个终端会话查看输出
      </div>
    </div>
  </div>
</template>
