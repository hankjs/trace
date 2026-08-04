const TOKEN_KEY = 'hank_admin_token'

function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
}

export function hasToken(): boolean {
  return !!localStorage.getItem(TOKEN_KEY)
}

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const token = getToken()
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  }
  const res = await fetch(url, { ...options, headers })
  if (res.status === 401) {
    clearToken()
    window.location.href = '/admin/login'
    throw new Error('Unauthorized')
  }
  const json = await res.json()
  if (json.code !== 0) {
    throw new Error(json.msg || `Request failed: ${res.status}`)
  }
  return json.data as T
}

export interface PaginatedResponse<T> {
  data: T[]
  total: number
  page: number
  per_page: number
}

export interface Session {
  id: string
  user_id: string | null
  title: string
  provider: string
  model: string
  username: string | null
  created_at: string
  updated_at: string
}

export interface AgentMetric {
  id: string
  session_id: string
  input_tokens: number
  output_tokens: number
  latency_ms: number
  model: string
  provider: string
  created_at: string
}

export interface ToolExecution {
  id: string
  session_id: string
  tool_name: string
  duration_ms: number
  is_error: boolean
  created_at: string
}

export interface MetricsOverview {
  total_input_tokens: number
  total_output_tokens: number
  avg_latency_ms: number
  total_llm_calls: number
  tool_error_count: number
  tool_total_count: number
}

export interface PromptTemplate {
  id: string
  name: string
  content: string
  category: string
  version: number
  created_at: string
}

export interface DbMessage {
  id: string
  session_id: string
  role: string
  content: string
  parent_id: string | null
  created_at: string
}

export interface User {
  id: string
  username: string
  can_login_admin: boolean
  can_login_client: boolean
  created_at: string
}

export interface Provider {
  id: string
  name: string
  provider_type: string
  api_key: string
  base_url: string
  default_model: string
  models: string
  priority: number
  enabled: boolean
  created_at: string
}

export interface WeixinLoginStart {
  login_id: string
  qrcode_url: string
}

export interface WeixinLoginStatus {
  status: 'waiting' | 'scanned' | 'confirmed' | 'expired' | 'error'
  account?: WeixinAccount | null
  msg?: string
}

export interface WeixinAccount {
  id: string
  ilink_bot_id: string
  base_url: string
  bot_user_id: string
  enabled: boolean
  created_at: string
}

export interface WeixinBinding {
  id: string
  username: string
  ilink_user_id: string
  created_at: string
}

export interface FeishuAccount {
  id: string
  name: string
  app_id: string
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface FeishuBinding {
  id: string
  account_id: string
  username: string
  open_id: string
  created_at: string
}

export interface FeishuBindCode {
  code: string
  expires_at: number
}

export interface ChannelConversation {
  channel: string
  account_id: string
  account_name: string
  conversation_id: string
  topic_id: string
  peer_id: string | null
  user_id: string | null
  username: string | null
  session_id: string | null
  message_count: number
  first_message_at: string
  last_message_at: string
  last_direction: 'inbound' | 'outbound'
  last_message_type: string
  last_content: string
  /** 实际执行后端：codex / claude / native provider 名；旧会话为空 */
  agent_provider: string | null
  /** 实际使用的模型名；旧会话为空 */
  agent_model: string | null
}

export interface ChannelMessage {
  id: string
  channel: string
  account_id: string
  account_name: string
  conversation_id: string
  topic_id: string
  external_message_id: string
  reply_to_external_id: string | null
  direction: 'inbound' | 'outbound'
  message_type: string
  content: string
  peer_id: string | null
  user_id: string | null
  username: string | null
  session_id: string | null
  created_at: string
}

export interface JobRun {
  id: number
  job_id: string
  trigger: 'system' | 'manual'
  status: 'running' | 'finished' | 'failed'
  operator: string | null
  started_at: string
  finished_at: string | null
  result: string | null
  error: string | null
}

export interface AdminJob {
  id: string
  name: string
  description: string
  schedule: string
  enabled: boolean
  next_run_time: string | null
  last_system_run: JobRun | null
  manual_run: JobRun | null
}

/** 与 hank_db::AgentInteraction 对齐（snake_case） */
export interface AgentInteraction {
  id: string
  session_id: string
  user_id: string
  channel: string
  account_id: string | null
  chat_id: string | null
  topic_id: string | null
  kind: string
  title: string
  goal: string | null
  analysis: string | null
  /** 按钮文案数组 JSON 文本，如 `["确认","否"]` */
  options: string
  status: string
  answer: string | null
  answered_by: string | null
  answered_at: string | null
  resume_ref: string | null
  card_message_id: string | null
  expires_at: string | null
  result: string | null
  error: string | null
  created_at: string
  updated_at: string
}

export interface AgentEventRecord {
  id: string
  session_id: string
  event_type: string
  payload: string
  seq?: number
  source?: string
  agent_type?: string
  created_at: string
}

export const api = {
  login(username: string, password: string) {
    return fetch('/api/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password, scope: 'admin' }),
    })
  },

