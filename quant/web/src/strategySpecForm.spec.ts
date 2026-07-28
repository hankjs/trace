import { describe, expect, it } from 'vitest'
import type { StrategyAstNode, StrategySpec } from './api'
import { buildStrategySpec, defaultStrategySpecForm, parseRejectionRules, strategySpecToForm } from './strategySpecForm'

function field(name: string): StrategyAstNode {
  return { op: 'field', name }
}

function literal(value: number | boolean): StrategyAstNode {
  return { op: 'literal', value }
}

function binary(op: string, left: StrategyAstNode, right: StrategyAstNode): StrategyAstNode {
  return { op, left, right }
}

function windowed(op: string, input: StrategyAstNode, window: number, shift: number): StrategyAstNode {
  return { op, input, window, shift }
}

function indicator(op: string, input: StrategyAstNode, window: number): StrategyAstNode {
  return { op, input, window }
}

/** 与 app/strategy/presets.py 默认参数输出一致的公共字段 */
function presetBase(overrides: {
  kind: 'single' | 'portfolio'
  canonicalId: string
  book: string
  candidateId: string
  hypothesis: string
  dataFields: string[]
  maxPositions?: number
  riskOverlay?: Record<string, unknown>
}): Record<string, unknown> {
  return {
    schema_version: 1,
    kind: overrides.kind,
    metadata: {
      canonical_id: overrides.canonicalId,
      sources: [{ book: overrides.book, candidate_id: overrides.candidateId }],
      evidence_status: 'unverified',
      hypothesis: overrides.hypothesis,
    },
    universe: { pool_id: 2, exclude_st: true, min_listing_days: 60, min_amount_avg20: 0 },
    data_requirements: overrides.dataFields.map((name) => ({
      field: name, availability: 'daily_close', required: true,
    })),
    holding: {
      allow_add: false, allow_reduce: false, add_rule: null, reduce_rule: null,
      step: 0.5, max_position: 1, cooldown_days: 0, risk_reentry: 'native_reset',
    },
    overlays: {
      risk: overrides.riskOverlay ?? {
        enabled: false, type: 'fixed_pct', value: 0.08, atr_period: 14, trailing: false,
      },
      take_profit: { enabled: false, type: 'fixed_pct', value: 0.2, atr_period: 14, trailing: false },
    },
    portfolio_constraints: {
      long_only: true,
      max_positions: overrides.maxPositions ?? 500,
      max_single_weight: 1,
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
      max_entry_premium: 0,
    },
    validation: {
      baseline_ids: ['buy_and_hold', 'equal_weight'],
      locked_oos: true,
      rejection_criteria: ['no_net_oos_increment', 'unstable_parameters', 'capacity_failure'],
      parameter_scans: [],
    },
  }
}

// 以下六个规格逐字段抄自后端 presets.py 默认参数的 model_dump 输出
function maCrossSpec(): StrategySpec {
  const fast = indicator('ma', field('close'), 5)
  const slow = indicator('ma', field('close'), 20)
  return {
    ...presetBase({
      kind: 'single', canonicalId: 'CAN-TRD-01',
      book: '股市趋势技术分析', candidateId: 'TREND-08',
      hypothesis: '短期均价高于长期均价时，趋势延续概率可能高于简单持有基线。',
      dataFields: ['close'],
    }),
    entry: { condition: binary('gt', fast, slow), reason_code: 'fast_ma_above_slow' },
    positioning: { type: 'binary', target: 1 },
    native_exit: { condition: binary('lte', fast, slow), reason_code: 'fast_ma_not_above_slow' },
  } as unknown as StrategySpec
}

function breakoutSpec(): StrategySpec {
  return {
    ...presetBase({
      kind: 'single', canonicalId: 'CAN-TRD-02',
      book: '股市趋势技术分析', candidateId: 'TREND-03',
      hypothesis: '收盘突破历史区间上沿后可能延续，跌破较短退出通道表示假设失效。',
      dataFields: ['close', 'high', 'low'],
    }),
    entry: {
      condition: binary('gt', field('close'), windowed('rolling_max', field('high'), 20, 1)),
      reason_code: 'close_above_prior_high',
    },
    positioning: { type: 'binary', target: 1 },
    native_exit: {
      condition: binary('lt', field('close'), windowed('rolling_min', field('low'), 10, 1)),
      reason_code: 'close_below_prior_low',
    },
  } as unknown as StrategySpec
}

