/**
 * design_complete 硬清单的前端镜像(与 app/strategy/evidence.py 对齐)。
 * 供策略页即时绿/红展示;权威判定仍以服务端 API 为准。
 */
import type { StrategySpec } from './api'

export interface DesignCheckItem {
  id: string
  ok: boolean
  code: string | null
  message: string
}

const KNOWN_BASELINES = new Set(['buy_and_hold', 'equal_weight'])
const KNOWN_LEGACY_REJECTION = new Set([
  'no_net_oos_increment',
  'unstable_parameters',
  'capacity_failure',
])
const HYP_PLACEHOLDERS = new Set([
  'todo', 'tbd', 'placeholder', 'n/a', 'na', 'none',
  '测试', '占位', '待补充', '假说', 'hypothesis',
])
const HYP_MIN = 20
const HYP_MAX = 1000

function item(
  id: string,
  ok: boolean,
  code: string | null,
  message: string,
): DesignCheckItem {
  return { id, ok, code: ok ? null : code, message }
}

export function evaluateDesignCompleteChecklist(
  spec: StrategySpec | null | undefined,
  capabilitySupported = true,
): DesignCheckItem[] {
  if (!spec) {
    return [item('CAPABILITY', false, 'capability_not_supported', '规格不可用')]
  }
  const metadata = (spec.metadata ?? {}) as Record<string, unknown>
  const validation = (spec.validation ?? {}) as Record<string, unknown>
  const hyp = String(metadata.hypothesis ?? '').trim()
  const checks: DesignCheckItem[] = []

  const lenOk = hyp.length >= HYP_MIN && hyp.length <= HYP_MAX
  checks.push(item(
    'HYP_LEN',
    lenOk,
    'hypothesis_too_short',
    lenOk
      ? `假说长度 ${hyp.length} 字,符合要求`
      : `假说去空白后长度 ${hyp.length},须在 ${HYP_MIN}–${HYP_MAX} 字`,
  ))

  const placeholder = HYP_PLACEHOLDERS.has(hyp.toLowerCase())
  checks.push(item(
    'HYP_PLACEHOLDER',
    !placeholder,
    'hypothesis_placeholder',
    placeholder
      ? '假说不能是占位词(todo/测试/TBD/占位等)'
      : '假说不是已知占位词',
  ))

  const baselinesRaw = validation.baseline_ids
  const baselines = Array.isArray(baselinesRaw)
    ? baselinesRaw.map((b) => String(b))
    : []
  const unknownBase = baselines.filter((b) => !KNOWN_BASELINES.has(b))
  checks.push(item(
    'BASELINE_KNOWN',
    unknownBase.length === 0 && baselines.length >= 1,
    'baseline_unknown',
    unknownBase.length
      ? `未知基线: ${unknownBase.join(', ')}`
      : baselines.length
        ? '基线均在已知集合内'
        : '至少需要 1 个已知基线',
  ))
  checks.push(item(
    'BASELINE_MIN',
    baselines.length >= 1,
    'baseline_missing',
    baselines.length >= 1
      ? `已声明 ${baselines.length} 个基线`
      : '至少需要 1 个基线',
  ))

  const criteriaRaw = validation.rejection_criteria
  const criteria = Array.isArray(criteriaRaw)
    ? criteriaRaw.map((c) => String(c).trim())
    : []
  const rulesRaw = validation.rejection_rules
  const rules = Array.isArray(rulesRaw) ? rulesRaw : []
  const emptyCriteria = criteria.some((c) => !c)
  const nonEmpty = criteria.filter(Boolean)
  const hasLegacy = nonEmpty.some((c) => KNOWN_LEGACY_REJECTION.has(c))
  const rejectNonempty = (
    criteria.length >= 1
    && !emptyCriteria
    && (hasLegacy || rules.length >= 1)
  )
  checks.push(item(
    'REJECT_NONEMPTY',
    rejectNonempty,
    'rejection_missing',
    rejectNonempty
      ? '否决条件非空且可用'
      : '否决条件去空白后存在空项,或缺少已知否决/结构化规则',
  ))

  const unknownCrit = nonEmpty.filter((c) => !KNOWN_LEGACY_REJECTION.has(c))
  const rejectKnown = unknownCrit.length === 0 && (hasLegacy || rules.length >= 1)
  checks.push(item(
    'REJECT_KNOWN',
    rejectKnown,
    'rejection_unknown',
    unknownCrit.length
      ? `未知否决条件: ${unknownCrit.join(', ')}`
      : rejectKnown
        ? '否决条件均在已知集合'
        : '缺少已知否决条件或结构化规则',
  ))

  const locked = Boolean(validation.locked_oos)
  checks.push(item(
    'LOCKED_OOS',
    locked,
    'oos_not_locked',
    locked ? '已锁定样本外' : '须将 validation.locked_oos 设为 true',
  ))

  if (spec.kind === 'single') {
    const nativeOk = spec.native_exit != null
    checks.push(item(
      'NATIVE_EXIT',
      nativeOk,
      'native_exit_missing',
      nativeOk ? '已声明原生离场' : '单标的策略必须包含 native_exit',
    ))
  } else {
    const posOk = spec.positioning != null
    checks.push(item(
      'NATIVE_EXIT',
      posOk,
      'native_exit_missing',
      posOk ? '组合 positioning 完整' : '组合策略 positioning 不完整',
    ))
  }

  checks.push(item(
    'CAPABILITY',
    capabilitySupported,
    'capability_not_supported',
    capabilitySupported
      ? '能力解析为 supported'
      : '能力状态须为 supported',
  ))

  return checks
}

export function designCompleteReady(checks: DesignCheckItem[]): boolean {
  return checks.length > 0 && checks.every((c) => c.ok)
}
