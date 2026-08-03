/** 受控表达式(StrategyAstNode)的算子注册表与类型工具。
 * mirror of app/strategy/operators.py; backend contract test test_operator_registry.py enforces key parity.
 *
 * 与后端 app/strategy/spec.py 的 _OP_FIELDS / 类型系统保持一致:
 * 每个节点求值结果为 number 或 bool,子节点槽位按类型约束可选算子。
 * 前端只做温和提示与类型过滤,硬校验交给后端 /api/strategies/validate。
 */
import type { StrategyAstNode } from './api'

export type ExpressionValueType = 'number' | 'bool'

/** 后端 SUPPORTED_FIELDS,带中文展示名 */
export const SUPPORTED_FIELDS: { name: string; label: string }[] = [
  { name: 'open', label: '开盘价' },
  { name: 'high', label: '最高价' },
  { name: 'low', label: '最低价' },
  { name: 'close', label: '收盘价' },
  { name: 'raw_close', label: '原始收盘价' },
  { name: 'volume', label: '成交量' },
  { name: 'amount', label: '成交额' },
  { name: 'is_st', label: '是否 ST' },
  { name: 'pe_ttm', label: '市盈率 TTM' },
  { name: 'pb', label: '市净率' },
  { name: 'ps_ttm', label: '市销率 TTM' },
  { name: 'market_cap', label: '总市值' },
  { name: 'roe', label: 'ROE' },
  { name: 'revenue_growth', label: '营收增速' },
  { name: 'profit_growth', label: '利润增速' },
  { name: 'gross_margin', label: '毛利率' },
  { name: 'net_margin', label: '净利率' },
  { name: 'debt_ratio', label: '资产负债率' },
  { name: 'cashflow_quality', label: '现金流质量' },
]

export const MAX_EXPRESSION_DEPTH = 12
export const MAX_EXPRESSION_NODES = 256

type SlotKey = 'args' | 'arg' | 'left' | 'right' | 'input' | 'high' | 'low' | 'close'

export interface ExpressionSlot {
  key: SlotKey
  label: string
  type: ExpressionValueType
  /** true 表示 all/any 的 args 列表槽位 */
  list?: boolean
}

type ScalarParam = 'name' | 'value' | 'window' | 'shift' | 'periods' | 'n' | 'ascending'

export interface ExpressionOpDef {
  op: string
  label: string
  result: ExpressionValueType | 'any'
  slots: ExpressionSlot[]
  params: ScalarParam[]
  /** 横截面算子,仅组合策略的 score / risk_filter / 条件可用 */
  crossSectional?: boolean
  defaults?: {
    window?: number
    shift?: number
    periods?: number
    n?: number
    ascending?: boolean
  }
}

const BINARY_SLOTS: ExpressionSlot[] = [
  { key: 'left', label: '左值', type: 'number' },
  { key: 'right', label: '右值', type: 'number' },
]

const INPUT_SLOT: ExpressionSlot = { key: 'input', label: '输入', type: 'number' }

