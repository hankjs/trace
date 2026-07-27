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

/**
 * 策略实例 = 算法模板 + 一组参数 + 用户起的名字。
 *
 * 与股票池同一套归属模型:公共策略(is_system)全用户可读且只读,
 * 自定义策略按 owner_id 归属。别人的策略后端返回 404 而不是 403。
 */
export interface Strategy {
  id: number
  /** 用户起的名字,同一属主内唯一(重名后端返回 409) */
  name: string
  /** 算法模板 key,如 ma_cross。建好后不可改,换算法即新建策略 */
  template: string
  /** 算法模板的中文名 */
  template_name: string
  kind: StrategyKind
  kind_name: string
  /** 只含用户显式覆盖的键;模板默认值调整后未覆盖的参数会跟着变 */
  params: Record<string, StrategyParamValue>
  /** params 合并模板默认值后的实际生效参数,表单初值取这里 */
  effective_params: Record<string, StrategyParamValue>
  /** false = 库里残留了模板已不认识的参数键,需用户修正后才能跑 */
  params_valid: boolean
  /** 启用的策略每天参与信号计算,单独占 max_enabled 配额 */
  enabled: boolean
  /** true = 全局共享的公共策略,只读 */
  is_system: boolean
  /** 属主。公共策略为哨兵 UUID(全零),不对应真实用户 */
  owner_id?: string | null
  /** 后端算好的可写标记,前端据此决定是否展示改名/删除入口 */
  editable: boolean
  /** 被多少条回测引用;>0 时删除会 409,只能改为停用 */
  backtest_count?: number | null
  created_at?: string | null
  /** 模板可生成哪些研究线/条件，由后端按模板声明。 */
  plan_capabilities?: ResearchPlanCapabilities | null
  research_plan_capabilities?: ResearchPlanCapabilities | null
}

/** single 逐只股票跑;portfolio 在股票池上排序后模拟持有一组 */
export type StrategyKind = 'single' | 'portfolio'

export type StrategyOverlayType = 'fixed_pct' | 'atr_multiple'

/** 风险与止盈都是策略参数，关闭时不参与信号和回测。 */
export interface StrategyOverlayConfig {
  enabled: boolean
  type: StrategyOverlayType
  value: number
  atr_period: number
}

/** 策略参数值。覆盖层以内嵌对象跟随策略版本化保存。 */
export type StrategyParamValue = number | string | boolean | StrategyOverlayConfig

export type ResearchPlanStatus =
  | 'active'
  | 'needs_review'
  | 'invalidated'
  | 'exit_triggered'
  | 'expired'

export type ResearchPlanType = 'single' | 'portfolio_rebalance'
export type ResearchPlanSignalType = 'buy' | 'sell' | 'watch' | 'hold' | 'rebalance' | 'qualification_change'
export type EntryObservationMode = 'none' | 'line' | 'range' | 'portfolio_rebalance'
export type ResearchRuleSource = 'entry' | 'native_risk' | 'risk_overlay' | 'take_profit' | 'native_exit' | 'rebalance'

export interface ResearchPriceReference {
  id?: string
  source: ResearchRuleSource
  name: string
  /** 单线使用 value，区间使用 lower/upper；无客观价格时三者均不返回。 */
  value?: number | null
  lower?: number | null
  upper?: number | null
  data_date: string
  calculation?: string | null
  status?: string | null
}

export interface ResearchCondition {
  id?: string
  source: ResearchRuleSource
  name: string
  summary: string
  formula?: string | null
  threshold?: number | string | null
  current_value?: number | string | null
  unit?: string | null
  data_date?: string | null
  status?: string | null
  triggered?: boolean
  price_reference?: ResearchPriceReference | null
}

export interface EntryObservation {
  mode: EntryObservationMode
  summary: string
  calculation?: string | null
  data_date: string
  valid_until?: string | null
  review_condition?: string | null
  line?: ResearchPriceReference | null
  range?: ResearchPriceReference | null
  conditions?: ResearchCondition[]
}

