/**
 * 实验 trial 对比表的纯函数:排序、最优高亮、参数列展开、摘要卡。
 */
export type TrialOutcome = 'ok' | 'no_trades' | 'error' | 'rejected' | string

export interface CompareTrial {
  id: number
  trial_index: number
  outcome: TrialOutcome
  param_patch?: Record<string, unknown> | null
  metrics_summary?: Record<string, number | null | undefined> | null
  backtest_run_id?: number | null
  error?: string | null
}

export type ObjectiveKey = 'sharpe' | 'annual_return' | 'total_return' | 'calmar' | 'max_drawdown'

export type SortDir = 'asc' | 'desc'

export interface SortState {
  key: ObjectiveKey | 'trial_index' | 'outcome'
  dir: SortDir
}

function metricValue(trial: CompareTrial, key: ObjectiveKey): number | null {
  const m = trial.metrics_summary || {}
  if (key === 'calmar') {
    const ann = m.annual_return
    const dd = m.max_drawdown
    if (ann == null || dd == null || !Number.isFinite(ann) || !Number.isFinite(dd)) return null
    const denom = Math.abs(dd)
    if (denom === 0) return null
    return ann / denom
  }
  const v = m[key]
  if (v == null || !Number.isFinite(v as number)) return null
  return v as number
}

/** 空指标沉底;稳定次序次键 trial_index */
export function sortTrials(trials: CompareTrial[], sort: SortState): CompareTrial[] {
  const dir = sort.dir === 'asc' ? 1 : -1
  return [...trials].sort((a, b) => {
    if (sort.key === 'trial_index') {
      return (a.trial_index - b.trial_index) * dir
    }
    if (sort.key === 'outcome') {
      const c = String(a.outcome).localeCompare(String(b.outcome))
      if (c !== 0) return c * dir
      return a.trial_index - b.trial_index
    }
    const va = metricValue(a, sort.key)
    const vb = metricValue(b, sort.key)
    if (va == null && vb == null) return a.trial_index - b.trial_index
    if (va == null) return 1
    if (vb == null) return -1
    if (va !== vb) return (va < vb ? -1 : 1) * dir
    return a.trial_index - b.trial_index
  })
}

/** 仅 outcome=ok 且目标指标非空参与最优;数值越大越好(含 max_drawdown 负值越接近 0) */
export function pickBestTrial(
  trials: CompareTrial[],
  objective: ObjectiveKey = 'sharpe',
): CompareTrial | null {
  const candidates = trials.filter((t) => t.outcome === 'ok' && metricValue(t, objective) != null)
  if (!candidates.length) return null
  let best = candidates[0]
  let bestVal = metricValue(best, objective) as number
  for (const t of candidates.slice(1)) {
    const v = metricValue(t, objective) as number
    if (v > bestVal) {
      best = t
      bestVal = v
    }
  }
  return best
}

/** 人类可读 param_patch: path=value 多行 */
export function formatParamPatch(patch: Record<string, unknown> | null | undefined): string {
  if (!patch || !Object.keys(patch).length) return '(基准)'
  return Object.entries(patch)
    .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
    .join('\n')
}

/** 键集合为 1～2 时拆独立列;>2 保持摘要 */
export function expandParamColumns(trials: CompareTrial[]): {
  mode: 'columns' | 'summary'
  keys: string[]
} {
  const keySet = new Set<string>()
  for (const t of trials) {
    for (const k of Object.keys(t.param_patch || {})) keySet.add(k)
  }
  const keys = [...keySet].sort()
  if (keys.length >= 1 && keys.length <= 2) {
    return { mode: 'columns', keys }
  }
  return { mode: 'summary', keys }
}

export interface TrialSummaryCard {
  total: number
  ok: number
  no_trades: number
  error: number
  rejected: number
  best_trial_index: number | null
  best_value: number | null
  best_param_patch: Record<string, unknown> | null
  min: number | null
  median: number | null
  max: number | null
}

function median(values: number[]): number | null {
  if (!values.length) return null
  const s = [...values].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2
}

export function summarizeTrials(
  trials: CompareTrial[],
  objective: ObjectiveKey = 'sharpe',
): TrialSummaryCard {
  const counts = { ok: 0, no_trades: 0, error: 0, rejected: 0 }
  for (const t of trials) {
    if (t.outcome === 'ok') counts.ok += 1
    else if (t.outcome === 'no_trades') counts.no_trades += 1
    else if (t.outcome === 'error') counts.error += 1
    else if (t.outcome === 'rejected') counts.rejected += 1
  }
  const best = pickBestTrial(trials, objective)
  const okVals = trials
    .filter((t) => t.outcome === 'ok')
    .map((t) => metricValue(t, objective))
    .filter((v): v is number => v != null)
  return {
    total: trials.length,
    ...counts,
    best_trial_index: best?.trial_index ?? null,
    best_value: best ? metricValue(best, objective) : null,
    best_param_patch: best?.param_patch ? { ...best.param_patch } : best ? {} : null,
    min: okVals.length >= 3 ? Math.min(...okVals) : null,
    median: okVals.length >= 3 ? median(okVals) : null,
    max: okVals.length >= 3 ? Math.max(...okVals) : null,
  }
}

export function objectiveValue(trial: CompareTrial, key: ObjectiveKey): number | null {
  return metricValue(trial, key)
}
