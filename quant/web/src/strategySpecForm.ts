/** 策略规格 <-> 表单状态的无损双向映射。
 *
 * 表单状态覆盖 StrategySpec 的全部受控字段(表达式以 StrategyAstNode 原样持有,
 * 不拆解为参数),保证六个系统模板与任意合法规格 spec→form→spec 语义不变。
 * 数值/枚举解析失败时回退默认值,硬校验交给后端 /api/strategies/validate。
 */
import type {
  StrategyAstNode,
  StrategyDataAvailability,
  StrategyDataRequirement,
  StrategyEvidenceStatus,
  StrategyOverlayType,
  StrategySpec,
} from './api'

export type { StrategyEvidenceStatus }

export interface StrategySourceForm {
  book: string
  candidateId: string
}

export interface DataRequirementForm {
  field: string
  availability: StrategyDataAvailability
  required: boolean
}

export interface ParameterScanForm {
  /** '$.a.b' 形式的参数路径 */
  path: string
  /** 逗号分隔的候选值文本,构建时解析为数字列表 */
  values: string
}

export interface StrategySpecFormState {
  kind: 'single' | 'portfolio'
  canonicalId: string
  sources: StrategySourceForm[]
  evidenceStatus: StrategyEvidenceStatus
  hypothesis: string
  poolId: number | null
  excludeSt: boolean
  minListingDays: number
  minAmountAvg20: number
  dataRequirements: DataRequirementForm[]
  entryCondition: StrategyAstNode
  entryReasonCode: string
  /** single 的原生离场;portfolio 不使用(构建时输出 null) */
  exitCondition: StrategyAstNode
  exitReasonCode: string
  /** single: binary/fixed */
  positionType: 'binary' | 'fixed'
  targetWeight: number
  /** portfolio 评分表达式(number) */
  scoreExpression: StrategyAstNode
  selectionN: number
  weightingType: 'equal' | 'rank'
  rebalance: 'fixed' | 'weekly' | 'monthly'
  rebalanceIntervalDays: number
  riskFilterEnabled: boolean
  riskFilterExpression: StrategyAstNode
  /** 加减仓(仅 single;portfolio 构建时强制关闭) */
  allowAdd: boolean
  allowReduce: boolean
  addCondition: StrategyAstNode
  addReasonCode: string
  reduceCondition: StrategyAstNode
  reduceReasonCode: string
  /** 单档仓位(占总资金比例,(0,1) 开区间) */
  positionStep: number
  /** 加仓后的总仓位上限 */
  maxPosition: number
  cooldownDays: number
  riskEnabled: boolean
  riskType: StrategyOverlayType
  riskValue: number
  riskAtrPeriod: number
  riskTrailing: boolean
  takeProfitEnabled: boolean
  takeProfitType: StrategyOverlayType
  takeProfitValue: number
  takeProfitAtrPeriod: number
  takeProfitTrailing: boolean
  maxPositions: number
  maxWeight: number
  maxTotalWeight: number
  maxEntryPremium: number
  lockedOos: boolean
  baselineIds: string
  rejectionCriteria: string
  /** 结构化否决规则的 JSON 数组文本(留空 = 无规则,构建时解析) */
  rejectionRulesText: string
  parameterScans: ParameterScanForm[]
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
}

function finiteNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback
}

function text(value: unknown, fallback: string): string {
  return typeof value === 'string' && value ? value : fallback
}

function cloneNode(value: unknown, fallback: StrategyAstNode): StrategyAstNode {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? JSON.parse(JSON.stringify(value)) as StrategyAstNode
    : JSON.parse(JSON.stringify(fallback)) as StrategyAstNode
}

const EVIDENCE_STATUSES: StrategyEvidenceStatus[] = [
  'unverified', 'design_complete', 'backtested', 'oos_passed', 'rejected',
]

const AVAILABILITIES: StrategyDataAvailability[] = ['daily_close', 'daily_open', 'point_in_time']

function field(name: string): StrategyAstNode {
  return { op: 'field', name }
}

function rolling(
  op: 'rolling_max' | 'rolling_min' | 'rolling_mean',
  name: string,
  window: number,
): StrategyAstNode {
  return { op, input: field(name), window, shift: 1 }
}

