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

export function normalizeStockCode(value: string): string | null {
  const normalized = value.trim().toLowerCase()
  if (/^(?:sh|sz|bj)\.\d{6}$/.test(normalized)) return normalized
  if (!/^\d{6}$/.test(normalized)) return null
  if (/^(?:4|8|92)/.test(normalized)) return `bj.${normalized}`
  if (/^(?:6|9)/.test(normalized)) return `sh.${normalized}`
  return `sz.${normalized}`
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
    const error = new Error(msg) as Error & { status?: number }
    error.status = res.status
    throw error
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

export interface StockSearchItem extends WatchItem {
  is_watch?: boolean
}

export interface SignalItem {
  id: number
  code: string
  name?: string
  industry?: string
  date: string
  strategy: string
  strategy_name?: string
  side: 'buy' | 'sell' | 'watch'
  side_name?: string
  price: number
  reason: Record<string, unknown>
  reason_text?: string
}

export interface Trade {
  id: number
  code: string
  name?: string
  trade_date: string
  side: 'buy' | 'sell'
  price: number
  qty: number
  fee: number
  note: string
}

export interface Position {
  code: string
  name?: string
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
  stocks?: StockRef[]
  start?: string
  end?: string
  metrics: BacktestMetrics
  equity: { date: string; equity: number }[]
}

export interface StockRef {
  code: string
  name: string
  industry?: string
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
  date: string | null
  prev_date?: string | null
  items: PickItem[]
  /** 调出名单:可能是对象或纯代码字符串 */
  dropped?: (PickItem | string)[]
}

export interface ScreenerItem {
  code: string
  name: string
  close?: number
  /** 涨跌幅:实际响应为 pct_chg,契约曾用 chg_pct,两者都兼容 */
  pct_chg?: number
  chg_pct?: number
  high_dist?: number
  mom20?: number
  mom60?: number
  rsi14?: number
  vol_ratio5?: number
  amount_avg20?: number
  industry?: string
  matched_conditions?: string[]
  match_reasons?: (string | ScreenerMatchReason)[]
  values?: Record<string, unknown>
  [key: string]: unknown
}

export interface ScreenerResult {
  date?: string
  total?: number
  count?: number
  combined_count?: number
  candidate_count?: number
  field_coverage?: Record<string, number>
  universe?: string
  condition_counts?: Record<string, number> | ScreenerConditionCount[]
  independent_counts?: Record<string, number>
  data_policy?: {
    point_in_time?: boolean
    valuation_max_age_days?: number
  }
  items: ScreenerItem[]
}

export interface ScreenerMatchReason {
  condition_id: string
  field: string
  field_name?: string
  actual: unknown
  matched: boolean
}

export interface ScreenerConditionCount {
  id: string
  field: string
  field_name?: string
  matched: number
  total?: number
  available?: number
}

export type CatalogSection =
  | 'factors'
  | 'indicators'
  | 'strategies'
  | 'signals'
  | 'backtest_metrics'
  | 'filter_fields'

export interface CatalogOption {
  value: string | number | boolean
  label: string
}

export interface CatalogEntry {
  key: string
  name: string
  description: string
  category?: string
  unit?: string
  direction?: string
  formula?: string
  caliber?: string
  caveat?: string
  limits?: string
  source?: string
  available?: boolean
  kind?: string
  kind_name?: string
  params?: CatalogParameter[]
  constraints?: string[]
  data_type?: 'number' | 'integer' | 'boolean' | 'string' | 'select'
  value_type?: 'number' | 'integer' | 'boolean' | 'string' | 'select'
  input_scale?: number
  operators?: string[]
  options?: CatalogOption[]
}

export interface CatalogParameter {
  key: string
  name: string
  description?: string
  default?: number | string | boolean
  value_type?: 'number' | 'integer' | 'boolean' | 'string'
  unit?: string
  minimum?: number
  maximum?: number
  step?: number
}

export interface CatalogPayload {
  factors: CatalogEntry[]
  indicators: CatalogEntry[]
  strategies: CatalogEntry[]
  signals: CatalogEntry[]
  backtest_metrics: CatalogEntry[]
  filter_fields: CatalogEntry[]
}

export type FilterLogic = 'and' | 'or'

export interface ScreenerCondition {
  id: string
  field: string
  operator: string
  value: string | number | boolean | null
  value2?: string | number | null
  enabled: boolean
}

export interface ScreenerGroup {
  id: string
  logic: FilterLogic
  conditions: ScreenerCondition[]
}

export interface StructuredScreenerRequest {
  date?: string
  logic: FilterLogic
  groups: ScreenerGroup[]
  limit?: number
  universe?: 'pool' | 'hs300_zz500' | 'hs300' | 'zz500' | 'watchlist' | 'all'
}

export interface StrategyListResult {
  strategies: string[]
  items?: CatalogEntry[]
  single?: string[]
  portfolio?: string[]
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
  stocks?: StockRef[]
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
    return request<{ code: string; name?: string; industry?: string; count: number; bars: KlineBar[] }>(
      `/api/market/kline?${params}`
    )
  },

  snapshot() {
    return request<{ count: number; items: SnapshotItem[] }>('/api/market/snapshot')
  },

  stockSearch(query: string, limit = 10) {
    const params = new URLSearchParams({ q: query, limit: String(limit) })
    return request<{ count?: number; items: StockSearchItem[] }>(`/api/market/stocks?${params}`)
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
    return request<StrategyListResult>('/api/backtest/strategies')
  },

  catalog() {
    return request<Partial<CatalogPayload>>('/api/catalog')
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

  async structuredScreener(body: StructuredScreenerRequest) {
    try {
      return await request<ScreenerResult>('/api/selection/screener', {
        method: 'POST',
        body: JSON.stringify(body),
      })
    } catch (error) {
      const status = (error as Error & { status?: number }).status
      if (status !== 404 && status !== 405) throw error

      const active = body.groups.flatMap((group) => group.conditions).filter((condition) => condition.enabled)
      const legacy: {
        pct_chg_min?: number
        pct_chg_max?: number
        vol_ratio_min?: number
        ma_bull?: boolean
        high_window?: number
        high_dist_max?: number
        amount_min?: number
      } = {}
      for (const condition of active) {
        const value = Number(condition.value)
        if (condition.field === 'pct_chg' && condition.operator === 'gte') legacy.pct_chg_min = value
        else if (condition.field === 'pct_chg' && condition.operator === 'lte') legacy.pct_chg_max = value
        else if (condition.field === 'vol_ratio5' && condition.operator === 'gte') legacy.vol_ratio_min = value
        else if (condition.field === 'ma_bull' && condition.operator === 'eq') legacy.ma_bull = Boolean(condition.value)
        else if (condition.field === 'high_window' && condition.operator === 'eq') legacy.high_window = value
        else if (condition.field === 'high_dist' && condition.operator === 'lte') legacy.high_dist_max = value
        else if (condition.field === 'amount_avg20' && condition.operator === 'gte') legacy.amount_min = value
        else throw error
      }
      return api.screener(legacy)
    }
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