export interface ResearchExitRule extends ResearchCondition {
  priority?: number
  enabled?: boolean
  /** 动态条件为 true 时，不应解释成预先确定的未来价格。 */
  dynamic?: boolean
}

export interface BacktestCostSnapshot {
  commission?: number
  stamp_tax?: number
  slippage?: number
  [key: string]: number | undefined
}

export interface BacktestEvidence {
  status: 'verified' | 'unverified'
  exact_match: boolean
  backtest_id?: number | null
  start?: string | null
  end?: string | null
  metrics?: Partial<BacktestMetrics> | null
  costs?: BacktestCostSnapshot | null
  message?: string | null
}

export type PortfolioChangeType = 'new' | 'keep' | 'increase' | 'decrease' | 'remove' | 'risk_filtered'

export interface PortfolioScoreFactor {
  name: string
  value: number
  weight: number
  contribution: number
}

export interface PortfolioWeightChange {
  code: string
  name?: string
  change_type: PortfolioChangeType
  change_name?: string
  previous_weight: number
  target_weight: number
  score?: number | null
  score_details?: Record<string, PortfolioScoreFactor>
  rank?: number | null
  reasons: string[]
  risk_rules?: ResearchExitRule[]
  risk_reference?: ResearchPriceReference | null
}

export interface PortfolioRebalancePlan {
  pool_id?: number | null
  pool_name: string
  frequency: string
  plan_date: string
  next_simulated_trade_date: string
  cash_weight: number
  changes: PortfolioWeightChange[]
  risk_summary?: string | null
}

export interface ResearchPlanCapabilities {
  plan_type?: ResearchPlanType
  observation_kinds?: EntryObservationMode[]
  price_references?: string[]
  native_exit?: string[]
  entry_modes?: EntryObservationMode[]
  supports_risk_overlay?: boolean
  supports_take_profit?: boolean
  native_exit_types?: string[]
}

export interface ResearchPlanSummary {
  id: number
  type: ResearchPlanType
  status: ResearchPlanStatus
  status_name?: string
  status_reason?: string | null
  data_date: string
  generated_at?: string | null
  next_simulated_trade_date?: string | null
  signal_close_price?: number | null
  signal_type?: ResearchPlanSignalType | null
  entry?: EntryObservation | null
  risk_rules?: ResearchExitRule[]
  take_profit_rules?: ResearchExitRule[]
  native_exit_rules?: ResearchExitRule[]
  evidence?: BacktestEvidence | null
  rebalance?: PortfolioRebalancePlan | null
}

export interface ResearchPlan extends ResearchPlanSummary {
  strategy_id: number
  strategy_name: string
  template: string
  strategy_version: string
  params_snapshot: Record<string, StrategyParamValue>
  adjustment: string
  signal_side?: ResearchPlanSignalType | null
  signal_reason?: string | null
  calculation_notes?: string[]
}

type ApiRecord = Record<string, unknown>

function asRecord(value: unknown): ApiRecord {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as ApiRecord : {}
}