export const EXPRESSION_OPS: ExpressionOpDef[] = [
  { op: 'field', label: '数据字段', result: 'number', slots: [], params: ['name'] },
  { op: 'literal', label: '常量', result: 'any', slots: [], params: ['value'] },
  {
    op: 'all', label: '全部满足 (AND)', result: 'bool',
    slots: [{ key: 'args', label: '条件', type: 'bool', list: true }], params: [],
  },
  {
    op: 'any', label: '任一满足 (OR)', result: 'bool',
    slots: [{ key: 'args', label: '条件', type: 'bool', list: true }], params: [],
  },
  {
    op: 'not', label: '取反 (NOT)', result: 'bool',
    slots: [{ key: 'arg', label: '条件', type: 'bool' }], params: [],
  },
  { op: 'gt', label: '大于 >', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'gte', label: '大于等于 ≥', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'lt', label: '小于 <', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'lte', label: '小于等于 ≤', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'cross_above', label: '上穿', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'cross_below', label: '下穿', result: 'bool', slots: BINARY_SLOTS, params: [] },
  { op: 'add', label: '加 +', result: 'number', slots: BINARY_SLOTS, params: [] },
  { op: 'subtract', label: '减 −', result: 'number', slots: BINARY_SLOTS, params: [] },
  { op: 'multiply', label: '乘 ×', result: 'number', slots: BINARY_SLOTS, params: [] },
  { op: 'divide', label: '除 ÷', result: 'number', slots: BINARY_SLOTS, params: [] },
  {
    op: 'rolling_mean', label: '滚动均值', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'rolling_max', label: '滚动最高', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'rolling_min', label: '滚动最低', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'rolling_std', label: '滚动标准差 (ddof=0)', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'rolling_rank', label: '滚动百分位排名', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'zscore', label: '滚动 Z-Score', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 20, shift: 1 },
  },
  {
    op: 'volume_ratio', label: '量比', result: 'number',
    slots: [INPUT_SLOT], params: ['window', 'shift'], defaults: { window: 5, shift: 1 },
  },
  {
    op: 'shift', label: '前移 N 日', result: 'number',
    slots: [INPUT_SLOT], params: ['periods'], defaults: { periods: 1 },
  },
  {
    op: 'ma', label: '均线 MA', result: 'number',
    slots: [INPUT_SLOT], params: ['window'], defaults: { window: 20 },
  },
  {
    op: 'rsi', label: 'RSI', result: 'number',
    slots: [INPUT_SLOT], params: ['window'], defaults: { window: 14 },
  },
  {
    op: 'momentum', label: '动量', result: 'number',
    slots: [INPUT_SLOT], params: ['window'], defaults: { window: 20 },
  },
  {
    op: 'return', label: '区间收益率', result: 'number',
    slots: [INPUT_SLOT], params: ['window'], defaults: { window: 5 },
  },
  {
    op: 'atr', label: 'ATR', result: 'number',
    slots: [
      { key: 'high', label: '最高价序列', type: 'number' },
      { key: 'low', label: '最低价序列', type: 'number' },
      { key: 'close', label: '收盘价序列', type: 'number' },
    ],
    params: ['window'], defaults: { window: 14 },
  },
  {
    op: 'rank', label: '横截面排名', result: 'number', crossSectional: true,
    slots: [INPUT_SLOT], params: ['ascending'], defaults: { ascending: false },
  },
  {
    op: 'top_n', label: '横截面 Top N', result: 'bool', crossSectional: true,
    slots: [INPUT_SLOT], params: ['n'], defaults: { n: 10 },
  },
  {
    op: 'cs_rank', label: '截面分位', result: 'number', crossSectional: true,
    slots: [INPUT_SLOT], params: ['group_by'], defaults: { group_by: null },
  },
  {
    op: 'cs_zscore', label: '截面标准化', result: 'number', crossSectional: true,
    slots: [INPUT_SLOT], params: ['group_by'], defaults: { group_by: null },
  },
  {
    op: 'cs_demean', label: '截面去均值', result: 'number', crossSectional: true,
    slots: [INPUT_SLOT], params: ['group_by'], defaults: { group_by: null },
  },
]

const OP_INDEX = new Map(EXPRESSION_OPS.map((def) => [def.op, def]))

export function opDef(op: string | undefined): ExpressionOpDef | undefined {
  return op ? OP_INDEX.get(op) : undefined
}

/** 槽位可用的算子:结果类型匹配(literal 两种槽位都可用),横截面算子按需放行 */
export function opsForType(type: ExpressionValueType, crossSectional: boolean): ExpressionOpDef[] {
  return EXPRESSION_OPS.filter((def) => {
    if (def.crossSectional && !crossSectional) return false
    return def.result === type || def.result === 'any'
  })
}

/** 节点的求值类型;literal 取决于值,未知 op 按 number 兜底 */
export function nodeType(node: StrategyAstNode | null | undefined): ExpressionValueType {
  if (!node) return 'number'
  if (node.op === 'literal') return typeof node.value === 'boolean' ? 'bool' : 'number'
  const result = opDef(node.op)?.result
  if (result === 'bool') return 'bool'
  return 'number'
}

function fieldNode(name: string): StrategyAstNode {
  return { op: 'field', name }
}

/** 动态槽位写入:SlotKey 联合类型下直接赋值会被收窄为 never,这里集中绕行 */
function setChild(node: StrategyAstNode, key: SlotKey, child: StrategyAstNode) {
  ;(node as unknown as Record<string, unknown>)[key] = child
}

