import type { StrategyOverlayType, StrategySpec, StrategyAstNode } from './api'

export interface StrategySpecFormState {
  kind: 'single' | 'portfolio'
  canonicalId: string
  sourceBook: string
  sourceCandidateId: string
  hypothesis: string
  poolId: number | null
  excludeSt: boolean
  minListingDays: number
  minAmountAvg20: number
  breakoutWindow: number
  volumeWindow: number
  volumeRatio: number
  exitWindow: number
  positionType: 'binary' | 'equal_weight' | 'rank_weight'
  targetWeight: number
  rebalance: 'fixed' | 'weekly'
  rebalanceIntervalDays: number
  maxPositions: number
  maxWeight: number
  riskEnabled: boolean
  riskType: StrategyOverlayType
  riskValue: number
  riskAtrPeriod: number
  takeProfitEnabled: boolean
  takeProfitType: StrategyOverlayType
  takeProfitValue: number
  takeProfitAtrPeriod: number
  maxEntryPremium: number
  lockedOos: boolean
  baselineIds: string
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

function nodes(value: unknown): StrategyAstNode[] {
  return Array.isArray(value) ? value.filter((item) => item && typeof item === 'object') as StrategyAstNode[] : []
}

function walk(node: unknown): StrategyAstNode[] {
  const current = record(node) as StrategyAstNode
  if (!Object.keys(current).length) return []
  return [
    current,
    ...walk(current.left),
    ...walk(current.right),
    ...walk(current.input),
    ...walk(current.arg),
    ...nodes(current.args).flatMap(walk),
    ...nodes(current.all).flatMap(walk),
    ...nodes(current.any).flatMap(walk),
  ]
}

function rollingWindow(
  root: unknown,
  op: 'rolling_max' | 'rolling_min' | 'rolling_mean',
  field: string,
  fallback: number
): number {
  const match = walk(root).find((node) => {
    if (node.op !== op) return false
    return record(node.input).name === field
  })
  return finiteNumber(match?.window, fallback)
}

function literalValue(root: unknown, fallback: number): number {
  const literal = walk(root).find((node) => node.op === 'literal' && typeof node.value === 'number')
  return finiteNumber(literal?.value, fallback)
}

export function isVolumeBreakoutSpec(spec?: StrategySpec): boolean {
  if (!spec) return true
  const entry = walk(record(spec.entry).condition)
  const nativeExit = walk(record(spec.native_exit).condition)
  return entry.some((node) => node.op === 'rolling_max' && record(node.input).name === 'high')
    && entry.some((node) => node.op === 'rolling_mean' && record(node.input).name === 'volume')
    && nativeExit.some((node) => node.op === 'rolling_min' && record(node.input).name === 'low')
}

function overlay(root: unknown, key: string, defaults: {
  enabled: boolean
  type: StrategyOverlayType
  value: number
  atrPeriod: number
}) {
  const value = record(record(root)[key])
  return {
    enabled: booleanValue(value.enabled, defaults.enabled),
    type: value.type === 'atr_multiple' ? 'atr_multiple' as const : defaults.type,
    value: finiteNumber(value.value, defaults.value),
    atrPeriod: finiteNumber(value.atr_period, defaults.atrPeriod),
  }
}

export function createBreakoutStrategySpec(): StrategySpec {
  return buildStrategySpec(defaultStrategySpecForm())
}

export function defaultStrategySpecForm(): StrategySpecFormState {
  return {
    kind: 'single',
    canonicalId: 'USER-VOLUME-BREAKOUT',
    sourceBook: '',
    sourceCandidateId: '',
    hypothesis: '价格创阶段新高且成交量同步放大时进入，跌破短期低点时退出。',
    poolId: null,
    excludeSt: true,
    minListingDays: 120,
    minAmountAvg20: 100_000_000,
    breakoutWindow: 20,
    volumeWindow: 20,
    volumeRatio: 1.5,
    exitWindow: 10,
    positionType: 'binary',
    targetWeight: 1,
    rebalance: 'fixed',
    rebalanceIntervalDays: 5,
    maxPositions: 10,
    maxWeight: 0.1,
    riskEnabled: false,
    riskType: 'fixed_pct',
    riskValue: 0.08,
    riskAtrPeriod: 14,
    takeProfitEnabled: false,
    takeProfitType: 'fixed_pct',
    takeProfitValue: 0.2,
    takeProfitAtrPeriod: 14,
    maxEntryPremium: 0.03,
    lockedOos: false,
    baselineIds: 'buy_and_hold',
  }
}

export function strategySpecToForm(spec?: StrategySpec): StrategySpecFormState {
  const defaults = defaultStrategySpecForm()
  if (!spec) return defaults
  const metadata = record(spec.metadata)
  const sources = Array.isArray(metadata.sources) ? metadata.sources.map(record) : []
  const source = sources[0] ?? {}
  const universe = record(spec.universe)
  const positioning = record(spec.positioning)
  const constraints = record(spec.portfolio_constraints)
  const execution = record(spec.execution)
  const validation = record(spec.validation)
  const risk = overlay(spec.overlays, 'risk', {
    enabled: defaults.riskEnabled,
    type: defaults.riskType,
    value: defaults.riskValue,
    atrPeriod: defaults.riskAtrPeriod,
  })
  const takeProfit = overlay(spec.overlays, 'take_profit', {
    enabled: defaults.takeProfitEnabled,
    type: defaults.takeProfitType,
    value: defaults.takeProfitValue,
    atrPeriod: defaults.takeProfitAtrPeriod,
  })
  const weightingType = record(positioning.weighting).type
  const positionType = spec.kind === 'portfolio'
    ? weightingType === 'rank' ? 'rank_weight' : 'equal_weight'
    : 'binary'

  return {
    ...defaults,
    kind: spec.kind === 'portfolio' ? 'portfolio' : 'single',
    canonicalId: String(metadata.canonical_id ?? defaults.canonicalId),
    sourceBook: String(source.book ?? ''),
    sourceCandidateId: String(source.candidate_id ?? ''),
    hypothesis: String(metadata.hypothesis ?? defaults.hypothesis),
    poolId: typeof universe.pool_id === 'number' ? universe.pool_id : null,
    excludeSt: booleanValue(universe.exclude_st, defaults.excludeSt),
    minListingDays: finiteNumber(universe.min_listing_days, defaults.minListingDays),
    minAmountAvg20: finiteNumber(universe.min_amount_avg20, defaults.minAmountAvg20),
    breakoutWindow: rollingWindow(record(spec.entry).condition, 'rolling_max', 'high', defaults.breakoutWindow),
    volumeWindow: rollingWindow(record(spec.entry).condition, 'rolling_mean', 'volume', defaults.volumeWindow),
    volumeRatio: literalValue(record(spec.entry).condition, defaults.volumeRatio),
    exitWindow: rollingWindow(record(spec.native_exit).condition, 'rolling_min', 'low', defaults.exitWindow),
    positionType,
    targetWeight: finiteNumber(positioning.target, defaults.targetWeight),
    rebalance: record(positioning.rebalance).frequency === 'weekly' ? 'weekly' : 'fixed',
    rebalanceIntervalDays: finiteNumber(record(positioning.rebalance).interval_days, defaults.rebalanceIntervalDays),
    maxPositions: finiteNumber(record(positioning.selection).n ?? constraints.max_positions, defaults.maxPositions),
    maxWeight: finiteNumber(constraints.max_single_weight, defaults.maxWeight),
    riskEnabled: risk.enabled,
    riskType: risk.type,
    riskValue: risk.value,
    riskAtrPeriod: risk.atrPeriod,
    takeProfitEnabled: takeProfit.enabled,
    takeProfitType: takeProfit.type,
    takeProfitValue: takeProfit.value,
    takeProfitAtrPeriod: takeProfit.atrPeriod,
    maxEntryPremium: finiteNumber(execution.max_entry_premium, defaults.maxEntryPremium),
    lockedOos: booleanValue(validation.locked_oos, defaults.lockedOos),
    baselineIds: Array.isArray(validation.baseline_ids) ? validation.baseline_ids.join(', ') : '',
  }
}

export function buildStrategySpec(form: StrategySpecFormState, base?: StrategySpec): StrategySpec {
  const canonicalId = form.canonicalId.trim() || 'USER-STRATEGY'
  const sources = [{
    book: form.sourceBook.trim() || '用户自定义',
    candidate_id: form.sourceCandidateId.trim() || canonicalId,
  }]
  const field = (name: string): StrategyAstNode => ({ op: 'field', name })
  const rolling = (op: 'rolling_max' | 'rolling_min' | 'rolling_mean', name: string, window: number): StrategyAstNode => ({
    op,
    input: field(name),
    window,
    shift: 1,
  })
  const normalizedBaselineIds = form.baselineIds
    .split(/[,，\s]+/)
    .map((item) => item.trim())
    .filter(Boolean)

  return {
    ...(base ?? {}),
    schema_version: 1,
    kind: form.kind,
    metadata: {
      ...record(base?.metadata),
      canonical_id: canonicalId,
      sources,
      evidence_status: 'unverified',
      hypothesis: form.hypothesis.trim() || '等待补充研究假设。',
    },
    universe: {
      ...record(base?.universe),
      pool_id: form.poolId,
      exclude_st: form.excludeSt,
      min_listing_days: form.minListingDays,
      min_amount_avg20: form.minAmountAvg20,
    },
    data_requirements: [
      { field: 'open', availability: 'daily_open', required: true },
      { field: 'high', availability: 'daily_close', required: true },
      { field: 'low', availability: 'daily_close', required: true },
      { field: 'close', availability: 'daily_close', required: true },
      { field: 'volume', availability: 'daily_close', required: true },
    ],
    entry: {
      condition: {
        op: 'all',
        args: [
          {
            op: 'gt',
            left: field('close'),
            right: rolling('rolling_max', 'high', form.breakoutWindow),
          },
          {
            op: 'gt',
            left: {
              op: 'divide',
              left: field('volume'),
              right: rolling('rolling_mean', 'volume', form.volumeWindow),
            },
            right: { op: 'literal', value: form.volumeRatio },
          },
        ],
      },
      reason_code: 'volume_breakout_entry',
    },
    positioning: form.kind === 'single'
      ? { type: 'binary', target: form.targetWeight }
      : {
          type: 'portfolio',
          score: { op: 'momentum', input: field('close'), window: form.breakoutWindow },
          selection: { type: 'top_n', n: form.maxPositions },
          weighting: { type: form.positionType === 'rank_weight' ? 'rank' : 'equal' },
          rebalance: {
            frequency: form.rebalance,
            interval_days: form.rebalance === 'fixed' ? form.rebalanceIntervalDays : null,
          },
          risk_filter: null,
        },
    holding: {
      allow_add: false,
      allow_reduce: false,
      cooldown_days: 0,
      risk_reentry: 'native_reset',
    },
    native_exit: {
      condition: {
        op: 'any',
        args: [{
          op: 'lt',
          left: field('close'),
          right: rolling('rolling_min', 'low', form.exitWindow),
        }],
      },
      reason_code: 'rolling_low_exit',
    },
    overlays: {
      risk: {
        enabled: form.riskEnabled,
        type: form.riskType,
        value: form.riskValue,
        atr_period: form.riskAtrPeriod,
        trailing: false,
      },
      take_profit: {
        enabled: form.takeProfitEnabled,
        type: form.takeProfitType,
        value: form.takeProfitValue,
        atr_period: form.takeProfitAtrPeriod,
        trailing: false,
      },
    },
    portfolio_constraints: {
      long_only: true,
      max_positions: form.maxPositions,
      max_single_weight: form.maxWeight,
      max_total_weight: 1,
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
      ...record(base?.validation),
      baseline_ids: normalizedBaselineIds,
      locked_oos: form.lockedOos,
      rejection_criteria: ['no_net_oos_increment', 'unstable_parameters'],
      parameter_scans: [],
    },
  }
}
