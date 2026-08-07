<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { api, type AgentEventRecord } from '../composables/api'

const route = useRoute()
const sessionId = route.params.id as string

const events = ref<AgentEventRecord[]>([])
const loading = ref(true)
const expandedIds = ref<Set<string>>(new Set())
const activeFilters = ref<Set<string>>(new Set())

const EVENT_COLORS: Record<string, string> = {
  thinking: 'bg-blue-500',
  tool_start: 'bg-gray-400',
  tool_result: 'bg-gray-500',
  worker_spawned: 'bg-green-500',
  worker_completed: 'bg-green-600',
  verification: 'bg-orange-500',
  error: 'bg-red-500',
  metrics: 'bg-gray-300',
  tool_metrics: 'bg-gray-300',
  text_delta: 'bg-purple-400',
  turn_complete: 'bg-gray-600',
  provider_fallback: 'bg-yellow-500',
  llm_request: 'bg-blue-500',
  llm_response: 'bg-green-500',
  llm_retry: 'bg-yellow-500',
  llm_failed: 'bg-red-500',
}

const EVENT_LABELS: Record<string, string> = {
  thinking: 'Thinking',
  tool_start: 'Tool Start',
  tool_result: 'Tool Result',
  worker_spawned: 'Worker Spawned',
  worker_completed: 'Worker Completed',
  verification: 'Verification',
  error: 'Error',
  metrics: 'Metrics',
  tool_metrics: 'Tool Metrics',
  text_delta: 'Text Output',
  turn_complete: 'Turn Complete',
  provider_fallback: 'Fallback',
  llm_request: 'LLM Request',
  llm_response: 'LLM Response',
  llm_retry: 'LLM Retry',
  llm_failed: 'LLM Failed',
}

// PLACEHOLDER_TIMELINE_LOGIC

const eventTypes = computed(() => {
  const types = new Set(events.value.map(e => e.event_type))
  return Array.from(types)
})

// Merge consecutive text_delta events into single blocks
const mergedEvents = computed(() => {
  const filtered = activeFilters.value.size > 0
    ? events.value.filter(e => activeFilters.value.has(e.event_type))
    : events.value

  const result: Array<{ key: string; event_type: string; events: AgentEventRecord[]; first: AgentEventRecord }> = []
  let currentTextGroup: AgentEventRecord[] = []

  for (const ev of filtered) {
    if (ev.event_type === 'text_delta') {
      currentTextGroup.push(ev)
    } else {
      if (currentTextGroup.length > 0) {
        result.push({ key: currentTextGroup[0].id, event_type: 'text_delta', events: currentTextGroup, first: currentTextGroup[0] })
        currentTextGroup = []
      }
      result.push({ key: ev.id, event_type: ev.event_type, events: [ev], first: ev })
    }
  }
  if (currentTextGroup.length > 0) {
    result.push({ key: currentTextGroup[0].id, event_type: 'text_delta', events: currentTextGroup, first: currentTextGroup[0] })
  }
  return result
})

function toggleFilter(type: string) {
  if (activeFilters.value.has(type)) {
    activeFilters.value.delete(type)
  } else {
    activeFilters.value.add(type)
  }
}

function toggleExpand(key: string) {
  if (expandedIds.value.has(key)) {
    expandedIds.value.delete(key)
  } else {
    expandedIds.value.add(key)
  }
}

function formatTime(dateStr: string) {
  const d = new Date(dateStr)
  const base = d.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
  const ms = String(d.getMilliseconds()).padStart(3, '0')
  return `${base}.${ms}`
}

function timeDelta(index: number): string | null {
  if (index === 0) return null
  const prev = mergedEvents.value[index - 1]
  const curr = mergedEvents.value[index]
  const diff = new Date(curr.first.created_at).getTime() - new Date(prev.first.created_at).getTime()
  if (diff < 1000) return `+${diff}ms`
  return `+${(diff / 1000).toFixed(1)}s`
}

function getPayloadPreview(item: { event_type: string; events: AgentEventRecord[] }): string {
  if (item.event_type === 'text_delta') {
    const texts = item.events.map(e => {
      try { return JSON.parse(e.payload).text || '' } catch { return '' }
    })
    const joined = texts.join('')
    return joined.length > 120 ? joined.slice(0, 120) + '...' : joined
  }
  try {
    const p = JSON.parse(item.events[0].payload)
    if (p.name) return p.name + (p.input ? ': ' + p.input.slice(0, 80) : '')
    if (p.message) return p.message.slice(0, 120)
    if (p.text) return p.text.slice(0, 120)
    if (p.description) return p.description.slice(0, 120)
    if (p.task_id) return `task: ${p.task_id}`
    if (p.tool_name) return `${p.tool_name} (${p.duration_ms}ms)`
    if (item.event_type === 'llm_request') return `${p.phase} · ${p.provider || ''}/${p.model} · ${p.message_count} messages · ${p.tools?.length || 0} tools`
    if (item.event_type === 'llm_response') return `${p.phase} · ${p.stop_reason} · in:${p.input_tokens} out:${p.output_tokens} · ${p.latency_ms}ms`
    if (item.event_type === 'llm_retry') return `${p.stage} · #${p.failed_attempt} → #${p.next_attempt} · ${p.error}`
    if (item.event_type === 'llm_failed') return `${p.stage} · #${p.attempt} · ${p.error}`
    if (p.input_tokens !== undefined) return `in:${p.input_tokens} out:${p.output_tokens} ${p.latency_ms}ms`
    return JSON.stringify(p).slice(0, 100)
  } catch { return '' }
}

