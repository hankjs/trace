/** 后端 /api 封装:统一 fetch、错误处理(HTTP 错误取 FastAPI 的 detail)。 */

const TOKEN_KEY = 'quant_token'
const USERNAME_KEY = 'quant_username'

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setAuth(token: string, username: string) {
  localStorage.setItem(TOKEN_KEY, token)
  localStorage.setItem(USERNAME_KEY, username)
}

export function clearAuth() {
  localStorage.removeItem(TOKEN_KEY)
  localStorage.removeItem(USERNAME_KEY)
}

export function currentUsername(): string {
  return localStorage.getItem(USERNAME_KEY) ?? ''
}

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  const token = getToken()
  if (token) headers.Authorization = `Bearer ${token}`
  const res = await fetch(url, { headers, ...options })
  if (res.status === 401) {
    // 未登录或 token 过期:清掉凭证回登录页(登录接口本身除外,由页面提示错误)
    if (!url.startsWith('/api/auth/login')) {
      clearAuth()
      location.href = '/login'
    }
  }
  if (!res.ok) {
    let msg = `请求失败: ${res.status}`
    try {
      const json = await res.json()
      if (typeof json.detail === 'string') msg = json.detail
      else if (Array.isArray(json.detail) && json.detail[0]?.msg) msg = json.detail[0].msg
    } catch {
      /* 非 JSON 错误体 */
    }
    throw new Error(msg)
  }
  return res.json() as Promise<T>
}

export interface KlineBar {
  date: string
  open: number
  high: number
  low: number
  close: number
  raw_close: number
  volume: number
  amount: number
}

export interface SnapshotItem {
  code: string
  name: string
  source: 'snapshot' | 'close' | null
  ts: string | null
  price: number | null
  pct_chg: number | null
}

export interface WatchItem {
  code: string
  name: string
  industry: string
}

export interface SignalItem {
  id: number
  code: string
  date: string
  strategy: string
  side: 'buy' | 'sell' | 'watch'
  price: number
  reason: Record<string, unknown>
}

export interface Trade {
  id: number
  code: string
  trade_date: string
  side: 'buy' | 'sell'
  price: number
  qty: number
  fee: number
  note: string
}

export interface Position {
  code: string
  qty: number
  avg_cost: number
  last_price: number | null
  price_source: string | null
  market_value: number | null
  unrealized_pnl: number | null
  realized_pnl: number
}

export interface PortfolioSummary {
  positions: Position[]
  total_market_value: number
  total_unrealized_pnl: number
  total_realized_pnl: number
}

export interface BacktestMetrics {
  total_return: number
  annual_return: number
  max_drawdown: number
  win_rate: number
  trade_count: number
  round_trips: number
  per_code?: Record<string, unknown>
}

export interface BacktestResult {
  run_id: number
  strategy?: string
  codes?: string[]
  start?: string
  end?: string
  metrics: BacktestMetrics
  equity: { date: string; equity: number }[]
}

/** 选股池因子(字段以后端实际响应为准,全部可选) */
export interface PickFactors {
  mom20?: number
  mom60?: number
  rsi14?: number
  atr_pct?: number
  vol_ratio5?: number
  ma20_slope?: number
  amount_avg20?: number
  [key: string]: number | undefined
}

export interface PickItem {
  rank: number
  code: string
  name: string
  score: number
  factors: PickFactors
  /** 相对前一交易日的变动标记:'new' 新进等(字段名/取值做防御性适配) */
  change?: string | null
  is_new?: boolean
  dropped?: boolean
}

export interface PicksResult {
  date: string
  prev_date?: string | null
  items: PickItem[]
  /** 调出名单:可能是对象或纯代码字符串 */
  dropped?: (PickItem | string)[]
}

export interface ScreenerItem {
  code: string
  name: string
  close: number
  /** 涨跌幅:实际响应为 pct_chg,契约曾用 chg_pct,两者都兼容 */
  pct_chg?: number
  chg_pct?: number
  high_dist?: number
  mom20?: number
  mom60?: number
  rsi14?: number
  vol_ratio5?: number
  amount_avg20?: number
  [key: string]: unknown
}

export interface ScreenerResult {
  date?: string
  total?: number
  count?: number
  items: ScreenerItem[]
}

export interface LeaderboardItem {
  strategy: string
  scope: string
  start: string
  end: string
  metrics: Record<string, number>
  run_at: string
}

/** 参数扫描指标:聚合口径可能带 _mean/_median 后缀,读取时用 metricOf 兜底 */
export interface SweepMetrics {
  [key: string]: number | undefined
}

export interface SweepResultItem {
  params: Record<string, number>
  metrics: SweepMetrics
  per_code?: Record<string, Record<string, number>>
}

