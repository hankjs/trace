/**
 * team 看板 API 客户端。
 *
 * TOKEN_KEY 用 hank_team_token，与 admin 的 hank_admin_token 分开——
 * 两个前端共用 localStorage key 会互相踢登录态。
 */
const TOKEN_KEY = 'hank_team_token'

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
    // hash 路由：跳转目标是 #/login（与 admin 的 /admin/login 不同）
    window.location.hash = '#/login'
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

export interface TeamTask {
  id: string
  task_no: string
  session_id: string
  user_id: string
  source: string
  issue_key: string | null
  title: string
  goal: string | null
  analysis: string | null
  status: string
  current_role: string | null
  dev_rounds: number
  backend: string
  exec_client_id: string | null
  agent_kind: string
  account_id: string | null
  chat_id: string | null
  topic_id: string | null
  card_message_id: string | null
  origin_message_id: string | null
  result: string | null
  error: string | null
  created_at: string
  updated_at: string
  finished_at: string | null
}

export interface TeamTaskRun {
  id: string
  task_id: string
  role: string
  round: number
  thread_id: string | null
  status: string
  verdict: string | null
  handoff: string | null
  summary: string | null
  dirty_files: number | null
  error: string | null
  started_at: string
  finished_at: string | null
}

export interface TeamTaskEvent {
  id: number
  task_id: string
  kind: string
  role: string | null
  round: number | null
  operator: string | null
  detail: string | null
  created_at: string
}

export interface TeamTaskDetail {
  task: TeamTask
  runs: TeamTaskRun[]
  events: TeamTaskEvent[]
}

export const api = {
  login(username: string, password: string) {
    // 看板数据接口在 admin_api 组内，要 admin JWT
    return fetch('/api/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password, scope: 'admin' }),
    })
  },

  listTasks(params?: {
    status?: string
    user_id?: string
    issue_key?: string
    page?: number
    per_page?: number
  }) {
    const q = new URLSearchParams()
    if (params?.status) q.set('status', params.status)
    if (params?.user_id) q.set('user_id', params.user_id)
    if (params?.issue_key) q.set('issue_key', params.issue_key)
    if (params?.page) q.set('page', String(params.page))
    if (params?.per_page) q.set('per_page', String(params.per_page))
    const qs = q.toString()
    return request<PaginatedResponse<TeamTask>>(
      `/api/team/tasks${qs ? `?${qs}` : ''}`,
    )
  },

  getTask(taskNo: string) {
    return request<TeamTaskDetail>(`/api/team/tasks/${encodeURIComponent(taskNo)}`)
  },

  cancelTask(taskNo: string) {
    return request<{ task_no: string; status: string; message?: string }>(
      `/api/team/tasks/${encodeURIComponent(taskNo)}/cancel`,
      { method: 'POST' },
    )
  },

  retryTask(taskNo: string) {
    return request<{
      task_no: string
      status?: string
      current_role?: string | null
      message?: string
    }>(`/api/team/tasks/${encodeURIComponent(taskNo)}/retry`, { method: 'POST' })
  },
}
