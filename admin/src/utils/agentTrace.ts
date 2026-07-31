import type { AgentEventRecord } from '../composables/api'

export interface ParsedAgentEvent {
  record: AgentEventRecord
  payload: Record<string, any>
}

export interface TraceToolCall {
  id: string
  name: string
  input: unknown
  start?: ParsedAgentEvent
  result?: ParsedAgentEvent
  metric?: ParsedAgentEvent
}

export interface TraceCall {
  key: string
  callId: string | null
  runId: string | null
  turnId: string | null
  phase: string
  model: string
  provider: string
  request?: ParsedAgentEvent
  response?: ParsedAgentEvent
  metrics?: ParsedAgentEvent
  retries: ParsedAgentEvent[]
  failures: ParsedAgentEvent[]
  events: ParsedAgentEvent[]
  tools: TraceToolCall[]
  text: string
  startedAt: string
  endedAt: string
}

export interface AgentTrace {
  calls: TraceCall[]
  controlEvents: ParsedAgentEvent[]
  toolCount: number
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
  elapsedMs: number
}

const CONTROL_EVENT_TYPES = new Set([
  'run_started',
  'run_completed',
  'run_failed',
  'run_cancelled',
  'turn_started',
  'turn_completed',
  'provider_fallback',
  'permission_requested',
  'permission_denied',
  'compression_triggered',
  'token_warning',
  'loop_detected',
  'file_changed',
  'context_assembled',
  'worker_spawned',
  'worker_completed',
  'verification_started',
  'verification_completed',
  'verification',
  'error',
  'ask_user',
])

function parsePayload(payload: string): Record<string, any> {
  try {
    const value = JSON.parse(payload)
    return value && typeof value === 'object' ? value : { value }
  } catch {
    return { raw: payload }
  }
}

function createCall(event: ParsedAgentEvent, key: string): TraceCall {
  const payload = event.payload
  return {
    key,
    callId: payload.call_id || null,
    runId: payload.run_id || null,
    turnId: payload.turn_id || null,
    phase: payload.phase || 'unknown',
    model: payload.model || '',
    provider: payload.provider || '',
    retries: [],
    failures: [],
    events: [],
    tools: [],
    text: '',
    startedAt: event.record.created_at,
    endedAt: event.record.created_at,
  }
}

function findTool(call: TraceCall, id: string, name = 'unknown'): TraceToolCall {
  let tool = call.tools.find(item => item.id === id)
  if (!tool) {
    tool = { id, name, input: null }
    call.tools.push(tool)
  } else if (tool.name === 'unknown' && name) {
    tool.name = name
  }
  return tool
}

export function buildAgentTrace(records: AgentEventRecord[]): AgentTrace {
  const parsed = records.map(record => ({ record, payload: parsePayload(record.payload) }))
  const calls: TraceCall[] = []
  const callsById = new Map<string, TraceCall>()
  const callsByTurn = new Map<string, TraceCall>()
  let currentCall: TraceCall | null = null

  const ensureCall = (event: ParsedAgentEvent): TraceCall => {
    const callId = event.payload.call_id as string | undefined
    const turnId = event.payload.turn_id as string | undefined
    let call = callId ? callsById.get(callId) : undefined
    if (!call && turnId) call = callsByTurn.get(turnId)
    if (!call && !callId && currentCall) call = currentCall
    if (!call) {
      const key = callId || `legacy:${event.record.id}`
      call = createCall(event, key)
      calls.push(call)
      if (callId) callsById.set(callId, call)
      if (turnId) callsByTurn.set(turnId, call)
    }
    if (callId && !call.callId) {
      call.callId = callId
      callsById.set(callId, call)
    }
    if (turnId && !call.turnId) {
      call.turnId = turnId
      callsByTurn.set(turnId, call)
    }
    call.runId ||= event.payload.run_id || null
    call.phase = event.payload.phase || call.phase
    call.model = event.payload.model || call.model
    call.provider = event.payload.provider || call.provider
    call.endedAt = event.record.created_at
    if (!call.events.some(item => item.record.id === event.record.id)) call.events.push(event)
    currentCall = call
    return call
  }

  for (const event of parsed) {
    const type = event.record.event_type
    if (type === 'llm_request') {
      let call: TraceCall
      if (!event.payload.call_id && !event.payload.turn_id && currentCall?.request) {
        call = createCall(event, `legacy:${event.record.id}`)
        calls.push(call)
        currentCall = call
      } else {
        call = ensureCall(event)
      }
      call.request = event
      call.startedAt = event.record.created_at
      continue
    }
    if (type === 'llm_response') {
      const call = ensureCall(event)
      call.response = event
      continue
    }
    if (type === 'llm_retry') {
      ensureCall(event).retries.push(event)
      continue
    }
    if (type === 'llm_failed') {
      ensureCall(event).failures.push(event)
      continue
    }
    if (type === 'text_delta' || type === 'thinking') {
      const call = ensureCall(event)
      call.text += String(event.payload.text || '')
      continue
    }
    if (type === 'tool_start') {
      const call = ensureCall(event)
      const tool = findTool(call, String(event.payload.id || event.record.id), event.payload.name)
      tool.name = event.payload.name || tool.name
      tool.input = event.payload.input ?? null
      tool.start = event
      continue
    }
    if (type === 'tool_result') {
      let call = event.payload.call_id ? callsById.get(event.payload.call_id) : undefined
      if (!call && event.payload.turn_id) call = callsByTurn.get(event.payload.turn_id)
      if (!call) {
        const id = String(event.payload.id || '')
        call = [...calls].reverse().find(item => item.tools.some(tool => tool.id === id))
      }
      call ||= ensureCall(event)
      const tool = findTool(call, String(event.payload.id || event.record.id), event.payload.name)
      tool.result = event
      call.endedAt = event.record.created_at
      if (!call.events.some(item => item.record.id === event.record.id)) call.events.push(event)
      currentCall = call
      continue
    }
    if (type === 'tool_metrics' && currentCall) {
      const tool = [...currentCall.tools]
        .reverse()
        .find(item => item.name === event.payload.tool_name && !item.metric)
      if (tool) tool.metric = event
      currentCall.events.push(event)
      currentCall.endedAt = event.record.created_at
      continue
    }
    if (type === 'metrics') {
      ensureCall(event).metrics = event
      continue
    }
    if (event.payload.call_id || event.payload.turn_id) ensureCall(event)
  }

  const controlEvents = parsed.filter(event => CONTROL_EVENT_TYPES.has(event.record.event_type))
  const uniqueTools = new Set<string>()
  let inputTokens = 0
  let outputTokens = 0
  let cacheReadTokens = 0
  let cacheWriteTokens = 0

  for (const call of calls) {
    for (const tool of call.tools) uniqueTools.add(`${call.key}:${tool.id}`)
    const usage = call.response?.payload || call.metrics?.payload
    if (usage) {
      inputTokens += Number(usage.input_tokens || 0)
      outputTokens += Number(usage.output_tokens || 0)
      cacheReadTokens += Number(usage.cache_read_tokens || 0)
      cacheWriteTokens += Number(usage.cache_write_tokens || 0)
    }
  }

  const first = records[0]?.created_at
  const last = records[records.length - 1]?.created_at
  const elapsedMs = first && last
    ? Math.max(0, new Date(last).getTime() - new Date(first).getTime())
    : 0

  return {
    calls,
    controlEvents,
    toolCount: uniqueTools.size,
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    elapsedMs,
  }
}