export interface SweepResult {
  strategy: string
  codes: string[]
  start: string
  end: string
  results: SweepResultItem[]
}

export const api = {
  login(username: string, password: string) {
    return request<{ token: string; username: string }>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    })
  },

  kline(code: string, start?: string, end?: string) {
    const params = new URLSearchParams({ code })
    if (start) params.set('start', start)
    if (end) params.set('end', end)
    return request<{ code: string; count: number; bars: KlineBar[] }>(
      `/api/market/kline?${params}`
    )
  },

  snapshot() {
    return request<{ count: number; items: SnapshotItem[] }>('/api/market/snapshot')
  },

  watchlist() {
    return request<{ count: number; items: WatchItem[] }>('/api/watchlist')
  },

  addWatch(code: string, name = '', industry = '') {
    return request<WatchItem>('/api/watchlist', {
      method: 'POST',
      body: JSON.stringify({ code, name, industry }),
    })
  },

  removeWatch(code: string) {
    return request<{ code: string; is_watch: boolean }>(`/api/watchlist/${code}`, {
      method: 'DELETE',
    })
  },

  signals(filters: { date?: string; code?: string; strategy?: string; side?: string; limit?: number } = {}) {
    const params = new URLSearchParams()
    if (filters.date) params.set('date', filters.date)
    if (filters.code) params.set('code', filters.code)
    if (filters.strategy) params.set('strategy', filters.strategy)
    if (filters.side) params.set('side', filters.side)
    if (filters.limit) params.set('limit', String(filters.limit))
    return request<{ count: number; items: SignalItem[] }>(`/api/signals?${params}`)
  },

  trades(code?: string) {
    const params = code ? `?code=${encodeURIComponent(code)}` : ''
    return request<{ count: number; items: Trade[] }>(`/api/portfolio/trades${params}`)
  },

  addTrade(body: { code: string; trade_date: string; side: string; price: number; qty: number; fee?: number; note?: string }) {
    return request<Trade>('/api/portfolio/trades', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  deleteTrade(id: number) {
    return request<{ deleted: number }>(`/api/portfolio/trades/${id}`, { method: 'DELETE' })
  },

  positions() {
    return request<PortfolioSummary>('/api/portfolio/positions')
  },

  strategies() {
    return request<{ strategies: string[] }>('/api/backtest/strategies')
  },

  runBacktest(body: { strategy: string; codes: string[]; start: string; end: string; params?: Record<string, unknown>; costs?: Record<string, unknown> }) {
    return request<BacktestResult>('/api/backtest', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  getBacktest(runId: number) {
    return request<BacktestResult>(`/api/backtest/${runId}`)
  },

  picks(date?: string) {
    const params = new URLSearchParams()
    if (date) params.set('date', date)
    const qs = params.toString()
    return request<PicksResult>(`/api/selection/picks${qs ? `?${qs}` : ''}`)
  },

  screener(filters: {
    pct_chg_min?: number
    pct_chg_max?: number
    vol_ratio_min?: number
    ma_bull?: boolean
    high_window?: number
    high_dist_max?: number
    amount_min?: number
  } = {}) {
    const params = new URLSearchParams()
    if (filters.pct_chg_min !== undefined) params.set('pct_chg_min', String(filters.pct_chg_min))
    if (filters.pct_chg_max !== undefined) params.set('pct_chg_max', String(filters.pct_chg_max))
    if (filters.vol_ratio_min !== undefined) params.set('vol_ratio_min', String(filters.vol_ratio_min))
    if (filters.ma_bull) params.set('ma_bull', 'true')
    if (filters.high_window !== undefined) params.set('high_window', String(filters.high_window))
    if (filters.high_dist_max !== undefined) params.set('high_dist_max', String(filters.high_dist_max))
    if (filters.amount_min !== undefined) params.set('amount_min', String(filters.amount_min))
    return request<ScreenerResult>(`/api/selection/screener?${params}`)
  },

  leaderboard() {
    return request<{ run_at?: string; items: LeaderboardItem[] }>('/api/backtest/leaderboard')
  },

  sweepBacktest(body: {
    strategy: string
    codes: string[]
    start: string
    end: string
    param_grid: Record<string, number[]>
  }) {
    return request<SweepResult>('/api/backtest/sweep', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  backfill(code: string, start?: string, end?: string) {
    const params = new URLSearchParams({ code })
    if (start) params.set('start', start)
    if (end) params.set('end', end)
    return request<{ code: string; start: string; end: string; bars: number }>(
      `/api/admin/backfill?${params}`,
      { method: 'POST' }
    )
  },
}
