/**
 * 常用 StrategySpec 表达式片段库(未验证,仅降低配置成本)。
 * snippet_id 稳定英文;参数写入 AST 叶子;插入后仍是用户可编辑草稿。
 */
import type { StrategyAstNode } from './api'

export type SnippetTarget =
  | 'entry'
  | 'exit'
  | 'add'
  | 'reduce'
  | 'score'
  | 'risk_filter'

export type SnippetResultType = 'bool' | 'number'

export interface SnippetParamDef {
  key: string
  label: string
  type: 'int' | 'float'
  default: number
  min?: number
  max?: number
}

export interface SpecSnippet {
  id: string
  name: string
  description: string
  /** 未验证声明 */
  disclaimer: string
  resultType: SnippetResultType
  targets: SnippetTarget[]
  kind: 'single' | 'portfolio' | 'both'
  params: SnippetParamDef[]
  build: (params: Record<string, number>) => StrategyAstNode
  suggestedFields: string[]
}

const DISCLAIMER = '未验证配置辅助,不代表策略有效或可交易'

function field(name: string): StrategyAstNode {
  return { op: 'field', name }
}

function lit(value: number): StrategyAstNode {
  return { op: 'literal', value }
}

function windowed(
  op: string,
  input: StrategyAstNode,
  window: number,
  shift: number,
): StrategyAstNode {
  return { op, input, window, shift } as StrategyAstNode
}

function ma(input: StrategyAstNode, window: number): StrategyAstNode {
  return { op: 'ma', input, window }
}

function clampParams(
  defs: SnippetParamDef[],
  raw: Record<string, number>,
): Record<string, number> {
  const out: Record<string, number> = {}
  for (const def of defs) {
    let v = raw[def.key] ?? def.default
    if (typeof v !== 'number' || !Number.isFinite(v)) v = def.default
    if (def.min != null && v < def.min) v = def.min
    if (def.max != null && v > def.max) v = def.max
    if (def.type === 'int') v = Math.round(v)
    out[def.key] = v
  }
  return out
}

/** 快慢均线:若 fast>=slow 则自动交换,保证快 < 慢 */
export function orderFastSlow(fast: number, slow: number): { fast: number, slow: number } {
  if (fast < slow) return { fast, slow }
  return { fast: slow, slow: fast }
}

const breakoutParams: SnippetParamDef[] = [
  { key: 'N', label: '突破窗口', type: 'int', default: 20, min: 2, max: 500 },
]

const breakoutVolParams: SnippetParamDef[] = [
  { key: 'N', label: '突破窗口', type: 'int', default: 20, min: 2, max: 500 },
  { key: 'M', label: '量比窗口', type: 'int', default: 20, min: 2, max: 500 },
  { key: 'thr', label: '量比阈值', type: 'float', default: 1.5, min: 0.1, max: 20 },
]

const maCrossParams: SnippetParamDef[] = [
  { key: 'fast', label: '快线', type: 'int', default: 10, min: 2, max: 500 },
  { key: 'slow', label: '慢线', type: 'int', default: 60, min: 2, max: 500 },
]

const maParams: SnippetParamDef[] = [
  { key: 'N', label: '均线窗口', type: 'int', default: 20, min: 2, max: 500 },
]

const channelParams: SnippetParamDef[] = [
  { key: 'N', label: '通道窗口', type: 'int', default: 10, min: 2, max: 500 },
]

const momParams: SnippetParamDef[] = [
  { key: 'N', label: '动量窗口', type: 'int', default: 20, min: 2, max: 500 },
]

const rsiParams: SnippetParamDef[] = [
  { key: 'period', label: 'RSI 周期', type: 'int', default: 14, min: 2, max: 500 },
  { key: 'level', label: '超卖阈值', type: 'float', default: 30, min: 1, max: 50 },
]