function defaultEntryCondition(): StrategyAstNode {
  return {
    op: 'all',
    args: [
      { op: 'gt', left: field('close'), right: rolling('rolling_max', 'high', 20) },
      {
        op: 'gt',
        left: { op: 'divide', left: field('volume'), right: rolling('rolling_mean', 'volume', 20) },
        right: { op: 'literal', value: 1.5 },
      },
    ],
  }
}

function defaultExitCondition(): StrategyAstNode {
  return { op: 'lt', left: field('close'), right: rolling('rolling_min', 'low', 10) }
}

function defaultScoreExpression(): StrategyAstNode {
  return { op: 'momentum', input: field('close'), window: 20 }
}

function defaultRiskFilter(): StrategyAstNode {
  return { op: 'lt', left: field('close'), right: { op: 'ma', input: field('close'), window: 20 } }
}

function defaultAddCondition(): StrategyAstNode {
  // 默认加仓规则:收盘价站上 20 日均线(趋势走强时加一档)
  return { op: 'gt', left: field('close'), right: { op: 'ma', input: field('close'), window: 20 } }
}

function defaultReduceCondition(): StrategyAstNode {
  // 默认减仓规则:收盘价跌破 10 日均线(趋势走弱时减一档)
  return { op: 'lt', left: field('close'), right: { op: 'ma', input: field('close'), window: 10 } }
}

export function defaultStrategySpecForm(): StrategySpecFormState {
  return {
    kind: 'single',
    canonicalId: 'USER-VOLUME-BREAKOUT',
    sources: [{ book: '', candidateId: '' }],
    evidenceStatus: 'unverified',
    hypothesis: '价格创阶段新高且成交量同步放大时进入，跌破短期低点时退出。',
    poolId: null,
    excludeSt: true,
    minListingDays: 120,
    minAmountAvg20: 100_000_000,
    dataRequirements: [
      { field: 'open', availability: 'daily_open', required: true },
      { field: 'high', availability: 'daily_close', required: true },
      { field: 'low', availability: 'daily_close', required: true },
      { field: 'close', availability: 'daily_close', required: true },
      { field: 'volume', availability: 'daily_close', required: true },
    ],
    entryCondition: defaultEntryCondition(),
    entryReasonCode: 'volume_breakout_entry',
    exitCondition: defaultExitCondition(),
    exitReasonCode: 'rolling_low_exit',
    positionType: 'binary',
    targetWeight: 1,
    scoreExpression: defaultScoreExpression(),
    selectionN: 10,
    weightingType: 'equal',
    rebalance: 'fixed',
    rebalanceIntervalDays: 5,
    riskFilterEnabled: false,
    riskFilterExpression: defaultRiskFilter(),
    allowAdd: false,
    allowReduce: false,
    addCondition: defaultAddCondition(),
    addReasonCode: 'add_on_strength',
    reduceCondition: defaultReduceCondition(),
    reduceReasonCode: 'reduce_on_weakness',
    positionStep: 0.5,
    maxPosition: 1,
    cooldownDays: 0,
    riskEnabled: false,
    riskType: 'fixed_pct',
    riskValue: 0.08,
    riskAtrPeriod: 14,
    riskTrailing: false,
    takeProfitEnabled: false,
    takeProfitType: 'fixed_pct',
    takeProfitValue: 0.2,
    takeProfitAtrPeriod: 14,
    takeProfitTrailing: false,
    maxPositions: 10,
    maxWeight: 0.1,
    maxTotalWeight: 1,
    maxEntryPremium: 0.03,
    lockedOos: false,
    baselineIds: 'buy_and_hold',
    rejectionCriteria: 'no_net_oos_increment, unstable_parameters',
    rejectionRulesText: '',
    parameterScans: [],
  }
}

