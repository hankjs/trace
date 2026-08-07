<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { api, type PromptTemplate } from '../composables/api'
import { useAiGenerate } from '../composables/useAiGenerate'

const { open } = useAiGenerate()

const templates = ref<PromptTemplate[]>([])
const name = ref('')
const content = ref('')
const category = ref<'prompt' | 'requirement' | 'task'>('prompt')
const filterCategory = ref<string>('')

const CATEGORIES = [
  { value: 'prompt', label: 'Prompt' },
  { value: 'requirement', label: '需求模板' },
  { value: 'task', label: '任务模板' },
]

const filteredTemplates = computed(() => {
  if (!filterCategory.value) return templates.value
  return templates.value.filter(t => t.category === filterCategory.value)
})

// Replay state
const replaySessionId = ref('')
const abMode = ref(false)
const promptA = ref('')
const promptB = ref('')

interface ReplayEvent {
  type: string
  [key: string]: unknown
}

interface ReplayResult {
  events: ReplayEvent[]
  running: boolean
  totalTokens: number
  steps: number
  toolCalls: number
  elapsed: number
}

function emptyResult(): ReplayResult {
  return { events: [], running: false, totalTokens: 0, steps: 0, toolCalls: 0, elapsed: 0 }
}

const resultA = ref<ReplayResult>(emptyResult())
const resultB = ref<ReplayResult>(emptyResult())

async function loadTemplates() {
  templates.value = await api.listPromptTemplates()
}

async function saveTemplate() {
  if (!name.value || !content.value) return
  await api.createPromptTemplate(name.value, content.value, category.value)
  name.value = ''
  content.value = ''
  await loadTemplates()
}

// PLACEHOLDER_PROMPTLAB_METHODS

async function deleteTemplate(id: string) {
  await api.deletePromptTemplate(id)
  await loadTemplates()
}

async function streamReplay(prompt: string, result: typeof resultA) {
  if (!replaySessionId.value) return
  result.value = { events: [], running: true, totalTokens: 0, steps: 0, toolCalls: 0, elapsed: 0 }
  const start = Date.now()

  const res = await api.replay(replaySessionId.value, {
    system_prompt: prompt || undefined,
  })

  const reader = res.body?.getReader()
  const decoder = new TextDecoder()
  if (!reader) { result.value.running = false; return }

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
          const event: ReplayEvent = JSON.parse(line.slice(6))
          result.value.events.push(event)
          // Accumulate stats
          if (event.type === 'metrics') {
            result.value.totalTokens += ((event.input_tokens as number) || 0) + ((event.output_tokens as number) || 0)
            result.value.steps++
          }
          if (event.type === 'tool_start') {
            result.value.toolCalls++
          }
        } catch { /* skip */ }
      }
    }
  }
  result.value.elapsed = Date.now() - start
  result.value.running = false
}

function runSingle() {
  streamReplay(content.value, resultA)
}

function runAB() {
  streamReplay(promptA.value, resultA)
  streamReplay(promptB.value, resultB)
}

function getTextOutput(result: ReplayResult): string {
  return result.events
    .filter(e => e.type === 'text_delta')
    .map(e => (e.text as string) || '')
    .join('')
}

function getEventSummary(result: ReplayResult): ReplayEvent[] {
  // Return non-text_delta events for mini timeline
  return result.events.filter(e => e.type !== 'text_delta')
}

const EVENT_BADGE: Record<string, string> = {
  thinking: 'text-blue-400 bg-blue-500/10',
  tool_start: 'text-gray-400 bg-gray-500/10',
  tool_result: 'text-gray-400 bg-gray-500/10',
  metrics: 'text-gray-300 bg-gray-400/10',
  error: 'text-red-400 bg-red-500/10',
  verification: 'text-orange-400 bg-orange-500/10',
  worker_spawned: 'text-green-400 bg-green-500/10',
  worker_completed: 'text-green-400 bg-green-500/10',
}

onMounted(loadTemplates)
</script>

// PLACEHOLDER_TEMPLATE

