<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { api, type ClientNotification } from '../composables/api'

const items = ref<ClientNotification[]>([])
const loading = ref(true)
const error = ref('')
let timer: ReturnType<typeof setInterval> | null = null

async function load() {
  try {
    items.value = await api.listNotifications(200)
    error.value = ''
  } catch (e: any) {
    error.value = e.message
  }
  loading.value = false
}

const KIND_LABEL: Record<string, string> = {
  notification: '通知',
  bell: '响铃',
  command: '命令',
}

function kindLabel(kind: string) {
  return KIND_LABEL[kind] || kind
}

function relativeTime(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr.replace(' ', 'T') + (dateStr.endsWith('Z') ? '' : 'Z')).getTime()
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return '刚刚'
  if (mins < 60) return `${mins} 分钟前`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `${hrs} 小时前`
  return `${Math.floor(hrs / 24)} 天前`
}

function shortId(id: string | null) {
  return id ? id.slice(0, 8) : '-'
}

onMounted(() => {
  load()
  timer = setInterval(load, 10000)
})

onUnmounted(() => {
  if (timer) clearInterval(timer)
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">通知</h1>
      <span class="text-[12px] text-text-tertiary">每 10 秒自动刷新</span>
    </div>

    <p v-if="error" class="mb-4 text-[13px] text-red-400">{{ error }}</p>
    <p v-if="!loading && items.length === 0" class="text-[13px] text-text-tertiary">
      暂无通知。client 终端里的任务完成 / 审批请求 / 响铃会出现在这里。
    </p>

    <div class="space-y-2">
      <div
        v-for="n in items"
        :key="n.id"
        class="border border-border-subtle rounded-md px-3 py-2.5 flex items-start gap-3"
      >
        <span
          class="mt-0.5 px-1.5 py-0.5 text-[11px] rounded shrink-0"
          :class="{
            'bg-accent/15 text-accent': n.kind === 'notification',
            'bg-yellow-400/15 text-yellow-400': n.kind === 'bell',
            'bg-green-400/15 text-green-400': n.kind === 'command',
          }"
        >{{ kindLabel(n.kind) }}</span>
        <div class="flex-1 min-w-0">
          <div class="text-[13px] text-text-primary">{{ n.title }}</div>
          <div class="text-[12px] text-text-secondary mt-0.5 break-all">{{ n.body }}</div>
        </div>
        <div class="text-[11px] text-text-tertiary shrink-0 text-right">
          <div>{{ relativeTime(n.created_at) }}</div>
          <div class="mt-0.5 font-mono">{{ shortId(n.term_id) }}</div>
        </div>
      </div>
    </div>
  </div>
</template>