  sessions(page = 1, perPage = 20, search = '', sessionType = '') {
    const params = new URLSearchParams({ page: String(page), per_page: String(perPage) })
    if (search) params.set('search', search)
    if (sessionType) params.set('session_type', sessionType)
    return request<PaginatedResponse<Session>>(`/api/admin/sessions?${params}`)
  },

  sessionReplay(id: string) {
    return request<{ messages: DbMessage[]; metrics: AgentMetric[]; tool_executions: ToolExecution[] }>(
      `/api/admin/sessions/${id}/replay`
    )
  },

  sessionEvents(id: string) {
    return request<AgentEventRecord[]>(`/api/admin/sessions/${id}/events`)
  },

  metricsOverview() {
    return request<MetricsOverview>('/api/admin/metrics/overview')
  },

  metricsBySession(id: string) {
    return request<{ metrics: AgentMetric[]; tool_executions: ToolExecution[] }>(
      `/api/admin/metrics/by-session/${id}`
    )
  },

  listPromptTemplates(category?: string) {
    const query = category ? `?category=${category}` : ''
    return request<PromptTemplate[]>(`/api/admin/prompt-templates${query}`)
  },

  createPromptTemplate(name: string, content: string, category?: string) {
    return request<{ id: string }>('/api/admin/prompt-templates', {
      method: 'POST',
      body: JSON.stringify({ name, content, category: category || 'prompt' }),
    })
  },

  deletePromptTemplate(id: string) {
    return request<void>(`/api/admin/prompt-templates/${id}`, { method: 'DELETE' })
  },

