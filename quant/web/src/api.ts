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

/**
 * 股票池种类:
 * - index  预置指数成分池(沪深300/中证500 等),按 in_date/out_date 做 point-in-time 解析
 * - all    全部A股,按 list_date/delist_date/is_st 解析,同样 point-in-time
 * - static 自定义静态池,只存代码不存日期,历史区间存在幸存者偏差
 */
export type PoolKind = 'index' | 'all' | 'static'

export interface Pool {
  id: number
  kind: PoolKind
  /** kind='index' 时的指数引用,如 hs300_zz500 */
  ref?: string | null
  name: string
  /** 新股上市满多少天才纳入,预置指数池为 0 */
  min_list_days: number
  /** true = 全局共享的系统预置池,只读 */
  is_system: boolean
  /** 属主。系统池为哨兵 UUID(全零),不对应真实用户 */
  owner_id?: string | null
  member_count?: number | null
  created_at?: string | null
}

/** 筛选/回测响应里回显的池信息 */
export interface PoolRef {
  id: number
  name: string
  kind: PoolKind
  /** 后端显式回传;缺省时前端按 kind==='static' 推断 */
  has_survivorship_bias?: boolean
}

export interface PoolMember {
  code: string
  name?: string
  industry?: string
}

/**
 * 预置池不可改名、不可增删成员,只能「另存为」自定义池。
 *
 * 用后端的 is_system 而不是 kind!=='static' 推断:后者在出现 kind='static'
 * 的系统池时会判断错误,而权限判断不该依赖这种巧合。
 */
export function isPresetPool(pool: Pool | null | undefined): boolean {
  return !!pool && pool.is_system === true
}

/** 静态池无成员历史,用于历史区间时结果含幸存者偏差 */
export function hasSurvivorshipBias(pool: Pool | PoolRef | null | undefined): boolean {
  if (!pool) return false
  if ('has_survivorship_bias' in pool && typeof pool.has_survivorship_bias === 'boolean') {
    return pool.has_survivorship_bias
  }
  return pool.kind === 'static'
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
  /** 组合回测所用股票池;静态池需在结果页标注幸存者偏差 */
  pool?: PoolRef
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
  /** 相对前一交易日的变动:'new' 新进 / 'keep' 保留 */
  change?: string | null
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
  /** 涨跌幅 */
  pct_chg?: number
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
  /** 本次实际使用的股票池(取代旧的 universe 字符串回显) */
  pool?: PoolRef
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
  /** 股票池 id;不传由后端取默认池(全部A股) */
  pool_id?: number
  /**
   * 只筛自选。自选是用户关系而非股票池,做成池会引入「自选变化时池成员
   * 如何同步」的问题,故为独立开关。置 true 时后端忽略 pool_id。
   */
  watchlist_only?: boolean
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

  // ---- 股票池组 ----

  pools() {
    return request<{ count?: number; items: Pool[] }>('/api/pools')
  },

  pool(id: number) {
    return request<Pool>(`/api/pools/${id}`)
  },

  createPool(body: { name: string; min_list_days?: number; codes?: string[] }) {
    return request<Pool>('/api/pools', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  updatePool(id: number, body: { name?: string; min_list_days?: number }) {
    return request<Pool>(`/api/pools/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(body),
    })
  },

  deletePool(id: number) {
    return request<{ deleted: number }>(`/api/pools/${id}`, { method: 'DELETE' })
  },

  poolMembers(id: number) {
    return request<{ count?: number; items: PoolMember[] }>(`/api/pools/${id}/members`)
  },

  /** 批量增加成员(粘贴导入用),返回实际写入与被忽略的代码 */
  addPoolMembers(id: number, codes: string[]) {
    return request<{ added: number; skipped?: string[]; items?: PoolMember[] }>(`/api/pools/${id}/members`, {
      method: 'POST',
      body: JSON.stringify({ codes }),
    })
  },

  removePoolMember(id: number, code: string) {
    return request<{ deleted: number }>(`/api/pools/${id}/members/${code}`, { method: 'DELETE' })
  },

  runBacktest(body: {
    strategy: string
    codes: string[]
    start: string
    end: string
    pool_id?: number
    params?: Record<string, unknown>
    costs?: Record<string, unknown>
  }) {
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

  // 结构化筛选:直连 POST /api/selection/screener。
  // 这里刻意不做任何降级重试。历史上曾在接口不可用时改调旧版 GET 接口,
  // 但旧接口不支持条件组的 OR 逻辑,也不支持任何基本面条件,
  // 结果是用户看到与所设筛选条件不符的列表却没有任何提示。
  // 现在失败就直接抛错,由页面把错误呈现给用户。
  structuredScreener(body: StructuredScreenerRequest) {
    return request<ScreenerResult>('/api/selection/screener', {
      method: 'POST',
      body: JSON.stringify(body),
    })
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
