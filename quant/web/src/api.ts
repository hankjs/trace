/** 后端 /api 封装:统一 fetch、错误处理(HTTP 错误取 FastAPI 的 detail)。 */

const TOKEN_KEY = 'quant_token'
const USERNAME_KEY = 'quant_username'
const CAN_ADMIN_KEY = 'quant_can_admin'

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setAuth(token: string, username: string, canAdmin = false) {
  localStorage.setItem(TOKEN_KEY, token)
  localStorage.setItem(USERNAME_KEY, username)
  localStorage.setItem(CAN_ADMIN_KEY, String(canAdmin))
}

export function clearAuth() {
  localStorage.removeItem(TOKEN_KEY)
  localStorage.removeItem(USERNAME_KEY)
  localStorage.removeItem(CAN_ADMIN_KEY)
}

export function currentUsername(): string {
  return localStorage.getItem(USERNAME_KEY) ?? ''
}

/** 仅用于界面显隐;接口鉴权以后端 require_admin 为准 */
export function isAdmin(): boolean {
  return localStorage.getItem(CAN_ADMIN_KEY) === 'true'
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
    let detail: unknown
    try {
      const json = await res.json()
      detail = json.detail
      if (typeof json.detail === 'string') msg = json.detail
      else if (json.detail && typeof json.detail.message === 'string') msg = json.detail.message
      else if (json.detail && typeof json.detail.error === 'string') msg = json.detail.error
      else if (Array.isArray(json.detail) && json.detail[0]?.msg) msg = json.detail[0].msg
    } catch {
      /* 非 JSON 错误体 */
    }
    const error = new Error(msg) as Error & { status?: number, detail?: unknown }
    error.status = res.status
    error.detail = detail
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
  /** 最新参考价(盘中快照优先,否则最近收盘);无行情数据为 null */
  price?: number | null
  /** 仅盘中快照提供涨跌幅(小数) */
  pct_chg?: number | null
  price_ts?: string | null
  price_source?: 'snapshot' | 'close' | null
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

export type StrategyResearchStatus = 'unverified' | 'verified' | 'rejected'

/** 规格 metadata.evidence_status:服务端状态机管理,前端只展示与触发允许的手动操作 */
export type StrategyEvidenceStatus =
  | 'unverified'
  | 'design_complete'
  | 'backtested'
  | 'oos_passed'
  | 'rejected'

export type StrategyEvidenceAction = 'mark_design_complete' | 'reset_rejected'

export type StrategyCapabilityStatus =
  | 'supported'
  | 'missing_data'
  | 'missing_engine'
  | 'subjective_only'
  | 'boundary_denied'

export interface StrategyCapabilityIssue {
  status: StrategyCapabilityStatus
  code: string
  path: string
  message: string
  operator?: string | null
  field?: string | null
}

export interface StrategyCapability {
  status: StrategyCapabilityStatus
  issues: StrategyCapabilityIssue[]
}

/**
 * 受控表达式节点。字段与后端 app/strategy/spec.py 的 Expression 一一对应;
 * 每种 op 只允许自己的字段子集(多一个少一个后端都拒绝),这里把全部可能
 * 字段列为可选,具体形状由 specExpression.ts 的算子注册表约束。
 */
export interface StrategyAstNode {
  op: string
  /** field: snake_case 字段名 */
  name?: string
  /** literal: 有限数字或布尔值 */
  value?: number | boolean
  /** all/any: 非空布尔子节点列表 */
  args?: StrategyAstNode[]
  /** not: 布尔子节点 */
  arg?: StrategyAstNode
  /** 比较/算术: 数值子节点 */
  left?: StrategyAstNode
  right?: StrategyAstNode
  /** 滚动/位移/指标/横截面: 数值子节点 */
  input?: StrategyAstNode
  /** atr: 三个数值子节点 */
  high?: StrategyAstNode
  low?: StrategyAstNode
  close?: StrategyAstNode
  window?: number
  shift?: number
  periods?: number
  /** rank: true 升序 */
  ascending?: boolean
  /** top_n */
  n?: number
}

/** entry / native_exit 的规则形状:{condition, reason_code} */
export interface StrategyRuleSpec {
  condition: StrategyAstNode
  /** snake_case 原因码 */
  reason_code: string
}

export type StrategyDataAvailability = 'daily_close' | 'daily_open' | 'point_in_time'

export interface StrategyDataRequirement {
  field: string
  availability: StrategyDataAvailability
  required: boolean
}

/**
 * 数据库中的完整策略定义。具体节点由服务端受控注册表校验，前端只构造已知节点，
 * 同时保留未知扩展字段，避免查看新版规格时丢失信息。
 */
export interface StrategySpec {
  schema_version: number
  kind?: StrategyKind
  metadata: Record<string, unknown>
  universe: Record<string, unknown>
  data_requirements: StrategyDataRequirement[]
  entry: StrategyRuleSpec
  positioning: Record<string, unknown>
  holding: Record<string, unknown>
  /** single 必须非 null;portfolio 为 null */
  native_exit: StrategyRuleSpec | null
  overlays: Record<string, unknown>
  portfolio_constraints: Record<string, unknown>
  execution: Record<string, unknown>
  validation: Record<string, unknown>
  [key: string]: unknown
}

interface StrategyValidationBase {
  capability: StrategyCapability
  errors: string[]
}

export type StrategyValidationResult = StrategyValidationBase & (
  | {
      valid: true
      kind: StrategyKind
      spec_schema_version: number
      normalized_spec: StrategySpec
      spec_hash: string
    }
  | {
      valid: false
      kind: StrategyKind | null
      spec_schema_version: number | null
      normalized_spec: StrategySpec | null
      spec_hash: string | null
    }
)

/**
 * 策略实例以 spec 为唯一规则来源。公共策略(is_system)全用户可读且只读，
 * 自定义策略按 owner_id 归属。legacy 模板字段仅供迁移期旧页面读取。
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
  /** 其中 spec_hash 与当前规格完全一致、可作为当前证据的回测数。 */
  evidence_backtest_count?: number | null
  created_at?: string | null
  updated_at?: string | null
  spec_schema_version?: number
  spec?: StrategySpec
  spec_hash?: string
  /** 服务端状态机维护的证据状态;自动推进的状态不允许手改 */
  evidence_status?: StrategyEvidenceStatus
  /** 当前状态允许的手动操作(仅 editable 策略非空) */
  evidence_actions?: StrategyEvidenceAction[]
  research_status?: StrategyResearchStatus
  capability?: StrategyCapability
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
export type ResearchPlanSignalType = 'buy' | 'sell' | 'watch' | 'add' | 'reduce' | 'hold' | 'rebalance' | 'qualification_change'
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
  side: 'buy' | 'sell' | 'watch' | 'add' | 'reduce'
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
  validation?: BacktestValidation | null
}