function getFullPayload(item: { event_type: string; events: AgentEventRecord[] }): string {
  if (item.event_type === 'text_delta') {
    return item.events.map(e => {
      try { return JSON.parse(e.payload).text || '' } catch { return '' }
    }).join('')
  }
  try {
    return JSON.stringify(JSON.parse(item.events[0].payload), null, 2)
  } catch { return item.events[0].payload }
}

onMounted(async () => {
  try {
    events.value = await api.sessionEvents(sessionId)
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div>
    <RouterLink to="/sessions" class="text-[12px] text-text-tertiary transition-colors hover:text-text-secondary">← Sessions</RouterLink>
    <div class="mb-6 mt-2 flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-3">
      <div class="flex min-w-0 flex-wrap items-center gap-2 sm:gap-3">
        <h1 class="text-lg font-semibold text-text-primary">Timeline</h1>
        <span class="font-mono text-[12px] text-text-tertiary">{{ sessionId.slice(0, 8) }}</span>
      </div>
      <RouterLink :to="`/sessions/${sessionId}`" class="min-h-9 text-[12px] text-accent transition-colors hover:text-accent-hover sm:ml-auto sm:min-h-0">Detail →</RouterLink>
    </div>

    <!-- Filters -->
    <div class="mb-6 flex flex-wrap gap-1.5">
      <button
        v-for="type in eventTypes"
        :key="type"
        type="button"
        @click="toggleFilter(type)"
        class="min-h-8 rounded-full border px-2.5 py-1 text-[11px] transition-all"
        :class="activeFilters.size === 0 || activeFilters.has(type)
          ? 'border-accent bg-accent/5 text-accent'
          : 'border-border text-text-tertiary hover:border-border-subtle'"
      >{{ EVENT_LABELS[type] || type }}</button>
    </div>

    <div v-if="loading" class="py-12 text-center text-[13px] text-text-tertiary">Loading...</div>

    <!-- Timeline -->
    <div v-else-if="mergedEvents.length" class="relative pl-6">
      <!-- Vertical line -->
      <div class="absolute left-[9px] top-2 bottom-2 w-px bg-border"></div>

      <div v-for="(item, idx) in mergedEvents" :key="item.key" class="relative mb-3">
        <!-- Dot -->
        <div
          class="absolute -left-6 top-1.5 w-[10px] h-[10px] rounded-full border-2 border-surface-base"
          :class="EVENT_COLORS[item.event_type] || 'bg-gray-400'"
        ></div>

        <!-- Content -->
        <div
          class="group cursor-pointer rounded-md border border-transparent hover:border-border-subtle hover:bg-surface-raised/50 px-2.5 py-1.5 transition-all"
          @click="toggleExpand(item.key)"
        >
          <div class="flex items-center gap-2">
            <span class="text-[11px] text-text-tertiary tabular-nums shrink-0">{{ formatTime(item.first.created_at) }}</span>
            <span v-if="timeDelta(idx)" class="text-[10px] text-text-tertiary/60 tabular-nums shrink-0">{{ timeDelta(idx) }}</span>
            <span
              class="text-[11px] font-medium px-1.5 py-0.5 rounded"
              :class="{
                'text-blue-400 bg-blue-500/10': item.event_type === 'thinking',
                'text-gray-400 bg-gray-500/10': ['tool_start', 'tool_result', 'metrics', 'tool_metrics'].includes(item.event_type),
                'text-green-400 bg-green-500/10': ['worker_spawned', 'worker_completed'].includes(item.event_type),
                'text-orange-400 bg-orange-500/10': item.event_type === 'verification',
                'text-red-400 bg-red-500/10': item.event_type === 'error',
                'text-purple-400 bg-purple-500/10': item.event_type === 'text_delta',
                'text-yellow-400 bg-yellow-500/10': item.event_type === 'provider_fallback',
                'text-blue-500 bg-blue-500/10': item.event_type === 'llm_request',
                'text-green-600 bg-green-500/10': item.event_type === 'llm_response',
                'text-yellow-600 bg-yellow-500/10': item.event_type === 'llm_retry',
                'text-red-500 bg-red-500/10': item.event_type === 'llm_failed',
              }"
            >{{ EVENT_LABELS[item.event_type] || item.event_type }}</span>
            <span v-if="item.event_type === 'text_delta' && item.events.length > 1" class="text-[10px] text-text-tertiary">({{ item.events.length }} chunks)</span>
          </div>
          <div class="text-[12px] text-text-secondary mt-1 truncate max-w-2xl">{{ getPayloadPreview(item) }}</div>
        </div>

        <!-- Expanded content -->
        <div v-if="expandedIds.has(item.key)" class="ml-2.5 mt-1.5 mb-2">
          <pre class="text-[11px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-80 overflow-y-auto p-3 bg-surface-raised border border-border-subtle rounded-md">{{ getFullPayload(item) }}</pre>
        </div>
      </div>
    </div>

    <div v-else class="py-12 text-center text-[13px] text-text-tertiary">No events recorded for this session</div>
  </div>
</template>