<template>
  <div>
    <h1 class="text-lg font-semibold text-text-primary mb-6">Prompt Lab</h1>

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-10">
      <!-- Left: Template management -->
      <div>
        <div class="space-y-3 mb-8">
          <input
            v-model="name"
            placeholder="Template name"
            class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
          />
          <select
            v-model="category"
            class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] text-text-primary focus:outline-none focus:border-accent transition-colors"
          >
            <option v-for="c in CATEGORIES" :key="c.value" :value="c.value">{{ c.label }}</option>
          </select>
          <textarea
            v-model="content"
            placeholder="System prompt..."
            rows="10"
            class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] font-mono leading-relaxed placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors resize-y"
          ></textarea>
          <div class="flex items-center gap-2">
            <button
              @click="saveTemplate"
              class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 transition-opacity"
            >Save</button>
            <button
              @click="open(text => content = text)"
              class="px-2 py-1.5 text-[13px] text-text-tertiary hover:text-accent transition-colors"
              title="AI 生成"
            >✨</button>
          </div>
        </div>

        <div v-if="templates.length">
          <div class="flex items-center gap-3 mb-3">
            <div class="text-[13px] text-text-tertiary font-medium">Saved</div>
            <select
              v-model="filterCategory"
              class="ml-auto bg-transparent border border-border rounded px-2 py-1 text-[11px] text-text-tertiary focus:outline-none focus:border-accent transition-colors"
            >
              <option value="">All</option>
              <option v-for="c in CATEGORIES" :key="c.value" :value="c.value">{{ c.label }}</option>
            </select>
          </div>
          <div class="divide-y divide-border-subtle">
            <div v-for="t in filteredTemplates" :key="t.id" class="flex items-center justify-between py-2.5">
              <div class="min-w-0">
                <div class="text-[13px] text-text-primary">{{ t.name }} <span class="text-text-tertiary">v{{ t.version }}</span> <span class="text-[10px] px-1.5 py-0.5 rounded bg-surface-raised text-text-tertiary">{{ t.category }}</span></div>
                <div class="text-[12px] text-text-tertiary truncate mt-0.5">{{ t.content.slice(0, 60) }}</div>
              </div>
              <button @click="content = t.content; name = t.name; category = t.category as any" class="text-[12px] text-accent hover:text-accent-hover shrink-0 ml-3 transition-colors">Load</button>
              <button @click="deleteTemplate(t.id)" class="text-[12px] text-text-tertiary hover:text-red-500 shrink-0 ml-2 transition-colors">删除</button>
            </div>
          </div>
        </div>
      </div>

      <!-- Right: Replay -->
      <div>
        <div class="flex items-center gap-3 mb-3">
          <div class="text-[13px] text-text-tertiary font-medium">Replay</div>
          <label class="flex items-center gap-1.5 text-[12px] text-text-tertiary cursor-pointer ml-auto">
            <input type="checkbox" v-model="abMode" class="rounded border-border" />
            A/B Mode
          </label>
        </div>

        <input
          v-model="replaySessionId"
          placeholder="Session ID"
          class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] font-mono placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors mb-3"
        />

        <!-- Single mode -->
        <div v-if="!abMode">
          <button
            @click="runSingle()"
            :disabled="resultA.running"
            class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 disabled:opacity-40 transition-opacity"
          >{{ resultA.running ? 'Running...' : 'Run' }}</button>

          <!-- Result display -->
          <div v-if="resultA.events.length" class="mt-5 space-y-4">
            <!-- Mini timeline -->
            <div class="flex flex-wrap gap-1">
              <span
                v-for="(ev, i) in getEventSummary(resultA)"
                :key="i"
                class="text-[10px] px-1.5 py-0.5 rounded"
                :class="EVENT_BADGE[ev.type] || 'text-gray-400 bg-gray-500/10'"
              >{{ ev.type === 'tool_start' ? (ev.name || 'tool') : ev.type }}</span>
            </div>
            <!-- Text output -->
            <pre class="text-[12px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-80 overflow-y-auto p-3 bg-surface-raised border border-border-subtle rounded-md">{{ getTextOutput(resultA) }}</pre>
            <!-- Stats -->
            <div class="flex gap-6 text-[11px] text-text-tertiary">
              <span>Tokens: <span class="text-text-secondary tabular-nums">{{ resultA.totalTokens.toLocaleString() }}</span></span>
              <span>Steps: <span class="text-text-secondary tabular-nums">{{ resultA.steps }}</span></span>
              <span>Tools: <span class="text-text-secondary tabular-nums">{{ resultA.toolCalls }}</span></span>
              <span>Time: <span class="text-text-secondary tabular-nums">{{ (resultA.elapsed / 1000).toFixed(1) }}s</span></span>
            </div>
          </div>
        </div>

        <!-- A/B mode -->
        <div v-else>
          <div class="mb-3 grid grid-cols-1 gap-4 sm:grid-cols-2">
            <textarea
              v-model="promptA"
              placeholder="Prompt A (system prompt)"
              rows="4"
              class="w-full resize-y rounded-md border border-border bg-transparent px-3 py-2 font-mono text-[12px] leading-relaxed placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
            ></textarea>
            <textarea
              v-model="promptB"
              placeholder="Prompt B (system prompt)"
              rows="4"
              class="w-full resize-y rounded-md border border-border bg-transparent px-3 py-2 font-mono text-[12px] leading-relaxed placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
            ></textarea>
          </div>
          <button
            @click="runAB()"
            :disabled="resultA.running || resultB.running"
            class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 disabled:opacity-40 transition-opacity"
          >{{ (resultA.running || resultB.running) ? 'Running...' : 'Run A/B' }}</button>

          <!-- A/B Results side by side -->
          <div v-if="resultA.events.length || resultB.events.length" class="mt-5 grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div>
              <div class="text-[11px] text-text-tertiary font-medium mb-2">Prompt A</div>
              <div class="flex flex-wrap gap-1 mb-2">
                <span
                  v-for="(ev, i) in getEventSummary(resultA)"
                  :key="i"
                  class="text-[10px] px-1.5 py-0.5 rounded"
                  :class="EVENT_BADGE[ev.type] || 'text-gray-400 bg-gray-500/10'"
                >{{ ev.type === 'tool_start' ? (ev.name || 'tool') : ev.type }}</span>
              </div>
              <pre class="text-[11px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-60 overflow-y-auto p-2.5 bg-surface-raised border border-border-subtle rounded-md">{{ getTextOutput(resultA) }}</pre>
            </div>
            <div>
              <div class="text-[11px] text-text-tertiary font-medium mb-2">Prompt B</div>
              <div class="flex flex-wrap gap-1 mb-2">
                <span
                  v-for="(ev, i) in getEventSummary(resultB)"
                  :key="i"
                  class="text-[10px] px-1.5 py-0.5 rounded"
                  :class="EVENT_BADGE[ev.type] || 'text-gray-400 bg-gray-500/10'"
                >{{ ev.type === 'tool_start' ? (ev.name || 'tool') : ev.type }}</span>
              </div>
              <pre class="text-[11px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-60 overflow-y-auto p-2.5 bg-surface-raised border border-border-subtle rounded-md">{{ getTextOutput(resultB) }}</pre>
            </div>
          </div>

          <!-- Comparison table -->
          <div v-if="!resultA.running && !resultB.running && (resultA.events.length || resultB.events.length)" class="mt-4">
            <table class="w-full text-[11px]">
              <thead>
                <tr class="text-text-tertiary border-b border-border-subtle">
                  <th class="text-left py-1.5 font-medium">Metric</th>
                  <th class="text-right py-1.5 font-medium">A</th>
                  <th class="text-right py-1.5 font-medium">B</th>
                </tr>
              </thead>
              <tbody class="text-text-secondary">
                <tr class="border-b border-border-subtle/50">
                  <td class="py-1.5">Tokens</td>
                  <td class="text-right tabular-nums">{{ resultA.totalTokens.toLocaleString() }}</td>
                  <td class="text-right tabular-nums">{{ resultB.totalTokens.toLocaleString() }}</td>
                </tr>
                <tr class="border-b border-border-subtle/50">
                  <td class="py-1.5">Steps</td>
                  <td class="text-right tabular-nums">{{ resultA.steps }}</td>
                  <td class="text-right tabular-nums">{{ resultB.steps }}</td>
                </tr>
                <tr class="border-b border-border-subtle/50">
                  <td class="py-1.5">Tool Calls</td>
                  <td class="text-right tabular-nums">{{ resultA.toolCalls }}</td>
                  <td class="text-right tabular-nums">{{ resultB.toolCalls }}</td>
                </tr>
                <tr>
                  <td class="py-1.5">Elapsed</td>
                  <td class="text-right tabular-nums">{{ (resultA.elapsed / 1000).toFixed(1) }}s</td>
                  <td class="text-right tabular-nums">{{ (resultB.elapsed / 1000).toFixed(1) }}s</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