/** 槽位的默认子节点:bool 槽给常量 true,number 槽给收盘价字段 */
export function defaultSlotNode(type: ExpressionValueType): StrategyAstNode {
  return type === 'bool' ? { op: 'literal', value: true } : fieldNode('close')
}

/** 构造某个算子的默认形状节点,字段与后端 _OP_FIELDS 精确对齐 */
export function defaultNode(op: string, slotType: ExpressionValueType): StrategyAstNode {
  const def = opDef(op)
  if (!def) return defaultSlotNode(slotType)
  const node: StrategyAstNode = { op }
  if (op === 'field') {
    node.name = 'close'
    return node
  }
  if (op === 'literal') {
    node.value = slotType === 'bool' ? true : 0
    return node
  }
  for (const slot of def.slots) {
    if (slot.list) {
      node.args = [defaultSlotNode(slot.type)]
    } else if (op === 'atr') {
      // ATR 三输入节点默认接对应价格字段
      setChild(node, slot.key, fieldNode(slot.key))
    } else {
      setChild(node, slot.key, defaultSlotNode(slot.type))
    }
  }
  if (def.defaults?.window !== undefined) node.window = def.defaults.window
  if (def.defaults?.shift !== undefined) node.shift = def.defaults.shift
  if (def.defaults?.periods !== undefined) node.periods = def.defaults.periods
  if (def.defaults?.n !== undefined) node.n = def.defaults.n
  if (def.defaults?.ascending !== undefined) node.ascending = def.defaults.ascending
  return node
}

/**
 * 切换算子时尽量保留兼容内容:同名槽位类型一致的子节点、
 * 以及新旧算子共有的标量参数(window/shift/periods/n/ascending/name/value)。
 */
export function switchNodeOp(
  previous: StrategyAstNode,
  op: string,
  slotType: ExpressionValueType,
): StrategyAstNode {
  const next = defaultNode(op, slotType)
  const def = opDef(op)
  if (!def) return next
  for (const slot of def.slots) {
    if (slot.list) {
      if (Array.isArray(previous.args) && previous.args.length) {
        const carried = previous.args.filter((child) => nodeType(child) === slot.type)
        if (carried.length) next.args = carried
      }
      continue
    }
    const child = previous[slot.key]
    if (child && !Array.isArray(child) && nodeType(child) === slot.type) setChild(next, slot.key, child)
  }
  const scalarKeys = ['window', 'shift', 'periods', 'n', 'ascending', 'name'] as const
  for (const key of scalarKeys) {
    if (next[key] !== undefined && previous[key] !== undefined) {
      ;(next as unknown as Record<string, unknown>)[key] = previous[key]
    }
  }
  if (next.op === 'literal' && previous.op === 'literal' && previous.value !== undefined) {
    const keepBool = typeof previous.value === 'boolean'
    if ((slotType === 'bool') === keepBool) next.value = previous.value
  }
  return next
}

/** 统计表达式节点数与深度,用于 UI 层的温和提示 */
export function expressionStats(node: StrategyAstNode | null | undefined): { nodes: number; depth: number } {
  if (!node || typeof node !== 'object') return { nodes: 0, depth: 0 }
  const children: (StrategyAstNode | undefined)[] = [
    node.arg, node.left, node.right, node.input, node.high, node.low, node.close,
    ...(Array.isArray(node.args) ? node.args : []),
  ]
  let nodes = 1
  let depth = 1
  for (const child of children) {
    if (!child) continue
    const stats = expressionStats(child)
    nodes += stats.nodes
    depth = Math.max(depth, 1 + stats.depth)
  }
  return { nodes, depth }
}

function collectFields(node: StrategyAstNode | null | undefined, into: Set<string>) {
  if (!node || typeof node !== 'object') return
  if (node.op === 'field' && typeof node.name === 'string') into.add(node.name)
  for (const child of [node.arg, node.left, node.right, node.input, node.high, node.low, node.close]) {
    collectFields(child, into)
  }
  if (Array.isArray(node.args)) {
    for (const child of node.args) collectFields(child, into)
  }
}

/** 收集一组表达式用到的全部 field 名 */
export function usedFields(nodes: (StrategyAstNode | null | undefined)[]): Set<string> {
  const into = new Set<string>()
  for (const node of nodes) collectFields(node, into)
  return into
}