  replay(sessionId: string, opts: { prompt_template_id?: string; system_prompt?: string }) {
    return fetch('/api/admin/replay', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {}),
      },
      body: JSON.stringify({ session_id: sessionId, ...opts }),
    })
  },

  // User management
  listUsers() {
    return request<User[]>('/api/admin/users')
  },

  createUser(username: string, password: string, can_login_admin: boolean, can_login_client: boolean) {
    return request<{ id: string; username: string }>('/api/admin/users', {
      method: 'POST',
      body: JSON.stringify({ username, password, can_login_admin, can_login_client }),
    })
  },

  updateUser(id: string, data: { can_login_admin?: boolean; can_login_client?: boolean; password?: string }) {
    return request<{ status: string }>(`/api/admin/users/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    })
  },

  deleteUser(id: string) {
    return request<void>(`/api/admin/users/${id}`, { method: 'DELETE' })
  },

  // Provider management
  listProviders() {
    return request<Provider[]>('/api/admin/providers')
  },

  createProvider(data: { name: string; provider_type: string; api_key: string; base_url?: string; default_model?: string; models?: Record<string, string>; priority?: number; enabled?: boolean }) {
    return request<Provider>('/api/admin/providers', {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  updateProvider(id: string, data: { name: string; provider_type: string; api_key: string; base_url?: string; default_model?: string; models?: Record<string, string>; priority?: number; enabled?: boolean }) {
    return request<{ status: string }>(`/api/admin/providers/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    })
  },

  deleteProvider(id: string) {
    return request<void>(`/api/admin/providers/${id}`, { method: 'DELETE' })
  },

  // Image provider management
  listImageProviders() {
    return request<Provider[]>('/api/admin/image-providers')
  },

  createImageProvider(data: { name: string; provider_type: string; api_key: string; base_url?: string; default_model?: string; models?: Record<string, string>; priority?: number; enabled?: boolean }) {
    return request<Provider>('/api/admin/image-providers', { method: 'POST', body: JSON.stringify(data) })
  },

  updateImageProvider(id: string, data: { name: string; provider_type: string; api_key: string; base_url?: string; default_model?: string; models?: Record<string, string>; priority?: number; enabled?: boolean }) {
    return request<{ status: string }>(`/api/admin/image-providers/${id}`, { method: 'PUT', body: JSON.stringify(data) })
  },

  deleteImageProvider(id: string) {
    return request<void>(`/api/admin/image-providers/${id}`, { method: 'DELETE' })
  },

  // Weixin bot management
  weixinLoginStart() {
    return request<WeixinLoginStart>('/api/admin/weixin/login', { method: 'POST' })
  },

  weixinLoginStatus(loginId: string) {
    return request<WeixinLoginStatus>(`/api/admin/weixin/login/${loginId}`)
  },

  listWeixinAccounts() {
    return request<WeixinAccount[]>('/api/admin/weixin/accounts')
  },

  updateWeixinAccount(id: string, data: { enabled: boolean }) {
    return request<{ status: string }>(`/api/admin/weixin/accounts/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    })
  },

  deleteWeixinAccount(id: string) {
    return request<void>(`/api/admin/weixin/accounts/${id}`, { method: 'DELETE' })
  },

  listWeixinBindings() {
    return request<WeixinBinding[]>('/api/admin/weixin/bindings')
  },

  weixinSend(bindingId: string, text: string) {
    return request<void>('/api/admin/weixin/send', {
      method: 'POST',
      body: JSON.stringify({ binding_id: bindingId, text }),
    })
  },

  deleteWeixinBinding(id: string) {
    return request<void>(`/api/admin/weixin/bindings/${id}`, { method: 'DELETE' })
  },

  // Feishu bot management
  listFeishuAccounts() {
    return request<FeishuAccount[]>('/api/admin/feishu/accounts')
  },

  createFeishuAccount(data: { name?: string; app_id: string; app_secret: string }) {
    return request<{ id: string }>('/api/admin/feishu/accounts', {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  updateFeishuAccount(id: string, data: { enabled?: boolean; name?: string; app_secret?: string }) {
    return request<void>(`/api/admin/feishu/accounts/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    })
  },

  deleteFeishuAccount(id: string) {
    return request<void>(`/api/admin/feishu/accounts/${id}`, { method: 'DELETE' })
  },

  listFeishuBindings() {
    return request<FeishuBinding[]>('/api/admin/feishu/bindings')
  },

  createFeishuBindCode(userId: string) {
    return request<FeishuBindCode>('/api/admin/feishu/bind-code', {
      method: 'POST',
      body: JSON.stringify({ user_id: userId }),
    })
  },

  deleteFeishuBinding(id: string) {
    return request<void>(`/api/admin/feishu/bindings/${id}`, { method: 'DELETE' })
  },

  feishuSend(bindingId: string, text: string) {
    return request<void>('/api/admin/feishu/send', {
      method: 'POST',
      body: JSON.stringify({ binding_id: bindingId, text }),
    })
  },

  chatRecordConversations(page = 1, perPage = 30, search = '', channel = 'feishu') {
    const params = new URLSearchParams({
      channel,
      page: String(page),
      per_page: String(perPage),
    })
    if (search) params.set('search', search)
    return request<PaginatedResponse<ChannelConversation>>(
      `/api/admin/chat-records/conversations?${params}`,
    )
  },

  chatRecordMessages(
    conversation: Pick<ChannelConversation, 'account_id' | 'conversation_id' | 'topic_id'>,
    page = 1,
    perPage = 100,
    channel = 'feishu',
  ) {
    const params = new URLSearchParams({
      channel,
      account_id: conversation.account_id,
      conversation_id: conversation.conversation_id,
      topic_id: conversation.topic_id,
      page: String(page),
      per_page: String(perPage),
    })
    return request<PaginatedResponse<ChannelMessage>>(
      `/api/admin/chat-records/messages?${params}`,
    )
  },

  // Scheduler job management
  listJobs() {
    return request<{ scheduler_running: boolean; jobs: AdminJob[] }>('/api/admin/jobs')
  },

  updateJob(id: string, data: { enabled: boolean }) {
    return request<void>(`/api/admin/jobs/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    })
  },

  jobRuns(id: string, limit = 20) {
    return request<JobRun[]>(`/api/admin/jobs/${id}/runs?limit=${limit}`)
  },

  runJob(id: string) {
    return request<{ status: string }>(`/api/admin/jobs/${id}/run`, { method: 'POST' })
  },

  // 交互单管理
  listInteractions(params: {
    status?: string
    kind?: string
    channel?: string
    page?: number
    per_page?: number
  } = {}) {
    const q = new URLSearchParams()
    if (params.status) q.set('status', params.status)
    if (params.kind) q.set('kind', params.kind)
    if (params.channel) q.set('channel', params.channel)
    q.set('page', String(params.page ?? 1))
    q.set('per_page', String(params.per_page ?? 30))
    return request<PaginatedResponse<AgentInteraction>>(
      `/api/admin/interactions?${q}`,
    )
  },

  getInteraction(id: string) {
    return request<AgentInteraction>(`/api/admin/interactions/${id}`)
  },

  answerInteraction(id: string, answer: string) {
    return request<AgentInteraction>(`/api/admin/interactions/${id}/answer`, {
      method: 'POST',
      body: JSON.stringify({ answer }),
    })
  },

  cancelInteraction(id: string) {
    return request<AgentInteraction>(`/api/admin/interactions/${id}/cancel`, {
      method: 'POST',
    })
  },

  chatGenerate(prompt: string, context?: string) {
    return fetch('/api/admin/chat/generate', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {}),
      },
      body: JSON.stringify({ prompt, context }),
    })
  },
}