/** 回测完成后的规格 validation 段执行报告(基线对比 / OOS 分段 / 否决判定) */
export interface BacktestValidation {
  baselines: BacktestValidationBaseline[]
  oos: BacktestValidationOos | null
  rejection: BacktestRejection
}

export interface BacktestValidationBaseline {
  baseline_id: string
  name: string
  status: 'ok' | 'unavailable'
  message?: string | null
  metrics?: {
    total_return?: number | null
    annual_return?: number | null
    max_drawdown?: number | null
    sharpe?: number | null
  } | null
  /** 策略 - 基线(越高越好) */
  delta?: Record<string, number | null> | null
}

export interface BacktestValidationOos {
  enabled: boolean
  available?: boolean
  fraction?: number
  oos_start?: string | null
  in_sample_bars?: number
  oos_bars?: number
  in_sample?: Record<string, number | null> | null
  oos?: Record<string, number | null> | null
  message?: string | null
}

export interface BacktestRejectionHit {
  criterion: string
  detail: string
  metric?: string
  segment?: string
  op?: string
  threshold?: number | null
  actual?: number | null
}

export interface BacktestRejection {
  /** passed 全过 / incomplete 有未评估条件 / rejected 命中否决 */
  verdict: 'passed' | 'incomplete' | 'rejected'
  hits: BacktestRejectionHit[]
  unevaluated: { criterion: string, reason: string }[]
}