function meanReversionSpec(): StrategySpec {
  const rsi14 = indicator('rsi', field('close'), 14)
  const trend = indicator('ma', field('close'), 60)
  return {
    ...presetBase({
      kind: 'single', canonicalId: 'CAN-REV-06',
      book: '量化交易从入门到精通', candidateId: 'QTP-003',
      hypothesis: '长期趋势向上时的短期超卖可能均值修复，修复完成或趋势失效时退出。',
      dataFields: ['close'],
    }),
    entry: {
      condition: {
        op: 'all',
        args: [binary('lt', rsi14, literal(30)), binary('gt', field('close'), trend)],
      },
      reason_code: 'uptrend_oversold',
    },
    positioning: { type: 'binary', target: 1 },
    native_exit: {
      condition: {
        op: 'any',
        args: [binary('gt', rsi14, literal(55)), binary('lt', field('close'), trend)],
      },
      reason_code: 'reversion_complete_or_trend_failed',
    },
  } as unknown as StrategySpec
}

function volumeBreakoutSpec(): StrategySpec {
  const highLine = windowed('rolling_max', field('high'), 20, 1)
  const lowLine = windowed('rolling_min', field('low'), 20, 1)
  const vol5 = windowed('rolling_mean', field('volume'), 5, 1)
  const voln = windowed('rolling_mean', field('volume'), 20, 1)
  return {
    ...presetBase({
      kind: 'single', canonicalId: 'CAN-TRD-04',
      book: '量化交易从入门到精通', candidateId: 'QTP-002',
      hypothesis: '价格和成交收缩后的放量向上突破可能形成趋势，平台下沿或 ATR 风险线失效。',
      dataFields: ['close', 'high', 'low', 'volume'],
      riskOverlay: { enabled: true, type: 'atr_multiple', value: 2, atr_period: 14, trailing: true },
    }),
    entry: {
      condition: {
        op: 'all',
        args: [
          binary('lte', binary('divide', binary('subtract', highLine, lowLine), field('close')), literal(0.15)),
          binary('lt', vol5, voln),
          binary('gt', field('volume'), binary('multiply', literal(2), voln)),
          binary('gt', field('close'), highLine),
        ],
      },
      reason_code: 'contracted_volume_breakout',
    },
    positioning: { type: 'binary', target: 1 },
    native_exit: {
      condition: binary('lt', field('close'), lowLine),
      reason_code: 'close_below_platform_low',
    },
  } as unknown as StrategySpec
}

function momentumRotationSpec(): StrategySpec {
  return {
    ...presetBase({
      kind: 'portfolio', canonicalId: 'CAN-TRD-05',
      book: '股票大作手回忆录', candidateId: 'LIV-04',
      hypothesis: '横截面中短期动量较强的股票可能延续，每周轮动并用短均线控制趋势失效。',
      dataFields: ['close'],
      maxPositions: 10,
    }),
    entry: { condition: literal(true), reason_code: 'eligible_for_ranking' },
    positioning: {
      type: 'portfolio',
      score: binary('add',
        binary('multiply', literal(0.6), indicator('momentum', field('close'), 20)),
        binary('multiply', literal(0.4), indicator('momentum', field('close'), 60))),
      selection: { type: 'top_n', n: 10 },
      weighting: { type: 'equal' },
      rebalance: { frequency: 'weekly', interval_days: null },
      risk_filter: binary('lt', field('close'), indicator('ma', field('close'), 20)),
    },
    native_exit: null,
  } as unknown as StrategySpec
}

function multifactorHoldSpec(): StrategySpec {
  return {
    ...presetBase({
      kind: 'portfolio', canonicalId: 'CAN-PORT-04',
      book: '打开量化投资的黑箱', candidateId: 'BLACKBOX-ALPHA-01',
      hypothesis: '中短期动量与均线斜率的组合排序可能比单因子等权基线更稳定。',
      dataFields: ['close'],
      maxPositions: 20,
    }),
    entry: { condition: literal(true), reason_code: 'eligible_for_ranking' },
    positioning: {
      type: 'portfolio',
      score: binary('add',
        binary('add',
          binary('multiply', literal(0.5), indicator('momentum', field('close'), 20)),
          binary('multiply', literal(0.3), indicator('momentum', field('close'), 60))),
        binary('multiply', literal(0.2), indicator('return', indicator('ma', field('close'), 20), 5))),
      selection: { type: 'top_n', n: 20 },
      weighting: { type: 'equal' },
      rebalance: { frequency: 'monthly', interval_days: null },
      risk_filter: null,
    },
    native_exit: null,
  } as unknown as StrategySpec
}

const PRESET_SPECS: [string, () => StrategySpec][] = [
  ['ma_cross', maCrossSpec],
  ['breakout', breakoutSpec],
  ['mean_reversion', meanReversionSpec],
  ['volume_breakout', volumeBreakoutSpec],
  ['momentum_rotation', momentumRotationSpec],
  ['multifactor_hold', multifactorHoldSpec],
]

