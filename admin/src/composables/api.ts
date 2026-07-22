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

export interface AgentEventRecord {
  id: string
  session_id: string
  event_type: string
  payload: string
  seq: number
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

  // ---- 终端代理 ----

  listClients() {
    return request<ClientAgentInfo[]>('/api/admin/clients')
  },

  listClientTerminals(clientId: string) {
    return request<TermInfo[]>(`/api/admin/clients/${clientId}/terminals`)
  },

  terminalOutput(clientId: string, termId: string, lines = 200) {
    return request<{ output: string }>(
      `/api/admin/clients/${clientId}/terminals/${termId}/output?lines=${lines}`
    )
  },

  terminalInput(clientId: string, termId: string, data: string) {
    return request<{ sent: boolean }>(
      `/api/admin/clients/${clientId}/terminals/${termId}/input`,
      { method: 'POST', body: JSON.stringify({ data }) }
    )
  },

  deleteWeixinBinding(id: string) {
    return request<void>(`/api/admin/weixin/bindings/${id}`, { method: 'DELETE' })
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

export interface ClientAgentInfo {
  id: string
  user_id: string
  hostname: string | null
  work_dir: string | null
  accept_remote: boolean
  online: boolean
}

export interface TermInfo {
  id: string
  shell: string
  cwd: string
  foreground_cmd: string
  alive: boolean
  created_at: string
}