export const SPEC_SNIPPETS: SpecSnippet[] = [
  {
    id: 'entry_breakout_n',
    name: 'N 日新高突破',
    description: 'close > rolling_max(high, N, shift=1)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['entry'],
    kind: 'both',
    params: breakoutParams,
    suggestedFields: ['close', 'high'],
    build(raw) {
      const p = clampParams(breakoutParams, raw)
      return {
        op: 'gt',
        left: field('close'),
        right: windowed('rolling_max', field('high'), p.N, 1),
      }
    },
  },
  {
    id: 'entry_breakout_vol',
    name: '突破 + 量比',
    description: '新高突破且 volume_ratio > thr',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['entry'],
    kind: 'both',
    params: breakoutVolParams,
    suggestedFields: ['close', 'high', 'volume'],
    build(raw) {
      const p = clampParams(breakoutVolParams, raw)
      return {
        op: 'all',
        args: [
          {
            op: 'gt',
            left: field('close'),
            right: windowed('rolling_max', field('high'), p.N, 1),
          },
          {
            op: 'gt',
            left: windowed('volume_ratio', field('volume'), p.M, 1),
            right: lit(p.thr),
          },
        ],
      }
    },
  },
  {
    id: 'entry_ma_cross_up',
    name: '双均线上穿',
    description: 'cross_above(ma fast, ma slow);fast≥slow 时自动交换',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['entry'],
    kind: 'both',
    params: maCrossParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(maCrossParams, raw)
      const { fast, slow } = orderFastSlow(p.fast, p.slow)
      return {
        op: 'cross_above',
        left: ma(field('close'), fast),
        right: ma(field('close'), slow),
      }
    },
  },
  {
    id: 'entry_close_above_ma',
    name: '收盘站上均线',
    description: 'close > ma(close, N)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['entry'],
    kind: 'both',
    params: maParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(maParams, raw)
      return {
        op: 'gt',
        left: field('close'),
        right: ma(field('close'), p.N),
      }
    },
  },
  {
    id: 'exit_channel_low',
    name: 'N 日低点通道离场',
    description: 'close < rolling_min(low, N, shift=1)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['exit'],
    kind: 'single',
    params: channelParams,
    suggestedFields: ['close', 'low'],
    build(raw) {
      const p = clampParams(channelParams, raw)
      return {
        op: 'lt',
        left: field('close'),
        right: windowed('rolling_min', field('low'), p.N, 1),
      }
    },
  },
  {
    id: 'exit_ma_cross_down',
    name: '双均线下穿离场',
    description: 'cross_below(ma fast, ma slow)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['exit'],
    kind: 'single',
    params: maCrossParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(maCrossParams, raw)
      const { fast, slow } = orderFastSlow(p.fast, p.slow)
      return {
        op: 'cross_below',
        left: ma(field('close'), fast),
        right: ma(field('close'), slow),
      }
    },
  },
  {
    id: 'exit_close_below_ma',
    name: '收盘跌破均线',
    description: 'close < ma(close, N)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['exit'],
    kind: 'single',
    params: maParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(maParams, raw)
      return {
        op: 'lt',
        left: field('close'),
        right: ma(field('close'), p.N),
      }
    },
  },
  {
    id: 'score_momentum_n',
    name: 'N 日动量评分',
    description: 'momentum(close, N)',
    disclaimer: DISCLAIMER,
    resultType: 'number',
    targets: ['score'],
    kind: 'portfolio',
    params: momParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(momParams, raw)
      return { op: 'momentum', input: field('close'), window: p.N }
    },
  },
  {
    id: 'filter_rsi_oversold_recover',
    name: 'RSI 超卖后恢复',
    description: 'RSI 自下上穿阈值(cross_above)',
    disclaimer: DISCLAIMER,
    resultType: 'bool',
    targets: ['entry', 'risk_filter'],
    kind: 'both',
    params: rsiParams,
    suggestedFields: ['close'],
    build(raw) {
      const p = clampParams(rsiParams, raw)
      return {
        op: 'cross_above',
        left: { op: 'rsi', input: field('close'), window: p.period },
        right: lit(p.level),
      }
    },
  },
]

export function snippetsForTarget(
  target: SnippetTarget,
  kind?: 'single' | 'portfolio',
): SpecSnippet[] {
  return SPEC_SNIPPETS.filter((s) => {
    if (!s.targets.includes(target)) return false
    if (!kind) return true
    return s.kind === 'both' || s.kind === kind
  })
}

export function getSnippet(id: string): SpecSnippet | undefined {
  return SPEC_SNIPPETS.find((s) => s.id === id)
}

export function buildSnippetAst(
  id: string,
  params: Record<string, number> = {},
): StrategyAstNode {
  const snip = getSnippet(id)
  if (!snip) throw new Error(`未知片段: ${id}`)
  return snip.build(params)
}

/** 合并 suggestedFields 到 data_requirements(required=true,不删已有) */
export function mergeSuggestedFields(
  existing: Array<{ field: string, availability: string, required: boolean }>,
  fields: string[],
): Array<{ field: string, availability: string, required: boolean }> {
  const out = existing.map((item) => ({ ...item }))
  const have = new Set(out.map((item) => item.field))
  for (const name of fields) {
    if (have.has(name)) {
      const row = out.find((item) => item.field === name)
      if (row) row.required = true
      continue
    }
    out.push({ field: name, availability: 'daily_close', required: true })
    have.add(name)
  }
  return out
}

/** 默认占位表达式(可无确认直接替换) */
export function isPlaceholderExpression(node: StrategyAstNode | null | undefined): boolean {
  if (!node || typeof node !== 'object') return true
  if (node.op === 'literal' && node.value === true) return true
  if (node.op === 'literal' && node.value === 0) return true
  if (
    node.op === 'gt'
    && (node.left as StrategyAstNode)?.op === 'field'
    && (node.left as StrategyAstNode)?.name === 'close'
    && (node.right as StrategyAstNode)?.op === 'literal'
    && (node.right as StrategyAstNode)?.value === 0
  ) {
    return true
  }
  return false
}