function asRecords(value: unknown): ApiRecord[] {
  return Array.isArray(value) ? value.map(asRecord) : []
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function normalizedPlanStatus(value: unknown): ResearchPlanStatus {
  const aliases: Record<string, ResearchPlanStatus> = {
    current: 'active',
    reevaluate: 'needs_review',
    invalid: 'invalidated',
    active: 'active',
    needs_review: 'needs_review',
    invalidated: 'invalidated',
    exit_triggered: 'exit_triggered',
    expired: 'expired',
  }
  return aliases[String(value)] ?? 'needs_review'
}

function calculationStatus(value: unknown): string {
  return {
    calculated: '已计算',
    insufficient_data: '数据不足',
    pending_simulated_entry: '等待模拟入场价后计算',
    disabled: '未启用',
  }[String(value)] ?? String(value || '按规则判断')
}

function normalizedRule(
  rawValue: unknown,
  source: ResearchRuleSource,
  dataDate: string,
  fallbackName: string
): ResearchExitRule {
  const raw = asRecord(rawValue)
  const referenceLine = asNumber(raw.reference_line)
  const currentValue = raw.current_value ?? asRecord(raw.current_values).fast_ma ?? null
  const summary = String(raw.condition ?? raw.explanation ?? calculationStatus(raw.calculation_status))
  return {
    source,
    name: String(raw.name ?? fallbackName),
    summary,
    current_value: typeof currentValue === 'number' || typeof currentValue === 'string' ? currentValue : null,
    data_date: String(raw.data_date ?? dataDate),
    status: calculationStatus(raw.calculation_status),
    enabled: typeof raw.enabled === 'boolean' ? raw.enabled : true,
    dynamic: referenceLine == null,
    price_reference: referenceLine == null ? null : {
      source,
      name: source === 'take_profit' ? '止盈参考线' : source === 'native_exit' ? '策略退出参考线' : '风险失效线',
      value: referenceLine,
      data_date: String(raw.data_date ?? dataDate),
      calculation: summary,
    },
  }
}

function normalizedEntry(rawValue: unknown, dataDate: string): EntryObservation {
  const raw = asRecord(rawValue)
  const mode = String(raw.kind ?? 'none') as EntryObservationMode
  const line = asNumber(raw.line)
  const lower = asNumber(raw.lower)
  const upper = asNumber(raw.upper)
  const conditions = asRecords(raw.conditions).map((condition, index): ResearchCondition => {
    const current = condition.value ?? condition.current_value ?? null
    const threshold = condition.threshold ?? null
    const summary = current == null
      ? '当前数据不足'
      : threshold == null
        ? `当前值 ${String(current)}`
        : `当前值 ${String(current)}，阈值 ${String(threshold)}`
    return {
      id: `entry-condition-${index}`,
      source: 'entry',
      name: String(condition.name ?? `观察条件 ${index + 1}`),
      summary,
      current_value: typeof current === 'number' || typeof current === 'string' ? current : null,
      threshold: typeof threshold === 'number' || typeof threshold === 'string' ? threshold : null,
      data_date: dataDate,
      status: calculationStatus(raw.calculation_status),
    }
  })
  return {
    mode,
    summary: String(raw.explanation ?? raw.name ?? '按策略条件观察'),
    calculation: calculationStatus(raw.calculation_status),
    data_date: String(raw.data_date ?? dataDate),
    valid_until: raw.valid_until == null ? null : String(raw.valid_until),
    review_condition: Array.isArray(raw.reevaluate_when) ? raw.reevaluate_when.map(String).join('；') : null,
    line: line == null ? null : {
      source: 'entry', name: '进场观察线', value: line,
      data_date: String(raw.data_date ?? dataDate), calculation: String(raw.explanation ?? ''),
    },
    range: lower == null || upper == null ? null : {
      source: 'entry', name: '进场观察区间', lower, upper,
      data_date: String(raw.data_date ?? dataDate), calculation: String(raw.explanation ?? ''),
    },
    conditions,
  }
}

function normalizedPortfolioChangeType(value: unknown): PortfolioChangeType {
  const aliases: Record<string, PortfolioChangeType> = {
    added: 'new', retained: 'keep', increased: 'increase', reduced: 'decrease', removed: 'remove',
    new: 'new', keep: 'keep', increase: 'increase', decrease: 'decrease', remove: 'remove', risk_filtered: 'risk_filtered',
  }
  return aliases[String(value)] ?? 'keep'
}

/** 后端保存忠实快照，API 层把模板差异归一为页面的连续阅读模型。 */
export function normalizeResearchPlanResponse(value: unknown): ResearchPlanSummary | ResearchPlan {
  const raw = asRecord(value)
  const dataDate = String(raw.data_date ?? '')
  const status = normalizedPlanStatus(raw.status)
  const statusReason = asRecord(raw.status_reason)
  const planType = String(raw.plan_type ?? 'single') as ResearchPlanType
  const evidenceRaw = asRecord(raw.backtest_evidence)
  const evidenceStatus = String(evidenceRaw.status ?? raw.backtest_status ?? 'unverified')
  const takeProfitRaw = asRecord(raw.take_profit)
  const portfolioSummary = asRecord(raw.portfolio_summary)
  const changes = asRecords(raw.portfolio_changes)
  const base: ResearchPlanSummary = {
    id: Number(raw.plan_id ?? raw.id ?? 0),
    type: planType,
    status,
    status_name: typeof raw.status_name === 'string' ? raw.status_name : undefined,
    status_reason: typeof raw.status_reason === 'string'
      ? raw.status_reason
      : typeof statusReason.text === 'string' ? statusReason.text : null,
    data_date: dataDate,
    generated_at: raw.generated_at == null ? null : String(raw.generated_at),
    next_simulated_trade_date: raw.next_simulated_execution_date == null
      ? null : String(raw.next_simulated_execution_date),
    signal_close_price: asNumber(raw.signal_close_price),
    signal_type: raw.signal_type == null ? null : raw.signal_type as ResearchPlanSignalType,
    entry: normalizedEntry(raw.entry_observation, dataDate),
    risk_rules: asRecords(raw.risk_rules).map((rule) => normalizedRule(
      rule,
      asRecord(rule).source === 'overlay' ? 'risk_overlay' : 'native_risk',
      dataDate,
      '风险失效条件'
    )),
    take_profit_rules: takeProfitRaw.enabled === true
      ? [normalizedRule(takeProfitRaw, 'take_profit', dataDate, '止盈覆盖层')]
      : [],
    native_exit_rules: asRecords(raw.native_exit).map((rule) => normalizedRule(rule, 'native_exit', dataDate, '策略退出条件')),
    evidence: {
      status: evidenceStatus === 'verified' ? 'verified' : 'unverified',
      exact_match: evidenceStatus === 'verified',
      backtest_id: asNumber(evidenceRaw.run_id),
      start: evidenceRaw.start == null ? null : String(evidenceRaw.start),
      end: evidenceRaw.end == null ? null : String(evidenceRaw.end),
      metrics: asRecord(evidenceRaw.metrics) as Partial<BacktestMetrics>,
      costs: asRecord(evidenceRaw.costs) as BacktestCostSnapshot,
      message: evidenceRaw.reason == null ? null : String(evidenceRaw.reason),
    },
    rebalance: planType !== 'portfolio_rebalance' ? null : {
      pool_name: String(portfolioSummary.pool_name ?? '股票池待同步'),
      frequency: String(portfolioSummary.frequency ?? '按策略频率'),
      plan_date: dataDate,
      next_simulated_trade_date: raw.next_simulated_execution_date == null ? '待交易日历确认' : String(raw.next_simulated_execution_date),
      cash_weight: asNumber(portfolioSummary.cash_weight) ?? 0,
      changes: changes.map((change): PortfolioWeightChange => {
        const riskSnapshot = asRecord(change.risk_snapshot)
        const scoreSnapshot = asRecord(change.score_details)
        const scoreFactors = asRecord(scoreSnapshot.factors)
        const riskLine = asNumber(riskSnapshot.reference_line)
        const holdingRules = asRecords(riskSnapshot.rules).map((rule) => {
          const source = String(rule.source)
          const normalizedSource: ResearchRuleSource = source === 'take_profit'
            ? 'take_profit'
            : source === 'risk_overlay' ? 'risk_overlay' : 'native_risk'
          return normalizedRule(
            rule,
            normalizedSource,
            String(rule.data_date ?? dataDate),
            normalizedSource === 'take_profit' ? '止盈覆盖层' : '风险失效条件'
          )
        })
        return {
          code: String(change.code ?? ''),
          name: change.name == null ? undefined : String(change.name),
          change_type: normalizedPortfolioChangeType(change.change_type),
          previous_weight: asNumber(change.previous_weight) ?? 0,
          target_weight: asNumber(change.target_weight) ?? 0,
          score: asNumber(change.score),
          score_details: Object.fromEntries(
            Object.entries(scoreFactors).flatMap(([key, value]) => {
              const factor = asRecord(value)
              const factorValue = asNumber(factor.value)
              const weight = asNumber(factor.weight)
              const contribution = asNumber(factor.contribution)
              if (factorValue == null || weight == null || contribution == null) return []
              return [[key, {
                name: String(factor.name ?? key),
                value: factorValue,
                weight,
                contribution,
              }]]
            })
          ),
          rank: asNumber(change.rank),
          reasons: Array.isArray(change.reasons) ? change.reasons.map((reason) => {
            if (typeof reason === 'string') return reason
            const item = asRecord(reason)
            return String(item.text ?? item.name ?? '未提供结构化原因')
          }) : [],
          risk_rules: holdingRules,
          risk_reference: riskLine == null ? null : {
            source: 'native_risk',
            name: String(riskSnapshot.name ?? '风险过滤线'),
            value: riskLine,
            data_date: String(riskSnapshot.data_date ?? dataDate),
          },
        }
      }),
      risk_summary: `风险规则 ${asRecords(raw.risk_rules).length} 条；止盈覆盖层${takeProfitRaw.enabled === true ? '已启用' : '未启用'}。`,
    },
  }
  const strategy = asRecord(raw.strategy)
  if (!Object.keys(strategy).length) return base
  const snapshot = asRecord(raw.params_snapshot)
  const effective = Object.keys(asRecord(snapshot.effective_params)).length
    ? asRecord(snapshot.effective_params)
    : snapshot
  return {
    ...base,
    strategy_id: Number(strategy.id ?? 0),
    strategy_name: String(strategy.name ?? ''),
    template: String(strategy.template ?? ''),
    strategy_version: String(strategy.version ?? ''),
    params_snapshot: effective as Record<string, StrategyParamValue>,
    adjustment: raw.price_adjustment === 'forward' ? '前复权' : String(raw.price_adjustment ?? '复权口径待同步'),
    signal_side: raw.signal_type == null ? null : raw.signal_type as ResearchPlanSignalType,
  }
}

/** 每个用户的策略数量与启用数上限,由后端下发 */
export interface StrategyLimits {
  max_total: number
  max_enabled: number
}

/** 公共策略只读,改名/改参数/删除一律走「另存为我的策略」 */
export function isPresetStrategy(strategy: Strategy | null | undefined): boolean {
  return !!strategy && strategy.is_system === true
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
  /** 产生该提示的策略实例 id */
  strategy_id: number
  /** 策略实例名(用户自定义),不是算法模板名 */
  strategy_name?: string
  /** 算法模板 key,如 ma_cross */
  template?: string
  is_system?: boolean
  side: 'buy' | 'sell' | 'watch'
  side_name?: string
  /** 新契约明确命名；price 仅保留给过渡期响应。 */
  signal_close_price?: number
  price: number
  reason?: Record<string, unknown>
  reason_text?: string
  research_plan_id?: number | null
  plan_status?: ResearchPlanStatus | null
  plan_status_name?: string | null
  plan_summary?: ResearchPlanSummary | null
  research_plan?: ResearchPlan | null
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
  evidence?: BacktestRunEvidence
}

export interface BacktestResult {
  run_id: number
  strategy_id?: number
  /** 策略实例名;策略行已被删除时后端回显 null */
  strategy_name?: string | null
  template?: string | null
  /** 本次实际生效的全量参数快照 */
  params?: Record<string, StrategyParamValue>
  /** 本次回测实际使用的覆盖层与费用快照。 */
  risk_overlay?: StrategyOverlayConfig | null
  take_profit?: StrategyOverlayConfig | null
  costs?: BacktestCostSnapshot
  codes?: string[]
  stocks?: StockRef[]
  start?: string
  end?: string
  /** 组合回测所用股票池;静态池需在结果页标注幸存者偏差 */
  pool?: PoolRef
  metrics: BacktestMetrics
  equity: { date: string; equity: number }[]
  evidence?: BacktestRunEvidence
  trade_details?: BacktestTradeDetail[]
  exit_reason_distribution?: BacktestExitReasonDistribution | BacktestExitReasonCount[] | Record<string, number>
  trades?: BacktestTrade[]
}

export interface BacktestExitReason {
  code: string
  name: string
  price_line?: number | null
}

export interface BacktestTradeDetail {
  code: string
  name?: string
  signal_date?: string | null
  execution_date: string
  execution_price: number
  size: number
  fees: number
  side: 'buy' | 'sell'
  primary_reason?: BacktestExitReason | null
  all_reasons: BacktestExitReason[]
  tradable: boolean
  execution_status: string
  closed_trades: number
  winning_trades: number
  realized_pnl: number
}

export interface BacktestExitReasonDistribution {
  by_primary: Record<string, number>
  all_hits: Record<string, number>
}

export interface BacktestRunEvidence {
  parameter_snapshot?: Record<string, StrategyParamValue>
  fee_assumptions?: Record<string, unknown>
  trade_details?: BacktestTradeDetail[]
  exit_reason_distribution?: BacktestExitReasonDistribution
  start?: string
  end?: string
}

export interface BacktestExitReasonCount {
  reason: string
  reason_name?: string
  count: number
  primary_count?: number
}

export interface BacktestTrade {
  id?: number | string
  code: string
  name?: string
  side?: 'buy' | 'sell'
  signal_date: string
  simulated_trade_date: string
  simulated_price: number
  exit_signal_date?: string | null
  exit_trade_date?: string | null
  exit_price?: number | null
  primary_exit_reason?: string | null
  exit_reasons: string[]
  fees?: number | null
  pnl?: number | null
  pnl_pct?: number | null
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
  | 'strategy_templates'
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
  plan_capabilities?: ResearchPlanCapabilities
  plan_capability?: Record<string, unknown>
  research_plan_capabilities?: ResearchPlanCapabilities
}

export interface CatalogParameter {
  key: string
  name: string
  description?: string
  default?: number | string | boolean | StrategyOverlayConfig
  value_type?: 'number' | 'integer' | 'boolean' | 'string' | 'overlay'
  unit?: string
  minimum?: number
  maximum?: number
  step?: number
  fields?: Record<string, unknown>
}

export interface CatalogPayload {
  factors: CatalogEntry[]
  indicators: CatalogEntry[]
  /** 算法模板元数据(参数定义、限制);策略实例本身走 /api/strategies */
  strategy_templates: CatalogEntry[]
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

export interface LeaderboardItem {
  strategy_id: number
  /** 策略实例名 */
  strategy: string
  template: string
  is_system: boolean
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
  params: Record<string, StrategyParamValue>
  metrics: SweepMetrics
  per_code?: Record<string, Record<string, number>>
}

export interface SweepResult {
  strategy_id: number
  strategy_name: string
  template: string
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

  signals(filters: { date?: string; code?: string; strategy_id?: number; side?: string; limit?: number } = {}) {
    const params = new URLSearchParams()
    if (filters.date) params.set('date', filters.date)
    if (filters.code) params.set('code', filters.code)
    if (filters.strategy_id) params.set('strategy_id', String(filters.strategy_id))
    if (filters.side) params.set('side', filters.side)
    if (filters.limit) params.set('limit', String(filters.limit))
    return request<{ count: number; items: SignalItem[] }>(`/api/signals?${params}`).then((payload) => ({
      ...payload,
      items: payload.items.map((signal): SignalItem => {
        const normalized = signal.research_plan
          ? normalizeResearchPlanResponse(signal.research_plan)
          : null
        return {
          ...signal,
          research_plan_id: normalized?.id ?? signal.research_plan_id,
          plan_status: normalized?.status ?? signal.plan_status,
          plan_status_name: normalized?.status_name ?? signal.plan_status_name,
          plan_summary: normalized ?? signal.plan_summary,
          research_plan: normalized && 'strategy_id' in normalized ? normalized as ResearchPlan : null,
        }
      }),
    }))
  },

  researchPlan(planId: number) {
    return request<unknown>(`/api/research-plans/${planId}`)
      .then((plan) => normalizeResearchPlanResponse(plan) as ResearchPlan)
  },

  stockResearchPlans(code: string, limit = 20) {
    const params = new URLSearchParams({ code, limit: String(limit) })
    return request<{ count: number; items: unknown[] }>(`/api/research-plans?${params}`)
      .then((payload) => ({ ...payload, items: payload.items.map(normalizeResearchPlanResponse) }))
  },

  portfolioResearchPlans(filters: { strategy_id?: number; date?: string; limit?: number } = {}) {
    const params = new URLSearchParams()
    if (filters.strategy_id) params.set('strategy_id', String(filters.strategy_id))
    if (filters.date) params.set('date', filters.date)
    if (filters.limit) params.set('limit', String(filters.limit))
    params.set('plan_type', 'portfolio_rebalance')
    return request<{ count: number; items: unknown[] }>(`/api/research-plans?${params}`)
      .then((payload) => ({ ...payload, items: payload.items.map(normalizeResearchPlanResponse) }))
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

  catalog() {
    return request<Partial<CatalogPayload>>('/api/catalog')
  },

  // ---- 策略 ----

  strategies() {
    return request<{ count?: number; items: Strategy[]; limits: StrategyLimits }>('/api/strategies')
  },

  strategy(id: number) {
    return request<Strategy>(`/api/strategies/${id}`)
  },

  /** 算法模板元数据(参数定义等)。与目录的 strategy_templates 同源,单独一份省一次整份目录请求 */
  strategyTemplates() {
    return request<{ items: CatalogEntry[] }>('/api/strategies/templates')
  },

  /** params 只需给要覆盖模板默认值的键 */
  createStrategy(body: {
    name: string
    template: string
    params?: Record<string, StrategyParamValue>
    enabled?: boolean
  }) {
    return request<Strategy>('/api/strategies', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 另存为我的策略。公共策略只读,调参前先复制一份;name 留空由后端加「副本」 */
  duplicateStrategy(id: number, body: { name?: string; params?: Record<string, StrategyParamValue> } = {}) {
    return request<Strategy>(`/api/strategies/${id}/duplicate`, {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 改名 / 改参数 / 启停。模板不可改,换算法请新建 */
  updateStrategy(id: number, body: {
    name?: string
    params?: Record<string, StrategyParamValue>
    enabled?: boolean
  }) {
    return request<Strategy>(`/api/strategies/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(body),
    })
  },

  /** 被回测引用时后端返回 409,提示改为停用 */
  deleteStrategy(id: number) {
    return request<{ deleted: number; id: number }>(`/api/strategies/${id}`, { method: 'DELETE' })
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
    strategy_id: number
    codes: string[]
    start: string
    end: string
    pool_id?: number
    /** 临时覆盖策略自身的参数,不改策略行 */
    params?: Record<string, unknown>
    costs?: BacktestCostSnapshot
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
    strategy_id: number
    codes: string[]
    start: string
    end: string
    param_grid: Record<string, Array<number | string | boolean>>
    costs?: BacktestCostSnapshot
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