/** 全局或单次回测的数据信任信号 */
export interface DataQualitySummary {
  as_of?: string
  alert_level?: 'ok' | 'warning' | 'critical'
  stock_count?: number
  latest_bar_date?: string | null
  st_stock_coverage_ratio?: number
  st_bar_coverage_ratio?: number
  valuation_coverage_ratio?: number
  fundamental_coverage_ratio?: number
  adjust_factor_missing_stocks?: number
  /** ST 统计窗口(日历日),默认约 60 */
  st_window_days?: number | null
  st_window_start?: string | null
  st_window_end?: string | null
  /** 旁路缓存写入时间(ISO),缺省表示现场现算 */
  computed_at?: string
}

export interface BacktestDataQuality {
  st_history_incomplete?: boolean
  st_null_bar_ratio?: number
  st_incomplete_codes?: string[]
  st_incomplete_code_count?: number
  field_coverage?: Record<string, { available: number; total: number; ratio: number }>
  warnings?: string[]
}

export type BacktestJobStatus = 'pending' | 'running' | 'done' | 'failed'

export interface ExecutionAttribution {
  buy_blocked_limit_up_or_halt?: number
  buy_signal_days?: number
  buy_filled?: number
  sell_delayed?: number
  sell_signal_days?: number
  sell_filled?: number
  missing_bar_block?: number
}

export interface MultiplicityReport {
  n_trials: number
  n_evaluable: number
  alpha: number
  bonferroni_alpha?: number | null
  best_metric?: number | null
  best_params?: Record<string, unknown>
  disclaimer: string
}

export interface ExperimentSummary {
  id: number
  permanent_candidate_id: string
  family_id?: string | null
  title: string
  hypothesis: string
  strategy_id?: number | null
  frozen_spec_hash: string
  identity_hash: string
  status: string
  trial_count?: number | null
  created_at?: string | null
}

export interface ExperimentTrial {
  id: number
  experiment_id: number
  trial_index: number
  param_patch?: Record<string, unknown>
  backtest_run_id?: number | null
  outcome: 'ok' | 'no_trades' | 'error' | 'rejected' | string
  metrics_summary?: Partial<BacktestMetrics> | null
  error?: string | null
  data_fingerprint?: string | null
  execution_fingerprint?: string | null
  created_at?: string | null
}

export type EvidencePromotionStatus = 'pending' | 'accepted' | 'dismissed' | 'superseded'
export type EvidencePromotionTarget = 'backtested' | 'oos_passed' | 'rejected'

/** 试验达标后系统提名的证据推进待办(用户确认才改 evidence_status) */
export interface EvidencePromotionTodo {
  id: number
  owner_id: string
  strategy_id: number
  experiment_id: number
  trial_id: number
  backtest_run_id: number
  status: EvidencePromotionStatus
  suggested_target: EvidencePromotionTarget | string
  quality_checks?: Array<{ id: string, ok: boolean, message: string }>
  metrics_summary?: Partial<BacktestMetrics> | null
  created_at?: string | null
  resolved_at?: string | null
}

export interface EvidencePromotionEval {
  eligible: boolean
  suggested_target?: EvidencePromotionTarget | string | null
  checks?: Array<{ id: string, ok: boolean, message: string }>
  block_reasons?: string[]
  todo?: EvidencePromotionTodo | null
}

export interface ExperimentDetail extends ExperimentSummary {
  frozen_spec_snapshot?: StrategySpec
  validation_snapshot?: Record<string, unknown> | null
  universe_snapshot?: Record<string, unknown> | null
  cost_snapshot?: Record<string, unknown> | null
  trials: ExperimentTrial[]
  multiplicity?: MultiplicityReport
  evidence_promotions?: EvidencePromotionTodo[]
  pending_promotions?: EvidencePromotionTodo[]
}

