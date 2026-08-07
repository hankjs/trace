<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { api, type DbMessage, type AgentMetric, type ToolExecution, type PromptTemplate } from '../composables/api'
import { useAiGenerate } from '../composables/useAiGenerate'

const { open } = useAiGenerate()
const route = useRoute()
const sessionId = route.params.id as string

const messages = ref<DbMessage[]>([])
const metrics = ref<AgentMetric[]>([])
const toolExecs = ref<ToolExecution[]>([])
const expandedTools = ref<Set<string>>(new Set())

// Debug panel state
const activeDebugMsgId = ref<string | null>(null)
const debugPrompt = ref('')
const debugOutput = ref<string[]>([])
const isDebugRunning = ref(false)
const promptTemplates = ref<PromptTemplate[]>([])

onMounted(async () => {
  const [data, templates] = await Promise.all([
    api.sessionReplay(sessionId),
    api.listPromptTemplates(),
  ])
  messages.value = data.messages
  metrics.value = data.metrics
  toolExecs.value = data.tool_executions
  promptTemplates.value = templates
})

function toggleDebug(msgId: string) {
  if (activeDebugMsgId.value === msgId) {
    activeDebugMsgId.value = null
    debugPrompt.value = ''
    debugOutput.value = []
  } else {
    activeDebugMsgId.value = msgId
    debugPrompt.value = ''
    debugOutput.value = []
  }
}

function selectTemplate(e: Event) {
  const id = (e.target as HTMLSelectElement).value
  const tpl = promptTemplates.value.find(t => t.id === id)
  if (tpl) debugPrompt.value = tpl.content
}

async function runDebugReplay() {
  if (!debugPrompt.value.trim() || isDebugRunning.value) return
  isDebugRunning.value = true
  debugOutput.value = []

  const res = await api.replay(sessionId, { system_prompt: debugPrompt.value })
  const reader = res.body?.getReader()
  const decoder = new TextDecoder()
  if (!reader) { isDebugRunning.value = false; return }

  let buffer = ''
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split('\n')
    buffer = lines.pop() || ''
    for (const line of lines) {
      if (line.startsWith('data: ')) {
        try {
          const event = JSON.parse(line.slice(6))
          if (event.type === 'text_delta') {
            debugOutput.value.push(event.text)
          }
        } catch { /* skip */ }
      }
    }
  }
  isDebugRunning.value = false
}

function parseContent(content: string) {
  try {
    const parsed = JSON.parse(content)
    return Array.isArray(parsed) ? parsed : [{ type: 'text', text: content }]
  } catch {
    return [{ type: 'text', text: content }]
  }
}

function toggleTool(id: string) {
  if (expandedTools.value.has(id)) {
    expandedTools.value.delete(id)
  } else {
    expandedTools.value.add(id)
  }
}

function formatJson(obj: unknown): string {
  if (typeof obj === 'string') return obj
  try {
    return JSON.stringify(obj, null, 2)
  } catch {
    return String(obj)
  }
}

function findToolResult(_blocks: any[], toolUseId: string): any | null {
  // Look in all messages for the matching tool_result
  for (const msg of messages.value) {
    const parsed = parseContent(msg.content)
    const result = parsed.find((b: any) => b.type === 'tool_result' && b.tool_use_id === toolUseId)
    if (result) return result
  }
  return null
}

const totalTokens = () => metrics.value.reduce((a, m) => a + m.input_tokens + m.output_tokens, 0)
const avgLatency = () => {
  if (!metrics.value.length) return 0
  return Math.round(metrics.value.reduce((a, m) => a + m.latency_ms, 0) / metrics.value.length)
}
</script>

