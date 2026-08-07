/**
 * App API 客户端。信封 {code,msg,data}：code!==0 抛错。
 */

const TOKEN_KEY = 'app-token'

export interface Envelope<T> {
  code: number
  msg: string
  data: T
}

export function getToken(): string {
  return localStorage.getItem(TOKEN_KEY) || ''
}

export function setToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY)
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly code: number,
  ) {
    super(message)
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (!headers.has('Content-Type') && init.body) {
    headers.set('Content-Type', 'application/json')
  }
  const token = getToken()
  if (token) headers.set('Authorization', `Bearer ${token}`)

  const res = await fetch(path, { ...init, headers })
  if (res.status === 401) {
    clearToken()
    throw new ApiError('登录已过期，请重新登录', 401)
  }
  const body = (await res.json()) as Envelope<T>
  if (body.code !== 0) throw new ApiError(body.msg || '请求失败', body.code)
  return body.data
}

const get = <T>(path: string) => request<T>(path)
const post = <T>(path: string, body?: unknown) =>
  request<T>(path, { method: 'POST', body: JSON.stringify(body ?? {}) })
const del = <T>(path: string) => request<T>(path, { method: 'DELETE' })

export interface LoginResult {
  token: string
  username: string
  can_admin: boolean
  can_client: boolean
}

export interface ClientInfo {
  id: string
  hostname: string | null
  work_dir: string | null
  accept_remote: boolean
  enabled: boolean
  last_active_at: string | null
  last_seen_at: string | null
  online: boolean
}

export interface TermInfo {
  id: string
  cols?: number
  rows?: number
  cwd?: string
  shell?: string
  foreground_cmd?: string
  alive?: boolean
  title?: string
}

export interface RtcIceConfig {
  iceServers: RTCIceServer[]
  ttl?: number
}

export const api = {
  login(username: string, password: string) {
    return request<LoginResult>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password, scope: 'client' }),
    }).then((data) => {
      setToken(data.token)
      return data
    })
  },

  whoami() {
    return get<{
      auth: string
      user_id: string
      username: string
      can_admin?: boolean
    }>('/api/auth/whoami')
  },

  clients() {
    return get<{ clients: ClientInfo[] }>('/api/app/clients')
  },

  setEnabled(clientId: string, enabled: boolean) {
    return post<{ id: string; enabled: boolean }>(
      `/api/app/clients/${clientId}/enabled`,
      { enabled },
    )
  },

  deleteClient(clientId: string) {
    return del<{ id: string; deleted: boolean }>(
      `/api/app/clients/${clientId}`,
    )
  },

  listTerminals(clientId: string) {
    return get<TermInfo[] | { terminals?: TermInfo[] }>(
      `/api/app/clients/${clientId}/terminals`,
    ).then((data) => {
      if (Array.isArray(data)) return data
      if (data && Array.isArray(data.terminals)) return data.terminals
      return []
    })
  },

  createTerminal(clientId: string, opts?: { cwd?: string; cols?: number; rows?: number }) {
    return post<{ terminal: TermInfo }>(
      `/api/app/clients/${clientId}/terminals`,
      opts ?? {},
    )
  },

  closeTerminal(clientId: string, termId: string) {
    return del<{ closed: boolean }>(
      `/api/app/clients/${clientId}/terminals/${termId}`,
    )
  },

  terminalOutputRaw(clientId: string, termId: string) {
    return get<{ output: string }>(
      `/api/app/clients/${clientId}/terminals/${termId}/output?raw=true`,
    )
  },

  terminalInput(clientId: string, termId: string, data: string) {
    return post<{ sent: boolean }>(
      `/api/app/clients/${clientId}/terminals/${termId}/input`,
      { data },
    )
  },

  terminalResize(clientId: string, termId: string, cols: number, rows: number) {
    return post<{ resized: boolean }>(
      `/api/app/clients/${clientId}/terminals/${termId}/resize`,
      { cols, rows },
    )
  },

  rtcIce() {
    return get<RtcIceConfig>('/api/app/rtc/ice')
  },

  rtcOffer(clientId: string, sdp: string) {
    return post<RTCSessionDescriptionInit>(
      `/api/app/clients/${clientId}/rtc/offer`,
      { sdp },
    )
  },
}