export interface BacktestResult {
  run_id: number
  /** pending/running 时 metrics/equity 可能为空;轮询直至 done|failed */
  status?: BacktestJobStatus
  error?: string | null
  strategy_id?: number
  /** 策略实例名;策略行已被删除时后端回显 null */
  strategy_name?: string | null
  template?: string | null
  kind?: StrategyKind
  strategy_spec_snapshot?: StrategySpec
  strategy_spec_hash?: string
  compiler_version?: string
  component_versions?: Record<string, string>
  data_fingerprint?: string
  universe_fingerprint?: string
  cost_fingerprint?: string
  execution_fingerprint?: string
  requested_codes?: string[]
  created_at?: string
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
  /** ST/基本面覆盖等数据信任信号 */
  data_quality?: BacktestDataQuality
  /** 成交失败归因(涨停未买/跌停延迟卖出等) */
  execution_attribution?: ExecutionAttribution
  /** 规格 validation 段执行报告(基线对比 / OOS 分段 / 否决判定) */
  validation?: BacktestValidation | null
  /** 本次回测触发的证据状态迁移;未推进为 null */
  evidence_transition?: { from: StrategyEvidenceStatus, to: StrategyEvidenceStatus } | null
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
  strategy_spec_snapshot?: StrategySpec
  strategy_spec_hash?: string
  compiler_version?: string
  component_versions?: Record<string, string>
  data_fingerprint?: string
  universe_fingerprint?: string
  cost_fingerprint?: string
  execution_fingerprint?: string
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
  strategy_spec_hash?: string
  codes: string[]
  stocks?: StockRef[]
  start: string
  end: string
  results: SweepResultItem[]
  /** true = 按规格 validation.parameter_scans 声明执行的扫描 */
  declared?: boolean
  declared_scans?: { path: string, values: Array<number | boolean> }[]
  /** 按声明扫描的参数稳定性评估(unstable_parameters 的判定依据) */
  stability?: {
    status: 'evaluated' | 'unevaluated'
    reason?: string
    current?: number
    median?: number
    better_share?: number
    unstable?: boolean
    current_params?: Record<string, StrategyParamValue>
  } | null
}

export interface UserSettings {
  user_id: string
  can_trade_bse: boolean
  updated_at: string | null
}

/** 一次执行记录(系统调度或手动触发,落 quant_job_run 持久化) */
export interface AdminJobRun {
  id?: number
  job_id?: string
  trigger?: 'system' | 'manual'
  /** 手动触发者的用户名;系统执行为 null */
  operator?: string | null
  status: 'running' | 'finished' | 'failed'
  started_at: string
  finished_at: string | null
  result?: unknown
  error?: string | null
}

export interface AdminJob {
  id: string
  name: string
  description: string
  /** 人类可读的调度说明,如「交易日 16:30」 */
  schedule: string
  /** 本进程不负责调度(dev/未抢到互斥锁)时为 null */
  next_run_time: string | null
  last_system_run: AdminJobRun | null
  manual_run: AdminJobRun | null
}

export interface AdminJobsResponse {
  scheduler_running: boolean
  jobs: AdminJob[]
}