export function strategySpecToForm(spec?: StrategySpec): StrategySpecFormState {
  const defaults = defaultStrategySpecForm()
  if (!spec) return defaults
  const metadata = record(spec.metadata)
  const sources = (Array.isArray(metadata.sources) ? metadata.sources : [])
    .map(record)
    .filter((source) => Object.keys(source).length)
    .map((source) => ({
      book: String(source.book ?? ''),
      candidateId: String(source.candidate_id ?? ''),
    }))
  const universe = record(spec.universe)
  const positioning = record(spec.positioning)
  const selection = record(positioning.selection)
  const weighting = record(positioning.weighting)
  const rebalance = record(positioning.rebalance)
  const constraints = record(spec.portfolio_constraints)
  const holding = record(spec.holding)
  const execution = record(spec.execution)
  const validation = record(spec.validation)
  const overlays = record(spec.overlays)
  const overlayOf = (key: string) => record(overlays[key])
  const risk = overlayOf('risk')
  const takeProfit = overlayOf('take_profit')
  const nativeExit = spec.native_exit ? record(spec.native_exit) : null
  const addRule = holding.add_rule ? record(holding.add_rule) : null
  const reduceRule = holding.reduce_rule ? record(holding.reduce_rule) : null
  const evidenceStatus = EVIDENCE_STATUSES.find((status) => status === metadata.evidence_status)
  const frequency = rebalance.frequency

  const dataRequirements: DataRequirementForm[] = (Array.isArray(spec.data_requirements) ? spec.data_requirements : [])
    .map(record)
    .filter((item) => typeof item.field === 'string' && item.field)
    .map((item) => ({
      field: String(item.field),
      availability: AVAILABILITIES.find((option) => option === item.availability) ?? 'daily_close',
      required: booleanValue(item.required, true),
    }))

  return {
    kind: spec.kind === 'portfolio' ? 'portfolio' : 'single',
    canonicalId: text(metadata.canonical_id, defaults.canonicalId),
    sources: sources.length ? sources : defaults.sources,
    evidenceStatus: evidenceStatus ?? 'unverified',
    hypothesis: text(metadata.hypothesis, defaults.hypothesis),
    poolId: typeof universe.pool_id === 'number' ? universe.pool_id : null,
    excludeSt: booleanValue(universe.exclude_st, defaults.excludeSt),
    minListingDays: finiteNumber(universe.min_listing_days, defaults.minListingDays),
    minAmountAvg20: finiteNumber(universe.min_amount_avg20, defaults.minAmountAvg20),
    dataRequirements: dataRequirements.length ? dataRequirements : defaults.dataRequirements,
    entryCondition: cloneNode(record(spec.entry).condition, defaults.entryCondition),
    entryReasonCode: text(record(spec.entry).reason_code, defaults.entryReasonCode),
    exitCondition: nativeExit
      ? cloneNode(nativeExit.condition, defaults.exitCondition)
      : defaults.exitCondition,
    exitReasonCode: nativeExit
      ? text(nativeExit.reason_code, defaults.exitReasonCode)
      : defaults.exitReasonCode,
    positionType: positioning.type === 'fixed' ? 'fixed' : 'binary',
    targetWeight: finiteNumber(positioning.target, defaults.targetWeight),
    scoreExpression: cloneNode(positioning.score, defaults.scoreExpression),
    selectionN: finiteNumber(selection.n, defaults.selectionN),
    weightingType: weighting.type === 'rank' ? 'rank' : 'equal',
    rebalance: frequency === 'weekly' || frequency === 'monthly' ? frequency : 'fixed',
    rebalanceIntervalDays: finiteNumber(rebalance.interval_days, defaults.rebalanceIntervalDays),
    riskFilterEnabled: positioning.risk_filter != null,
    riskFilterExpression: positioning.risk_filter != null
      ? cloneNode(positioning.risk_filter, defaults.riskFilterExpression)
      : defaults.riskFilterExpression,
    allowAdd: booleanValue(holding.allow_add, defaults.allowAdd),
    allowReduce: booleanValue(holding.allow_reduce, defaults.allowReduce),
    addCondition: addRule
      ? cloneNode(addRule.condition, defaults.addCondition)
      : defaults.addCondition,
    addReasonCode: addRule
      ? text(addRule.reason_code, defaults.addReasonCode)
      : defaults.addReasonCode,
    reduceCondition: reduceRule
      ? cloneNode(reduceRule.condition, defaults.reduceCondition)
      : defaults.reduceCondition,
    reduceReasonCode: reduceRule
      ? text(reduceRule.reason_code, defaults.reduceReasonCode)
      : defaults.reduceReasonCode,
    positionStep: finiteNumber(holding.step, defaults.positionStep),
    maxPosition: finiteNumber(holding.max_position, defaults.maxPosition),
    cooldownDays: finiteNumber(holding.cooldown_days, defaults.cooldownDays),
    riskEnabled: booleanValue(risk.enabled, defaults.riskEnabled),
    riskType: risk.type === 'atr_multiple' ? 'atr_multiple' : 'fixed_pct',
    riskValue: finiteNumber(risk.value, defaults.riskValue),
    riskAtrPeriod: finiteNumber(risk.atr_period, defaults.riskAtrPeriod),
    riskTrailing: booleanValue(risk.trailing, defaults.riskTrailing),
    takeProfitEnabled: booleanValue(takeProfit.enabled, defaults.takeProfitEnabled),
    takeProfitType: takeProfit.type === 'atr_multiple' ? 'atr_multiple' : 'fixed_pct',
    takeProfitValue: finiteNumber(takeProfit.value, defaults.takeProfitValue),
    takeProfitAtrPeriod: finiteNumber(takeProfit.atr_period, defaults.takeProfitAtrPeriod),
    takeProfitTrailing: booleanValue(takeProfit.trailing, defaults.takeProfitTrailing),
    maxPositions: finiteNumber(constraints.max_positions, defaults.maxPositions),
    maxWeight: finiteNumber(constraints.max_single_weight, defaults.maxWeight),
    maxTotalWeight: finiteNumber(constraints.max_total_weight, defaults.maxTotalWeight),
    maxEntryPremium: finiteNumber(execution.max_entry_premium, defaults.maxEntryPremium),
    lockedOos: booleanValue(validation.locked_oos, defaults.lockedOos),
    baselineIds: Array.isArray(validation.baseline_ids)
      ? validation.baseline_ids.map(String).join(', ')
      : defaults.baselineIds,
    rejectionCriteria: Array.isArray(validation.rejection_criteria)
      ? validation.rejection_criteria.map(String).join(', ')
      : defaults.rejectionCriteria,
    rejectionRulesText: Array.isArray(validation.rejection_rules) && validation.rejection_rules.length
      ? JSON.stringify(validation.rejection_rules, null, 2)
      : '',
    parameterScans: (Array.isArray(validation.parameter_scans) ? validation.parameter_scans : [])
      .map(record)
      .filter((scan) => typeof scan.path === 'string' && scan.path)
      .map((scan) => ({
        path: String(scan.path),
        values: Array.isArray(scan.values) ? scan.values.map(String).join(', ') : '',
      })),
  }
}

