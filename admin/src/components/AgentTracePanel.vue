<script setup lang="ts">
import { computed, shallowRef, watch } from 'vue'
import type { AgentEventRecord } from '../composables/api'
import { buildAgentTrace, type ParsedAgentEvent, type TraceCall } from '../utils/agentTrace'

const props = defineProps<{
  events: AgentEventRecord[]
  loading?: boolean
  error?: string
}>()

const trace = computed(() => buildAgentTrace(props.events))
const expandedCalls = shallowRef<Set<string>>(new Set())

watch(
  () => trace.value.calls.map(call => call.key).join(','),
  () => {
    const first = trace.value.calls[0]
    expandedCalls.value = first ? new Set([first.key]) : new Set()
  },
  { immediate: true },
)

function toggleCall(key: string) {
  const next = new Set(expandedCalls.value)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  expandedCalls.value = next
}

function formatTime(value: string) {
  const date = new Date(value)
  const base = date.toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
  return `${base}.${String(date.getMilliseconds()).padStart(3, '0')}`
}

function formatDuration(ms: number) {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`
}

function formatJson(value: unknown) {
  if (typeof value === 'string') {
    try { return JSON.stringify(JSON.parse(value), null, 2) } catch { return value }
  }
  try { return JSON.stringify(value, null, 2) } catch { return String(value) }
}

function phaseLabel(phase: string) {
  return ({ simple: '主 Agent', act: '执行', think: '思考', worker: 'Worker', verify: '验证' } as Record<string, string>)[phase] || phase
}

function callLatency(call: TraceCall) {
  const payload = call.response?.payload || call.metrics?.payload
  if (payload?.latency_ms !== undefined) return Number(payload.latency_ms)
  return Math.max(0, new Date(call.endedAt).getTime() - new Date(call.startedAt).getTime())
}

function callUsage(call: TraceCall) {
  return call.response?.payload || call.metrics?.payload || {}
}

function callStatus(call: TraceCall) {
  if (call.failures.length) return '失败'
  if (call.response?.payload.timed_out) return '超时'
  if (call.response?.payload.cancelled) return '取消'
  if (call.response) return String(call.response.payload.stop_reason || '完成')
  return '历史记录'
}

function controlLabel(type: string) {
  return ({
    run_started: '运行开始', run_completed: '运行完成', run_failed: '运行失败', run_cancelled: '运行取消',
    turn_started: '轮次开始', turn_completed: '轮次完成', provider_fallback: '供应商回退',
    permission_requested: '请求权限', permission_denied: '权限拒绝', compression_triggered: '上下文压缩',
    token_warning: 'Token 预警', loop_detected: '循环检测', file_changed: '文件变更',
    context_assembled: '上下文组装', worker_spawned: 'Worker 启动', worker_completed: 'Worker 完成',
    verification_started: '验证开始', verification_completed: '验证完成', verification: '验证结果',
    error: '错误', ask_user: '等待用户',
  } as Record<string, string>)[type] || type
}

function controlPreview(event: ParsedAgentEvent) {
  const payload = event.payload
  if (payload.message) return payload.message
  if (payload.reason) return payload.reason
  if (payload.summary) return payload.summary
  if (payload.text) return payload.text
  if (payload.command) return payload.command
  if (payload.changes) return formatJson(payload.changes)
  if (payload.before_tokens !== undefined) return `${payload.before_tokens} → ${payload.after_tokens} tokens`
  return formatJson(payload)
}
</script>

<template>
  <div class="min-h-0">
    <div v-if="loading" class="px-4 py-10 text-xs text-text-tertiary">加载调用链...</div>
    <p v-else-if="error" class="m-4 rounded-md border border-red-300/50 bg-red-50 px-3 py-2 text-xs text-red-600">{{ error }}</p>
    <div v-else-if="!events.length" class="px-4 py-10 text-xs text-text-tertiary">该 Session 暂无 Agent 事件记录</div>
    <template v-else>
      <div class="flex flex-wrap items-center gap-x-5 gap-y-1 border-b border-border-subtle px-4 py-2.5 text-[11px] text-text-tertiary">
        <span><b class="font-medium text-text-primary">{{ trace.calls.length }}</b> 次模型调用</span>
        <span><b class="font-medium text-text-primary">{{ trace.toolCount }}</b> 次工具调用</span>
        <span>输入 <b class="font-medium tabular-nums text-text-secondary">{{ trace.inputTokens.toLocaleString() }}</b></span>
        <span>输出 <b class="font-medium tabular-nums text-text-secondary">{{ trace.outputTokens.toLocaleString() }}</b></span>
        <span v-if="trace.cacheReadTokens || trace.cacheWriteTokens">缓存 {{ trace.cacheReadTokens.toLocaleString() }} 读 / {{ trace.cacheWriteTokens.toLocaleString() }} 写</span>
        <span class="ml-auto font-mono tabular-nums">{{ formatDuration(trace.elapsedMs) }}</span>
      </div>

      <div v-if="trace.calls.length" class="divide-y divide-border-subtle">
        <section v-for="(call, index) in trace.calls" :key="call.key">
          <button
            class="grid w-full grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2 px-4 py-2.5 text-left transition-colors hover:bg-hover"
            :aria-expanded="expandedCalls.has(call.key)"
            @click="toggleCall(call.key)"
          >
            <span class="font-mono text-xs text-text-tertiary">{{ expandedCalls.has(call.key) ? '⌄' : '›' }}</span>
            <span class="min-w-0">
              <span class="flex flex-wrap items-center gap-x-2 gap-y-1">
                <b class="text-xs font-semibold text-text-primary">#{{ index + 1 }} {{ phaseLabel(call.phase) }}</b>
                <span v-if="call.provider || call.model" class="truncate font-mono text-[10px] text-text-tertiary">{{ [call.provider, call.model].filter(Boolean).join(' / ') }}</span>
                <span v-if="call.retries.length" class="rounded bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-700">重试 {{ call.retries.length }}</span>
              </span>
              <span class="mt-0.5 flex flex-wrap gap-x-3 text-[10px] text-text-tertiary">
                <span>{{ formatTime(call.startedAt) }}</span>
                <span v-if="call.callId" class="font-mono">call {{ call.callId.slice(0, 12) }}</span>
                <span v-if="call.turnId" class="font-mono">turn {{ call.turnId.slice(0, 12) }}</span>
              </span>
            </span>
            <span class="flex items-center gap-3 text-right text-[10px] text-text-tertiary">
              <span v-if="call.tools.length">{{ call.tools.length }} tools</span>
              <span class="tabular-nums">{{ callUsage(call).input_tokens || 0 }}→{{ callUsage(call).output_tokens || 0 }} tok</span>
              <span class="w-14 tabular-nums">{{ formatDuration(callLatency(call)) }}</span>
              <span :class="call.failures.length || call.response?.payload.is_error ? 'text-red-600' : 'text-text-secondary'">{{ callStatus(call) }}</span>
            </span>
          </button>

          <div v-if="expandedCalls.has(call.key)" class="border-t border-border-subtle bg-surface-raised/35">
            <section class="px-4 py-3">
              <div class="mb-2 flex items-center justify-between gap-3">
                <h3 class="text-[11px] font-semibold text-text-secondary">模型请求</h3>
                <span class="font-mono text-[10px] text-text-tertiary">max_tokens {{ call.request?.payload.max_tokens ?? '—' }}</span>
              </div>
              <div v-if="call.request" class="space-y-3">
                <details open>
                  <summary class="cursor-pointer select-none text-[11px] font-medium text-text-tertiary">System Prompt</summary>
                  <pre class="mt-1.5 max-h-96 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-3 py-2 font-mono text-[11px] leading-relaxed text-text-secondary">{{ call.request.payload.system || '—' }}</pre>
                </details>
                <details>
                  <summary class="cursor-pointer select-none text-[11px] font-medium text-text-tertiary">Messages · {{ call.request.payload.messages?.length ?? call.request.payload.message_count ?? 0 }}</summary>
                  <div class="mt-1.5 divide-y divide-border-subtle border border-border-subtle bg-surface">
                    <div v-for="(message, messageIndex) in call.request.payload.messages || []" :key="messageIndex" class="px-3 py-2">
                      <div class="mb-1 font-mono text-[10px] font-medium text-text-tertiary">{{ messageIndex + 1 }} · {{ message.role }}</div>
                      <pre class="max-h-80 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-text-secondary">{{ formatJson(message.content) }}</pre>
                    </div>
                    <div v-if="!call.request.payload.messages" class="px-3 py-2 text-[11px] text-text-tertiary">旧记录仅保存了消息数量</div>
                  </div>
                </details>
                <details>
                  <summary class="cursor-pointer select-none text-[11px] font-medium text-text-tertiary">可用工具 · {{ call.request.payload.tool_definitions?.length ?? call.request.payload.tools?.length ?? 0 }}</summary>
                  <div class="mt-1.5 divide-y divide-border-subtle border border-border-subtle bg-surface">
                    <div v-for="tool in call.request.payload.tool_definitions || []" :key="tool.name" class="px-3 py-2">
                      <div class="font-mono text-[11px] font-semibold text-text-primary">{{ tool.name }}</div>
                      <p class="mt-1 whitespace-pre-wrap text-[11px] leading-relaxed text-text-secondary">{{ tool.description }}</p>
                      <pre class="mt-1.5 max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono text-[10px] leading-relaxed text-text-tertiary">{{ formatJson(tool.input_schema) }}</pre>
                    </div>
                    <pre v-if="!call.request.payload.tool_definitions" class="px-3 py-2 whitespace-pre-wrap font-mono text-[11px] text-text-secondary">{{ formatJson(call.request.payload.tools || []) }}</pre>
                  </div>
                </details>
              </div>
              <div v-else class="text-[11px] text-text-tertiary">旧记录未保存模型请求</div>
            </section>

            <section class="border-t border-border-subtle px-4 py-3">
              <div class="mb-2 flex flex-wrap items-center gap-x-4 gap-y-1">
                <h3 class="text-[11px] font-semibold text-text-secondary">模型响应</h3>
                <span class="font-mono text-[10px] text-text-tertiary">stop {{ call.response?.payload.stop_reason || '—' }}</span>
                <span class="font-mono text-[10px] text-text-tertiary">{{ callUsage(call).input_tokens || 0 }} in / {{ callUsage(call).output_tokens || 0 }} out</span>
                <span v-if="callUsage(call).cache_read_tokens || callUsage(call).cache_write_tokens" class="font-mono text-[10px] text-text-tertiary">cache {{ callUsage(call).cache_read_tokens || 0 }} / {{ callUsage(call).cache_write_tokens || 0 }}</span>
              </div>
              <pre class="max-h-96 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-3 py-2 font-mono text-[11px] leading-relaxed text-text-secondary">{{ call.response ? formatJson(call.response.payload.content) : (call.text || '旧记录未保存结构化响应') }}</pre>
            </section>

            <section v-if="call.tools.length" class="border-t border-border-subtle px-4 py-3">
              <h3 class="mb-2 text-[11px] font-semibold text-text-secondary">工具调用 · {{ call.tools.length }}</h3>
              <div class="divide-y divide-border-subtle border-y border-border-subtle">
                <div v-for="tool in call.tools" :key="tool.id" class="py-2.5">
                  <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
                    <b class="font-mono text-[11px] text-text-primary">{{ tool.name }}</b>
                    <span class="font-mono text-[10px] text-text-tertiary">{{ tool.id }}</span>
                    <span v-if="tool.start?.payload.risk" class="text-[10px] text-text-tertiary">{{ tool.start.payload.risk }}</span>
                    <span v-if="tool.result?.payload.duration_ms ?? tool.metric?.payload.duration_ms" class="ml-auto font-mono text-[10px] tabular-nums text-text-tertiary">{{ formatDuration(tool.result?.payload.duration_ms ?? tool.metric?.payload.duration_ms) }}</span>
                    <span v-if="tool.result?.payload.is_error" class="text-[10px] font-medium text-red-600">ERROR</span>
                  </div>
                  <div class="mt-2 grid gap-2 xl:grid-cols-2">
                    <div>
                      <div class="mb-1 text-[10px] font-medium text-text-tertiary">输入</div>
                      <pre class="max-h-72 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-2.5 py-2 font-mono text-[10px] leading-relaxed text-text-secondary">{{ formatJson(tool.input) }}</pre>
                    </div>
                    <div>
                      <div class="mb-1 text-[10px] font-medium text-text-tertiary">输出</div>
                      <pre class="max-h-72 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-2.5 py-2 font-mono text-[10px] leading-relaxed" :class="tool.result?.payload.is_error ? 'text-red-600' : 'text-text-secondary'">{{ tool.result?.payload.content ?? '未记录结果' }}</pre>
                    </div>
                  </div>
                </div>
              </div>
            </section>

            <section v-if="call.retries.length || call.failures.length" class="border-t border-border-subtle px-4 py-3">
              <h3 class="mb-2 text-[11px] font-semibold text-text-secondary">重试与失败</h3>
              <div class="space-y-1.5 font-mono text-[10px]">
                <div v-for="retry in call.retries" :key="retry.record.id" class="text-amber-700">{{ retry.payload.stage }} · #{{ retry.payload.failed_attempt }} → #{{ retry.payload.next_attempt }} · 等待 {{ retry.payload.delay_ms }}ms · {{ retry.payload.error }}</div>
                <div v-for="failure in call.failures" :key="failure.record.id" class="text-red-600">{{ failure.payload.stage }} · #{{ failure.payload.attempt }} · {{ failure.payload.error }}</div>
              </div>
            </section>

            <details class="border-t border-border-subtle px-4 py-3">
              <summary class="cursor-pointer select-none text-[11px] font-medium text-text-tertiary">原始事件 · {{ call.events.length }}</summary>
              <pre class="mt-1.5 max-h-96 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-3 py-2 font-mono text-[10px] leading-relaxed text-text-secondary">{{ formatJson(call.events.map(event => ({ event_type: event.record.event_type, created_at: event.record.created_at, payload: event.payload }))) }}</pre>
            </details>
          </div>
        </section>
      </div>

      <section v-if="trace.controlEvents.length" class="border-t border-border px-4 py-3">
        <h3 class="mb-2 text-[11px] font-semibold text-text-secondary">运行控制事件 · {{ trace.controlEvents.length }}</h3>
        <div class="divide-y divide-border-subtle border-y border-border-subtle">
          <details v-for="event in trace.controlEvents" :key="event.record.id" class="py-2">
            <summary class="grid cursor-pointer select-none grid-cols-[74px_88px_minmax(0,1fr)] gap-2 text-[10px]">
              <span class="font-mono tabular-nums text-text-tertiary">{{ formatTime(event.record.created_at) }}</span>
              <span class="font-medium text-text-secondary">{{ controlLabel(event.record.event_type) }}</span>
              <span class="truncate text-text-tertiary">{{ controlPreview(event) }}</span>
            </summary>
            <pre class="mt-1.5 max-h-72 overflow-auto whitespace-pre-wrap break-words border border-border-subtle bg-surface px-3 py-2 font-mono text-[10px] leading-relaxed text-text-secondary">{{ formatJson(event.payload) }}</pre>
          </details>
        </div>
      </section>
    </template>
  </div>
</template>