export const api = {
  login(username: string, password: string) {
    return request<{ token: string; username: string; can_admin: boolean }>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    })
  },

  /** 定时任务列表(仅 admin) */
  adminJobs() {
    return request<AdminJobsResponse>('/api/admin/jobs')
  },

  /** 手动触发定时任务(仅 admin),后台执行,轮询 adminJobs 看状态 */
  runAdminJob(jobId: string) {
    return request<{ status: string; job_id: string }>(
      `/api/admin/jobs/${encodeURIComponent(jobId)}/run`,
      { method: 'POST' },
    )
  },

  /** 单个任务的执行历史(仅 admin),新到旧 */
  adminJobRuns(jobId: string, limit = 20) {
    return request<AdminJobRun[]>(
      `/api/admin/jobs/${encodeURIComponent(jobId)}/runs?limit=${limit}`,
    )
  },

  getSettings() {
    return request<UserSettings>('/api/settings')
  },

  patchSettings(body: { can_trade_bse?: boolean }) {
    return request<UserSettings>('/api/settings', {
      method: 'PATCH',
      body: JSON.stringify(body),
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

  /** 数据信任摘要:ST/估值/财务覆盖率与告警级别 */
  dataQuality() {
    return request<DataQualitySummary>('/api/market/data-quality')
  },

  stockSearch(query: string, limit = 10) {
    const params = new URLSearchParams({ q: query, limit: String(limit) })
    return request<{ count?: number; items: StockSearchItem[] }>(`/api/market/stocks?${params}`)
  },

  /** 全市场股票清单:选股器一次拉取,客户端过滤与虚拟滚动 */
  stockList() {
    return request<{ count: number; items: StockSearchItem[] }>('/api/market/stocks?all=true')
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

  createStrategy(body: {
    name: string
    spec: StrategySpec
    enabled?: boolean
  }) {
    return request<Strategy>('/api/strategies', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 另存为我的策略。服务端复制当前完整 spec，name 留空时自动加「副本」。 */
  duplicateStrategy(
    id: number,
    body: { name?: string; spec?: StrategySpec } = {}
  ) {
    return request<Strategy>(`/api/strategies/${id}/duplicate`, {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 原地更新当前完整规格，不创建策略历史版本。 */
  updateStrategy(id: number, body: {
    name?: string
    spec?: StrategySpec
    enabled?: boolean
    research_status?: StrategyResearchStatus
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

  validateStrategySpec(spec: StrategySpec, opts?: { check_design_gate?: boolean }) {
    return request<StrategyValidationResult & {
      design_complete_checks?: Array<{ id: string, ok: boolean, code: string | null, message: string }>
      design_complete_ready?: boolean
    }>('/api/strategies/validate', {
      method: 'POST',
      body: JSON.stringify({
        spec,
        check_design_gate: opts?.check_design_gate ?? false,
      }),
    })
  },

  validateStrategy(id: number) {
    return request<StrategyValidationResult>(`/api/strategies/${id}/validate`, {
      method: 'POST',
    })
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

  async runBacktest(body: {
    strategy_id: number
    codes: string[]
    start: string
    end: string
    pool_id?: number
    costs?: BacktestCostSnapshot
  }) {
    const initial = await request<BacktestResult>('/api/backtests', {
      method: 'POST',
      body: JSON.stringify(body),
    })
    // 同步路径直接 done;异步路径 202 pending 后轮询直到完成
    if (!initial.status || initial.status === 'done' || initial.status === 'failed') {
      if (initial.status === 'failed') {
        throw new Error(initial.error || '回测失败')
      }
      return initial
    }
    const runId = initial.run_id
    const deadline = Date.now() + 10 * 60 * 1000
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 1200))
      const current = await request<BacktestResult>(`/api/backtests/${runId}`)
      if (current.status === 'done' || current.status === 'failed' || !current.status) {
        if (current.status === 'failed') {
          throw new Error(current.error || '回测失败')
        }
        return current
      }
    }
    throw new Error(`回测 #${runId} 超时仍未完成，可稍后用编号查询`)
  },

  getBacktest(runId: number) {
    return request<BacktestResult>(`/api/backtests/${runId}`)
  },

  listExperiments(includeArchived = false) {
    const params = new URLSearchParams()
    if (includeArchived) params.set('include_archived', 'true')
    const qs = params.toString()
    return request<{ count: number; items: ExperimentSummary[] }>(
      `/api/experiments${qs ? `?${qs}` : ''}`,
    )
  },

  createExperiment(body: {
    title: string
    hypothesis: string
    permanent_candidate_id: string
    spec: StrategySpec
    strategy_id?: number
    family_id?: string
    universe_snapshot?: Record<string, unknown>
    cost_snapshot?: Record<string, number>
  }) {
    return request<ExperimentSummary>('/api/experiments', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  getExperiment(id: number) {
    return request<ExperimentDetail>(`/api/experiments/${id}`)
  },

  createExperimentTrial(id: number, body: {
    codes: string[]
    start: string
    end: string
    param_patch?: Record<string, number | string | boolean>
    costs?: Record<string, number>
    pool_id?: number
    dynamic_universe?: boolean
  }) {
    return request<{
      trial: ExperimentTrial
      promotion?: EvidencePromotionEval
      backtest?: {
        run_id?: number
        metrics?: BacktestMetrics
        validation?: BacktestValidation | null
        data_quality?: BacktestDataQuality
        execution_attribution?: ExecutionAttribution
        execution_fingerprint?: string
      }
    }>(`/api/experiments/${id}/trials`, {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 批量 trial(≤32);单项失败落库不中断 */
  createExperimentTrialsBatch(id: number, body: {
    codes: string[]
    start: string
    end: string
    param_patches: Array<Record<string, number | string | boolean>>
    costs?: Record<string, number>
    pool_id?: number
    dynamic_universe?: boolean
  }) {
    return request<{
      count: number
      items: Array<{
        trial: ExperimentTrial
        error: string | null
        backtest_run_id?: number
        promotion?: EvidencePromotionEval
      }>
      pending_promotions?: EvidencePromotionTodo[]
    }>(`/api/experiments/${id}/trials/batch`, {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  listEvidencePromotions(opts?: {
    status?: string | null
    strategy_id?: number
    experiment_id?: number
  }) {
    const params = new URLSearchParams()
    if (opts?.status != null) params.set('status', opts.status)
    if (opts?.strategy_id != null) params.set('strategy_id', String(opts.strategy_id))
    if (opts?.experiment_id != null) params.set('experiment_id', String(opts.experiment_id))
    const qs = params.toString()
    return request<{ count: number, items: EvidencePromotionTodo[] }>(
      `/api/experiments/promotions${qs ? `?${qs}` : ''}`,
    )
  },

  acceptEvidencePromotion(todoId: number) {
    return request<{
      todo: EvidencePromotionTodo
      evidence_transition?: { from: string, to: string } | null
      evaluation?: EvidencePromotionEval
    }>(`/api/experiments/promotions/${todoId}/accept`, { method: 'POST' })
  },

  dismissEvidencePromotion(todoId: number) {
    return request<EvidencePromotionTodo>(
      `/api/experiments/promotions/${todoId}/dismiss`,
      { method: 'POST' },
    )
  },

  archiveExperiment(id: number) {
    return request<ExperimentSummary>(`/api/experiments/${id}/archive`, {
      method: 'POST',
    })
  },

  costSensitivity(body: {
    strategy_id: number
    codes: string[]
    start: string
    end: string
    pool_id?: number
    costs?: BacktestCostSnapshot
    slippage_multipliers?: number[]
  }) {
    return request<{
      strategy_id: number
      base_slippage: number
      results: Array<{
        slippage_multiplier: number
        slippage: number
        metrics?: Partial<BacktestMetrics>
        error?: string
        execution_attribution?: ExecutionAttribution
      }>
      disclaimer: string
    }>('/api/backtest/sensitivity', {
      method: 'POST',
      body: JSON.stringify(body),
    })
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
    param_grid?: Record<string, Array<number | boolean>>
    costs?: BacktestCostSnapshot
    /** true 时忽略 param_grid,按规格 validation.parameter_scans 声明扫描 */
    declared?: boolean
  }) {
    return request<SweepResult>('/api/backtest/sweep', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  },

  /** 证据状态手动操作:标记设计完成 / 否决复位;其余状态由回测自动推进 */
  updateStrategyEvidence(id: number, action: StrategyEvidenceAction) {
    return request<Strategy & { evidence_transition?: { from: StrategyEvidenceStatus, to: StrategyEvidenceStatus } }>(
      `/api/strategies/${id}/evidence`,
      { method: 'POST', body: JSON.stringify({ action }) },
    )
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