function splitList(value: string): string[] {
  return value.split(/[,，;；\s]+/).map((item) => item.trim()).filter(Boolean)
}

/** 解析结构化否决规则 JSON 文本;留空返回 [],非法 JSON 返回 null(由编辑器提示) */
export function parseRejectionRules(text: string): Array<Record<string, unknown>> | null {
  const trimmed = text.trim()
  if (!trimmed) return []
  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (!Array.isArray(parsed) || parsed.some((item) => !item || typeof item !== 'object' || Array.isArray(item))) {
      return null
    }
    return parsed as Array<Record<string, unknown>>
  } catch {
    return null
  }
}

function snakeCase(value: string, fallback: string): string {
  const trimmed = value.trim()
  return /^[a-z][a-z0-9_]{0,99}$/.test(trimmed) ? trimmed : fallback
}

export function buildStrategySpec(form: StrategySpecFormState): StrategySpec {
  const canonicalId = form.canonicalId.trim() || 'USER-STRATEGY'
  const sources = form.sources
    .map((source) => ({
      book: source.book.trim(),
      candidate_id: source.candidateId.trim(),
    }))
    .filter((source) => source.book || source.candidate_id)
  const normalizedSources = (sources.length ? sources : [{ book: '', candidate_id: '' }])
    .map((source) => ({
      book: source.book || '用户自定义',
      candidate_id: source.candidate_id || canonicalId,
    }))
  const baselineIds = splitList(form.baselineIds)
  const rejectionCriteria = splitList(form.rejectionCriteria)
  const rejectionRules = parseRejectionRules(form.rejectionRulesText) ?? []
  const parameterScans = form.parameterScans
    .map((scan) => ({
      path: scan.path.trim(),
      values: splitList(scan.values)
        .map(Number)
        .filter((item) => Number.isFinite(item)),
    }))
    .filter((scan) => scan.path && scan.values.length)

  const dataRequirements: StrategyDataRequirement[] = form.dataRequirements
    .filter((item) => item.field)
    .map((item) => ({
      field: item.field,
      availability: item.availability,
      required: item.required,
    }))

  return {
    schema_version: 1,
    kind: form.kind,
    metadata: {
      canonical_id: canonicalId,
      sources: normalizedSources,
      evidence_status: form.evidenceStatus,
      hypothesis: form.hypothesis.trim() || '等待补充研究假设。',
    },
    universe: {
      pool_id: form.poolId,
      exclude_st: form.excludeSt,
      min_listing_days: form.minListingDays,
      min_amount_avg20: form.minAmountAvg20,
    },
    data_requirements: dataRequirements.length
      ? dataRequirements
      : [{ field: 'close', availability: 'daily_close', required: true }],
    entry: {
      condition: form.entryCondition,
      reason_code: snakeCase(form.entryReasonCode, 'custom_entry'),
    },
    positioning: form.kind === 'single'
      ? { type: form.positionType, target: form.targetWeight }
      : {
          type: 'portfolio',
          score: form.scoreExpression,
          selection: { type: 'top_n', n: form.selectionN },
          weighting: { type: form.weightingType },
          rebalance: {
            frequency: form.rebalance,
            interval_days: form.rebalance === 'fixed' ? form.rebalanceIntervalDays : null,
          },
          risk_filter: form.riskFilterEnabled ? form.riskFilterExpression : null,
        },
    holding: {
      // 组合策略暂不支持加减仓(后端同样硬校验);单标的按开关输出规则或 null
      allow_add: form.kind === 'single' && form.allowAdd,
      allow_reduce: form.kind === 'single' && form.allowReduce,
      add_rule: form.kind === 'single' && form.allowAdd
        ? {
            condition: form.addCondition,
            reason_code: snakeCase(form.addReasonCode, 'add_on_strength'),
          }
        : null,
      reduce_rule: form.kind === 'single' && form.allowReduce
        ? {
            condition: form.reduceCondition,
            reason_code: snakeCase(form.reduceReasonCode, 'reduce_on_weakness'),
          }
        : null,
      step: form.positionStep,
      max_position: form.maxPosition,
      cooldown_days: form.cooldownDays,
      risk_reentry: 'native_reset',
    },
    native_exit: form.kind === 'single'
      ? {
          condition: form.exitCondition,
          reason_code: snakeCase(form.exitReasonCode, 'custom_exit'),
        }
      : null,
    overlays: {
      risk: {
        enabled: form.riskEnabled,
        type: form.riskType,
        value: form.riskValue,
        atr_period: form.riskAtrPeriod,
        trailing: form.riskTrailing,
      },
      take_profit: {
        enabled: form.takeProfitEnabled,
        type: form.takeProfitType,
        value: form.takeProfitValue,
        atr_period: form.takeProfitAtrPeriod,
        trailing: form.takeProfitTrailing,
      },
    },
    portfolio_constraints: {
      long_only: true,
      max_positions: form.maxPositions,
      max_single_weight: form.maxWeight,
      max_total_weight: form.maxTotalWeight,
    },
    execution: {
      signal_time: 'close',
      execution_time: 'next_open',
      buy_limit_policy: 'reject',
      sell_limit_policy: 'retry',
      suspension_policy: 'reject_entry_retry_exit',
      missing_bar_policy: 'reject_entry_retry_exit',
      cost_model: 'a_share_daily_v1',
      max_entry_premium: form.maxEntryPremium,
    },
    validation: {
      baseline_ids: baselineIds.length ? baselineIds : ['buy_and_hold'],
      locked_oos: form.lockedOos,
      rejection_criteria: rejectionCriteria.length ? rejectionCriteria : ['no_net_oos_increment'],
      // 与后端序列化兼容约定一致:无结构化规则时不输出该键,规格哈希保持稳定;
      // 非法 JSON 视为无规则,编辑器另有红字提示
      ...(rejectionRules.length ? { rejection_rules: rejectionRules } : {}),
      parameter_scans: parameterScans,
    },
  }
}

/** 默认表单对应的完整规格,供测试与新建占位使用 */
export function createBreakoutStrategySpec(): StrategySpec {
  return buildStrategySpec(defaultStrategySpecForm())
}