<template>
  <div>
    <RouterLink to="/sessions" class="text-[12px] text-text-tertiary transition-colors hover:text-text-secondary">← Sessions</RouterLink>
    <div class="mb-6 mt-2 flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center sm:gap-3">
      <h1 class="text-lg font-semibold text-text-primary">{{ sessionId.slice(0, 8) }}</h1>
      <div class="flex flex-wrap gap-3">
        <RouterLink :to="`/sessions/${sessionId}/timeline`" class="min-h-9 text-[12px] text-accent transition-colors hover:text-accent-hover sm:min-h-0">Timeline →</RouterLink>
        <RouterLink :to="`/sessions/${sessionId}/explore`" class="min-h-9 text-[12px] text-accent transition-colors hover:text-accent-hover sm:min-h-0">Explore →</RouterLink>
      </div>
    </div>

    <div v-if="metrics.length" class="mb-8 flex flex-wrap gap-x-6 gap-y-2 text-[12px] sm:gap-8">
      <div><span class="text-text-tertiary">Tokens</span> <span class="ml-1.5 tabular-nums">{{ totalTokens().toLocaleString() }}</span></div>
      <div><span class="text-text-tertiary">Avg latency</span> <span class="ml-1.5 tabular-nums">{{ avgLatency() }}ms</span></div>
      <div><span class="text-text-tertiary">Tools</span> <span class="ml-1.5 tabular-nums">{{ toolExecs.length }}</span></div>
    </div>

    <div class="space-y-4">
      <div v-for="msg in messages" :key="msg.id" v-show="parseContent(msg.content).some((b: any) => b.type !== 'tool_result')">
        <div class="flex items-center gap-2 mb-1.5">
          <span
            class="text-[11px] font-medium uppercase tracking-wide"
            :class="msg.role === 'user' ? 'text-accent' : 'text-text-tertiary'"
          >{{ msg.role }}</span>
          <span class="text-[11px] text-text-tertiary">{{ new Date(msg.created_at).toLocaleTimeString() }}</span>
          <button
            v-if="msg.role === 'user'"
            @click="toggleDebug(msg.id)"
            class="ml-auto text-[11px] px-2 py-0.5 rounded border transition-colors"
            :class="activeDebugMsgId === msg.id ? 'border-accent text-accent' : 'border-border-subtle text-text-tertiary hover:text-text-secondary hover:border-border'"
          >调试</button>
        </div>
        <div class="pl-0">
          <template v-for="(block, i) in parseContent(msg.content)" :key="i">
            <pre v-if="block.type === 'text' && block.text" class="text-[13px] text-text-primary whitespace-pre-wrap font-[inherit] leading-relaxed">{{ block.text }}</pre>

            <!-- Tool Use: show as a card with name, input, and linked output -->
            <div v-else-if="block.type === 'tool_use'" class="my-2 border border-border-subtle rounded-lg overflow-hidden">
              <div
                class="flex items-center gap-2 px-3 py-2 bg-hover cursor-pointer select-none"
                @click="toggleTool(block.id)"
              >
                <span class="text-[11px] text-accent font-mono font-medium">⚡ {{ block.name }}</span>
                <span class="ml-auto text-[11px] text-text-tertiary">{{ expandedTools.has(block.id) ? '▼' : '▶' }}</span>
              </div>
              <div v-if="expandedTools.has(block.id)" class="border-t border-border-subtle">
                <div class="px-3 py-2">
                  <div class="text-[11px] text-text-tertiary uppercase tracking-wide mb-1">Input</div>
                  <pre class="text-[12px] text-text-secondary font-mono whitespace-pre-wrap break-all leading-relaxed max-h-[300px] overflow-auto">{{ formatJson(block.input) }}</pre>
                </div>
                <div v-if="findToolResult(parseContent(msg.content), block.id)" class="border-t border-border-subtle px-3 py-2">
                  <div class="text-[11px] text-text-tertiary uppercase tracking-wide mb-1">Output</div>
                  <pre class="text-[12px] text-text-secondary font-mono whitespace-pre-wrap break-all leading-relaxed max-h-[300px] overflow-auto">{{ formatJson(findToolResult(parseContent(msg.content), block.id)?.content) }}</pre>
                </div>
              </div>
            </div>

            <!-- Tool Result: skip rendering, already shown as Output inside the tool_use card -->
            <template v-else-if="block.type === 'tool_result'"></template>
          </template>
        </div>

        <!-- Inline debug panel -->
        <div v-if="msg.role === 'user' && activeDebugMsgId === msg.id" class="mt-3 border border-border-subtle rounded-lg p-4 bg-hover/30">
          <div class="flex items-center gap-3 mb-3">
            <select
              @change="selectTemplate"
              class="flex-1 bg-transparent border border-border rounded-md px-2.5 py-1.5 text-[12px] text-text-secondary focus:outline-none focus:border-accent transition-colors"
            >
              <option value="">选择模板...</option>
              <option v-for="t in promptTemplates" :key="t.id" :value="t.id">{{ t.name }} (v{{ t.version }})</option>
            </select>
            <button
              @click="runDebugReplay"
              :disabled="isDebugRunning || !debugPrompt.trim()"
              class="px-3 py-1.5 bg-text-primary text-surface-raised text-[12px] rounded-md hover:opacity-80 disabled:opacity-40 transition-opacity shrink-0"
            >{{ isDebugRunning ? 'Running...' : 'Run' }}</button>
            <button
              @click="open(text => debugPrompt = text)"
              class="px-2 py-1.5 text-[12px] text-text-tertiary hover:text-accent transition-colors shrink-0"
              title="AI 生成"
            >✨</button>
            <button
              @click="toggleDebug(msg.id)"
              class="text-[11px] text-text-tertiary hover:text-text-secondary transition-colors shrink-0"
            >收起</button>
          </div>
          <textarea
            v-model="debugPrompt"
            placeholder="System prompt..."
            rows="5"
            class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[12px] font-mono leading-relaxed placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors resize-y"
          ></textarea>
          <div v-if="debugOutput.length" class="mt-3">
            <div class="text-[11px] text-text-tertiary uppercase tracking-wide mb-1.5">Output</div>
            <pre class="text-[12px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-80 overflow-y-auto p-3 bg-surface-raised border border-border-subtle rounded-md">{{ debugOutput.join('') }}</pre>
          </div>
        </div>
      </div>
      <div v-if="!messages.length" class="py-12 text-center text-[13px] text-text-tertiary">No messages</div>
    </div>

    <div v-if="toolExecs.length" class="mt-10">
      <div class="text-[13px] text-text-tertiary font-medium mb-3">Tool executions</div>
      <div class="divide-y divide-border-subtle">
        <div v-for="t in toolExecs" :key="t.id" class="flex items-center justify-between py-2 text-[12px]">
          <span class="text-text-secondary font-mono">{{ t.tool_name }}</span>
          <span :class="t.is_error ? 'text-red-500' : 'text-text-tertiary'" class="tabular-nums">{{ t.duration_ms }}ms</span>
        </div>
      </div>
    </div>
  </div>
</template>
