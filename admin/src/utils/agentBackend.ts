/**
 * 会话执行后端的展示标签。
 *
 * sessions.provider 里存的是实际执行后端：外部 CLI Agent 为 `codex` / `claude`，
 * native 路径为 provider 记录名（anthropic、openai 等）。旧会话建表时没写这个字段，
 * 回填迁移只能补 provider，model 仍为空，此时退化成只显示后端名。
 */
const BACKEND_LABELS: Record<string, string> = {
  codex: 'Codex',
  claude: 'Claude Code',
  native: '原生',
}

export function backendLabel(provider?: string | null): string {
  const key = (provider || '').trim()
  if (!key) return '未记录'
  return BACKEND_LABELS[key] || key
}

/** 外部 CLI Agent 用强调色，native provider 用中性色，未记录用最弱的一档。 */
export function backendTone(provider?: string | null): string {
  const key = (provider || '').trim()
  if (!key) return 'text-text-tertiary'
  if (key === 'codex' || key === 'claude') return 'text-accent'
  return 'text-text-secondary'
}

/** 后端 + 模型的紧凑展示，如 `Claude Code · claude-opus-4-6`。 */
export function backendSummary(provider?: string | null, model?: string | null): string {
  const label = backendLabel(provider)
  const name = (model || '').trim()
  return name ? `${label} · ${name}` : label
}
