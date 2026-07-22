<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue'
import { api, type ClientAgentInfo, type TermInfo } from '../composables/api'

const clients = ref<ClientAgentInfo[]>([])
const selectedClientId = ref('')
const terminals = ref<TermInfo[]>([])
const selectedTermId = ref('')
const output = ref('')
const input = ref('')
const autoRefresh = ref(true)
const error = ref('')

let outputTimer: ReturnType<typeof setInterval> | null = null
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
  } catch (e: any) {
    terminals.value = []
    error.value = e.message
  }
}

async function loadOutput() {
  if (!selectedClientId.value || !selectedTermId.value) return
  try {
    const res = await api.terminalOutput(selectedClientId.value, selectedTermId.value, 200)
    output.value = res.output
    error.value = ''
  } catch (e: any) {
    error.value = e.message
  }
}

async function send() {
  const data = input.value
  if (!selectedClientId.value || !selectedTermId.value || !data) return
  try {
    await api.terminalInput(selectedClientId.value, selectedTermId.value, data + '\n')
    input.value = ''
    setTimeout(loadOutput, 300)
  } catch (e: any) {
    error.value = e.message
  }
}

watch(selectedClientId, () => {
  selectedTermId.value = ''
  output.value = ''
  loadTerminals()
})

watch(selectedTermId, (id) => {
  output.value = ''
  loadOutput()
  if (outputTimer) clearInterval(outputTimer)
  outputTimer = null
  if (id && autoRefresh.value) {
    outputTimer = setInterval(loadOutput, 3000)
  }
})

watch(autoRefresh, (on) => {
  if (outputTimer) clearInterval(outputTimer)
  outputTimer = null
  if (on && selectedTermId.value) {
    outputTimer = setInterval(loadOutput, 3000)
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
  listTimer = setInterval(() => {
    loadClients()
    loadTerminals()
  }, 5000)
})

onUnmounted(() => {
  if (outputTimer) clearInterval(outputTimer)
  if (listTimer) clearInterval(listTimer)
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">终端</h1>
      <label class="flex items-center gap-1.5 text-[13px] text-text-secondary cursor-pointer">
        <input v-model="autoRefresh" type="checkbox" class="rounded" />
        自动刷新输出
      </label>
    </div>

    <p v-if="error" class="mb-4 text-[13px] text-red-400">{{ error }}</p>

    <div class="grid grid-cols-[220px_1fr] gap-6">
      <!-- 左：client 列表 + 终端列表 -->
      <div class="space-y-6">
        <div>
          <div class="text-[12px] text-text-tertiary font-medium mb-2 px-1">桌面 CLIENT</div>
          <div
            v-for="c in clients"
            :key="c.id"
            class="px-2 py-1.5 rounded-md cursor-pointer text-[13px] flex items-center gap-2 transition-colors"
            :class="c.id === selectedClientId ? 'bg-surface-raised text-text-primary' : 'text-text-secondary hover:bg-surface-raised/50'"
            @click="selectedClientId = c.id"
          >
            <span
              class="w-1.5 h-1.5 rounded-full shrink-0"
              :class="c.online ? 'bg-green-400' : 'bg-text-tertiary'"
            ></span>
            <span class="truncate">{{ c.hostname || shortId(c.id) }}</span>
          </div>
          <div v-if="clients.length === 0" class="text-[12px] text-text-tertiary px-1">
            暂无 client 注册
          </div>
        </div>

        <div v-if="selectedClientId">
          <div class="text-[12px] text-text-tertiary font-medium mb-2 px-1">终端会话</div>
          <div
            v-for="t in terminals"
            :key="t.id"
            class="px-2 py-1.5 rounded-md cursor-pointer text-[13px] transition-colors"
            :class="t.id === selectedTermId ? 'bg-surface-raised text-text-primary' : 'text-text-secondary hover:bg-surface-raised/50'"
            @click="selectedTermId = t.id"
          >
            <div class="flex items-center gap-2">
              <span
                class="w-1.5 h-1.5 rounded-full shrink-0"
                :class="t.alive ? 'bg-green-400' : 'bg-text-tertiary'"
              ></span>
              <span class="truncate">{{ t.foreground_cmd }}</span>
              <span class="text-[11px] text-text-tertiary ml-auto shrink-0">{{ shortId(t.id) }}</span>
            </div>
            <div class="text-[11px] text-text-tertiary truncate pl-3.5">{{ homeCwd(t.cwd) }}</div>
          </div>
          <div v-if="terminals.length === 0" class="text-[12px] text-text-tertiary px-1">
            该 client 没有终端会话
          </div>
        </div>
      </div>

      <!-- 右：输出 + 输入 -->
      <div v-if="selectedTermId" class="flex flex-col min-h-[480px]">
        <div class="flex-1 bg-[#0d1117] border border-border-subtle rounded-md p-3 overflow-auto max-h-[60vh]">
          <pre class="text-[12px] leading-relaxed text-gray-200 font-mono whitespace-pre-wrap break-all">{{ output || '（暂无输出）' }}</pre>
        </div>
        <div class="flex gap-2 mt-3">
          <input
            v-model="input"
            type="text"
            placeholder="输入命令，回车发送（如 ls -la）"
            class="flex-1 bg-transparent border border-border rounded-md px-3 py-2 text-[13px] font-mono placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
            @keydown.enter="send"
          />
          <button
            class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 transition-opacity"
            @click="send"
          >
            发送
          </button>
          <button
            class="px-3.5 py-1.5 border border-border text-text-secondary text-[13px] rounded-md hover:text-text-primary transition-colors"
            @click="loadOutput"
          >
            刷新
          </button>
        </div>
      </div>
      <div v-else class="flex items-center justify-center min-h-[480px] text-[13px] text-text-tertiary border border-dashed border-border-subtle rounded-md">
        选择一个终端会话查看输出
      </div>
    </div>
  </div>
</template>
