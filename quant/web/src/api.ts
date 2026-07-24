/** 后端 /api 封装:统一 fetch、错误处理(HTTP 错误取 FastAPI 的 detail)。 */

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
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
  side: 'buy' | 'sell'
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

export const api = {
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