describe('strategySpecForm', () => {
  it.each(PRESET_SPECS)('round-trips the %s system preset without losing fields', (_, makeSpec) => {
    const spec = makeSpec()
    expect(buildStrategySpec(strategySpecToForm(spec))).toEqual(spec)
  })

  it('round-trips structured rejection rules and omits the key when empty', () => {
    // 无规则:不输出 rejection_rules 键(与后端序列化兼容约定一致,规格哈希稳定)
    const emptyForm = defaultStrategySpecForm()
    expect(buildStrategySpec(emptyForm).validation).not.toHaveProperty('rejection_rules')

    const form = defaultStrategySpecForm()
    form.sources = [{ book: '量化交易从入门到精通', candidateId: 'QTP-010' }]
    form.rejectionRulesText = JSON.stringify([{
      metric: 'annual_return', op: 'lt', threshold: 0, segment: 'oos',
      description: '样本外年化为负则否决',
    }], null, 2)

    const spec = buildStrategySpec(form)
    expect(spec.validation.rejection_rules).toEqual([{
      metric: 'annual_return', op: 'lt', threshold: 0, segment: 'oos',
      description: '样本外年化为负则否决',
    }])
    expect(strategySpecToForm(spec)).toEqual(form)
  })

  it('treats invalid rejection rules JSON as no rules and flags it', () => {
    expect(parseRejectionRules('')).toEqual([])
    expect(parseRejectionRules('not json')).toBeNull()
    expect(parseRejectionRules('{"metric":"x"}')).toBeNull()
    const form = defaultStrategySpecForm()
    form.rejectionRulesText = 'not json'
    expect(buildStrategySpec(form).validation).not.toHaveProperty('rejection_rules')
  })

  it('round-trips a fully edited form through spec and back', () => {
    const form = defaultStrategySpecForm()
    form.sources = [{ book: '量化交易从入门到精通', candidateId: 'QTP-002' }]
    form.evidenceStatus = 'backtested'
    form.cooldownDays = 3
    form.riskEnabled = true
    form.riskType = 'atr_multiple'
    form.riskValue = 2.5
    form.riskTrailing = true
    form.parameterScans = [{ path: '$.entry.condition', values: '10, 20' }]

    const restored = strategySpecToForm(buildStrategySpec(form))

    expect(restored).toEqual(form)
  })

  it('round-trips a portfolio form including monthly rebalance and risk filter', () => {
    const form = defaultStrategySpecForm()
    form.sources = [{ book: '股票大作手回忆录', candidateId: 'LIV-04' }]
    form.kind = 'portfolio'
    form.rebalance = 'monthly'
    form.riskFilterEnabled = true
    form.maxTotalWeight = 0.9

    const spec = buildStrategySpec(form)

    expect(spec.native_exit).toBeNull()
    expect(spec.positioning).toMatchObject({
      type: 'portfolio',
      rebalance: { frequency: 'monthly', interval_days: null },
    })
    expect(spec.portfolio_constraints).toMatchObject({ max_total_weight: 0.9 })
    expect(strategySpecToForm(spec)).toEqual(form)
  })

  it('round-trips holding add/reduce rules for single strategies', () => {
    const form = defaultStrategySpecForm()
    form.sources = [{ book: '量化交易从入门到精通', candidateId: 'QTP-009' }]
    form.allowAdd = true
    form.allowReduce = true
    form.addReasonCode = 'pyramid_add'
    form.reduceReasonCode = 'trail_reduce'
    form.positionStep = 0.25
    form.maxPosition = 0.75

    const spec = buildStrategySpec(form)

    expect(spec.holding).toMatchObject({
      allow_add: true,
      allow_reduce: true,
      step: 0.25,
      max_position: 0.75,
    })
    expect(spec.holding.add_rule).toMatchObject({ reason_code: 'pyramid_add' })
    expect(spec.holding.reduce_rule).toMatchObject({ reason_code: 'trail_reduce' })
    expect(strategySpecToForm(spec)).toEqual(form)
  })

  it('forces holding adjust rules off for portfolio strategies', () => {
    const form = defaultStrategySpecForm()
    form.kind = 'portfolio'
    form.allowAdd = true
    form.allowReduce = true

    const spec = buildStrategySpec(form)

    // 组合策略暂不支持加减仓:构建输出与后端校验一致的关闭形态
    expect(spec.holding).toMatchObject({
      allow_add: false,
      allow_reduce: false,
      add_rule: null,
      reduce_rule: null,
    })
    const restored = strategySpecToForm(spec)
    expect(restored.allowAdd).toBe(false)
    expect(restored.allowReduce).toBe(false)
  })

  it('uses controlled AST nodes and next-open execution', () => {
    const spec = buildStrategySpec(defaultStrategySpecForm())

    expect(spec.entry.condition.args).toHaveLength(2)
    expect(spec.native_exit?.condition.op).toBe('lt')
    expect(spec.execution).toMatchObject({
      signal_time: 'close',
      execution_time: 'next_open',
      cost_model: 'a_share_daily_v1',
    })
    const operators = JSON.stringify(spec).match(/"op":"([^"]+)"/g) ?? []
    expect(operators.join(' ')).not.toMatch(/eval|python|javascript|shell/i)
  })
})
