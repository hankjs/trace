<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import {
  CheckCircle2,
  Code2,
  Copy,
  FlaskConical,
  Lock,
  Plus,
  RefreshCw,
  Trash2,
  TriangleAlert,
} from 'lucide-vue-next'
import {
  api,
  type Pool,
  type Strategy,
  type StrategyCapabilityStatus,
  type StrategyEvidenceAction,
  type StrategyEvidenceStatus,
  type StrategyValidationResult,
} from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import StrategySpecEditor from '../components/StrategySpecEditor.vue'
import {
  designCompleteReady,
  evaluateDesignCompleteChecklist,
  type DesignCheckItem,
} from '../designCompleteChecklist'
import { useStrategies } from '../strategies'
import {
  buildStrategySpec,
  defaultStrategySpecForm,
  strategySpecToForm,
  type StrategySpecFormState,
} from '../strategySpecForm'
import { useAsyncAction } from '../useAsyncAction'

const router = useRouter()
const {
  strategies,
  limits,
  loading: strategiesLoading,
  load: loadStrategies,
  invalidate,
  customStrategies,
  presetStrategies,
  enabledCount,
} = useStrategies()

const pools = ref<Pool[]>([])
const selectedId = ref<number | null>(null)
const creating = ref(false)
const name = ref('')
const form = ref<StrategySpecFormState>(defaultStrategySpecForm())
const validation = ref<StrategyValidationResult | null>(null)
const validating = ref(false)
const validationError = ref('')
const { busy, error, notice, clear, fail, run: runAction } = useAsyncAction()

const selected = computed<Strategy | null>(
  () => strategies.value.find((strategy) => strategy.id === selectedId.value) ?? null
)
const readonlyStrategy = computed(() => !!selected.value && !selected.value.editable)
const usedByBacktests = computed(() => selected.value?.backtest_count ?? 0)
const quotaFull = computed(() => limits.value.max_total > 0 && customStrategies.value.length >= limits.value.max_total)
const draftSpec = computed(() => buildStrategySpec(form.value))
const previewSpec = computed(() => validation.value?.normalized_spec ?? draftSpec.value)
const previewJson = computed(() => JSON.stringify(previewSpec.value, null, 2))
const capability = computed(() => validation.value?.capability ?? selected.value?.capability ?? null)
/** 与 StrategySpecEditor 的 id-prefix 保持一致,供清单点击滚动定位 */
const specIdPrefix = computed(() => (creating.value ? 'create-spec' : 'edit-spec'))

/**
 * 验证设计清单以**已保存规格**为准,与后端 apply_manual_action 一致。
 * 草稿变更不即时改清单,避免「全绿但未保存」或「已合规草稿却灰掉」的死胡同。
 */
const designChecks = computed<DesignCheckItem[]>(() => {
  const strategy = selected.value
  if (!strategy) return []
  const capOk = (strategy.capability?.status ?? 'supported') === 'supported'
  return evaluateDesignCompleteChecklist(strategy.spec, capOk)
})
const designReady = computed(() => designCompleteReady(designChecks.value))
/**
 * 草稿相对已保存是否有未提交修改。
 * 两侧都经 form 归一化再比较,避免 API 额外字段导致「永远 dirty」。
 */
const specDirty = computed(() => {
  if (creating.value || !selected.value?.spec) return false
  try {
    const savedNormalized = buildStrategySpec(strategySpecToForm(selected.value.spec))
    return JSON.stringify(draftSpec.value) !== JSON.stringify(savedNormalized)
  } catch {
    return true
  }
})
const canMarkDesign = computed(() => designReady.value && !specDirty.value)
const designBlockReason = computed(() => {
  if (specDirty.value) return '有未保存修改，请先保存后再标记'
  if (!designReady.value) {
    const first = designChecks.value.find((c) => !c.ok)
    return first ? `${first.id}: ${first.message}` : '清单未全部通过'
  }
  return ''
})
const designGateMessage = ref('')

const capabilityText: Record<StrategyCapabilityStatus, string> = {
  supported: '当前数据与引擎支持',
  missing_data: '缺少所需数据',
  missing_engine: '引擎尚未支持',
  subjective_only: '仅适合作为主观研究记录',
  boundary_denied: '超出日频研究边界',
}

const CHECK_FIELD_ANCHORS: Record<string, string> = {
  HYP_LEN: 'hypothesis',
  HYP_PLACEHOLDER: 'hypothesis',
  BASELINE_KNOWN: 'validation',
  BASELINE_MIN: 'validation',
  REJECT_NONEMPTY: 'validation',
  REJECT_KNOWN: 'validation',
  LOCKED_OOS: 'validation',
  NATIVE_EXIT: 'exit',
  CAPABILITY: 'capability',
}

function scrollToCheckField(checkId: string) {
  // CAPABILITY 落在页内「校验与能力」区块,不在 Spec 编辑器内
  if (checkId === 'CAPABILITY') {
    document.getElementById('capability-heading')?.scrollIntoView({ behavior: 'smooth', block: 'center' })
    return
  }
  const key = CHECK_FIELD_ANCHORS[checkId]
  const prefix = specIdPrefix.value
  const map: Record<string, string> = {
    hypothesis: `${prefix}-hypothesis`,
    validation: `${prefix}-validation-heading`,
    exit: `${prefix}-exit-heading`,
    data: `${prefix}-data-heading`,
  }
  const elId = key ? map[key] : undefined
  if (!elId) return
  document.getElementById(elId)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
}

function kindName(strategy: Strategy) {
  return strategy.kind === 'portfolio' ? '组合策略' : '单标的策略'
}

const EVIDENCE_STATUS_NAMES: Record<StrategyEvidenceStatus, string> = {
  unverified: '未验证',
  design_complete: '验证设计完成',
  backtested: '已回测（样本内）',
  oos_passed: '样本外否决条件通过',
  rejected: '已否决',
}

function evidenceStatusName(status?: StrategyEvidenceStatus) {
  return status ? EVIDENCE_STATUS_NAMES[status] ?? status : '未验证'
}

/** 证据状态的手动操作(标记设计完成 / 否决复位);其余状态由回测自动推进 */
async function runEvidenceAction(action: StrategyEvidenceAction) {
  const strategy = selected.value
  if (!strategy || !strategy.editable) return
  if (action === 'mark_design_complete' || action === 'reset_rejected') {
    if (specDirty.value) {
      designGateMessage.value = '有未保存修改，请先保存后再操作。'
      return
    }
    if (!designReady.value) {
      const first = designChecks.value.find((c) => !c.ok)
      designGateMessage.value = first
        ? `验证设计清单未全部通过：${first.message}`
        : '验证设计清单未全部通过,请先补全假说/基线/否决/锁定样本外等项。'
      return
    }
  }
  designGateMessage.value = ''
  await runAction(async () => {
    try {
      await api.updateStrategyEvidence(strategy.id, action)
      await refreshStrategies(strategy.id)
    } catch (caught) {
      const err = caught as Error & { detail?: { error?: string, checks?: DesignCheckItem[], message?: string } }
      if (err.detail?.error === 'design_complete_checklist_failed' && err.detail.checks) {
        const failed = err.detail.checks.filter((c) => !c.ok)
        designGateMessage.value = failed.map((c) => c.message).join('；') || err.message
        // 清单区已给出可操作明细,顶栏只给一句人话,避免刷机器码
        throw new Error('清单未通过，见下方验证设计清单')
      }
      throw caught
    }
  }, {
    success: action === 'mark_design_complete'
      ? '已标记为验证设计完成。通过清单仅表示可以开始严肃回测证据链,不代表策略有效或可交易。'
      : '已复位否决结论，状态回到验证设计完成。',
  })
}

async function refreshStrategies(selectId?: number) {
  invalidate()
  const items = await loadStrategies(true)
  if (selectId !== undefined) {
    selectedId.value = selectId
    creating.value = false
  } else if (!items.some((strategy) => strategy.id === selectedId.value)) {
    selectedId.value = items[0]?.id ?? null
  }
}

async function validateSaved(strategyId: number) {
  validating.value = true
  validationError.value = ''
  try {
    validation.value = await api.validateStrategy(strategyId)
  } catch (caught) {
    validationError.value = (caught as Error).message
  } finally {
    validating.value = false
  }
}

watch([selected, creating], ([strategy, isCreating]) => {
  if (!strategy || isCreating) return
  name.value = strategy.name
  form.value = strategySpecToForm(strategy.spec)
  validation.value = null
  void validateSaved(strategy.id)
}, { immediate: true })

watch(strategies, (items) => {
  if (selectedId.value === null && items.length) selectedId.value = items[0].id
}, { immediate: true })

watch(selectedId, () => {
  clear()
  designGateMessage.value = ''
})

function startCreate() {
  creating.value = true
  selectedId.value = null
  name.value = '20 日放量突破'
  form.value = defaultStrategySpecForm()
  form.value.poolId = pools.value[0]?.id ?? null
  validation.value = null
  validationError.value = ''
  clear()
}

async function validateDraft(): Promise<Extract<StrategyValidationResult, { valid: true }>> {
  validating.value = true
  validationError.value = ''
  try {
    const result = await api.validateStrategySpec(draftSpec.value)
    validation.value = result
    if (!result.valid) {
      const details = result.errors.length ? result.errors.join('；') : '策略规格未通过校验'
      throw new Error(details)
    }
    return result
  } finally {
    validating.value = false
  }
}

async function validateCurrent(): Promise<StrategyValidationResult> {
  return validateDraft()
}

async function createStrategy() {
  const normalizedName = name.value.trim()
  if (!normalizedName) {
    fail('请填写策略名称')
    return
  }
  await runAction(async () => {
    const result = await validateDraft()
    const strategy = await api.createStrategy({
      name: normalizedName,
      spec: result.normalized_spec,
      enabled: true,
    })
    await refreshStrategies(strategy.id)
    return strategy
  }, { success: (strategy) => `已创建「${strategy.name}」，规则可直接进入回测验证。` })
}

async function saveStrategy() {
  const strategy = selected.value
  if (!strategy || readonlyStrategy.value) return
  const normalizedName = name.value.trim()
  if (!normalizedName) {
    fail('请填写策略名称')
    return
  }
  await runAction(async () => {
    const result = await validateDraft()
    await api.updateStrategy(strategy.id, {
      name: normalizedName,
      spec: result.normalized_spec,
    })
    await refreshStrategies(strategy.id)
  }, { success: '已原地保存当前策略规格。既有回测继续保留创建时的完整规格快照。' })
}

async function saveAsMine() {
  const source = selected.value
  if (!source) return
  await runAction(async () => {
    const copy = await api.duplicateStrategy(source.id)
    await refreshStrategies(copy.id)
    return copy
  }, { success: (copy) => `已另存为「${copy.name}」，可编辑完整规则。` })
}

async function toggleEnabled(strategy: Strategy) {
  if (!strategy.editable) return
  await runAction(async () => {
    if (!strategy.enabled) await validateCurrent()
    await api.updateStrategy(strategy.id, { enabled: !strategy.enabled })
    await refreshStrategies(strategy.id)
  }, {
    success: strategy.enabled
      ? `已停用「${strategy.name}」，不再参与每日研究信号。`
      : `已启用「${strategy.name}」，将按当前规格参与每日研究信号。`,
  })
}

async function deleteStrategy() {
  const strategy = selected.value
  if (!strategy || readonlyStrategy.value || usedByBacktests.value > 0) return
  if (!window.confirm(`确认删除策略「${strategy.name}」？相关派生信号会一并删除，操作不可撤销。`)) return
  await runAction(async () => {
    await api.deleteStrategy(strategy.id)
    selectedId.value = null
    await refreshStrategies()
  }, { success: `已删除「${strategy.name}」。` })
}

async function openBacktest() {
  const strategy = selected.value
  if (!strategy) return
  await router.push({ name: 'strategies-backtest', query: { strategy: String(strategy.id) } })
}

async function init() {
  await runAction(async () => {
    const [, poolResult] = await Promise.all([loadStrategies(true), api.pools()])
    pools.value = poolResult.items ?? []
  })
}

void init()
</script>

<template>
  <div class="space-y-4 lg:flex lg:h-full lg:min-h-0 lg:flex-col lg:gap-4 lg:space-y-0">
    <div v-if="limits.max_total" class="flex flex-wrap items-end justify-end gap-3">
      <p class="text-xs text-text-tertiary">
        我的策略 {{ customStrategies.length }} / {{ limits.max_total }}
        · 启用 {{ enabledCount }} / {{ limits.max_enabled }}
      </p>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice">{{ notice }}</InlineFeedback>

    <div class="grid gap-5 lg:min-h-0 lg:flex-1 lg:grid-cols-[18rem_minmax(0,1fr)] lg:grid-rows-[minmax(0,1fr)]">
      <section data-tour="strategies-list" class="space-y-3 lg:min-h-0 lg:overflow-y-auto" aria-labelledby="strategy-list-heading">
        <h3 id="strategy-list-heading" class="text-sm font-semibold">全部策略</h3>
        <LoadingRows v-if="strategiesLoading" :rows="3" />
        <template v-else>
          <div v-for="group in [
            { title: '公共策略', items: presetStrategies },
            { title: '我的策略', items: customStrategies },
          ]" :key="group.title" class="space-y-1.5">
            <span class="block text-xs text-text-tertiary">{{ group.title }}</span>
            <ul class="space-y-1.5">
              <li v-for="strategy in group.items" :key="strategy.id">
                <button
                  type="button"
                  class="w-full rounded-md border px-3 py-2 text-left text-sm transition-colors"
                  :class="strategy.id === selectedId && !creating
                    ? 'border-accent bg-active text-text-primary'
                    : 'border-border bg-surface-raised text-text-secondary hover:bg-hover'"
                  @click="selectedId = strategy.id; creating = false"
                >
                  <span class="flex items-center gap-1.5">
                    <span class="min-w-0 flex-1 truncate font-medium">{{ strategy.name }}</span>
                    <Lock v-if="!strategy.editable" :size="13" class="shrink-0 text-text-tertiary" aria-label="公共只读" />
                  </span>
                  <span class="mt-0.5 block text-xs text-text-tertiary">
                    {{ kindName(strategy) }} · {{ evidenceStatusName(strategy.evidence_status) }}
                    <template v-if="!strategy.enabled"> · 已停用</template>
                  </span>
                </button>
              </li>
              <li v-if="!group.items.length" class="rounded-md border border-dashed border-border px-3 py-4 text-center text-xs text-text-tertiary">
                {{ group.title === '我的策略' ? '还没有自定义策略' : '暂无公共策略' }}
              </li>
            </ul>
          </div>
        </template>
        <button
          type="button"
          :disabled="busy || quotaFull"
          class="btn btn-primary w-full"
          @click="startCreate"
        >
          <Plus :size="15" />
          新建策略
        </button>
        <p v-if="quotaFull" class="text-xs text-text-tertiary">策略数量已达上限 {{ limits.max_total }}。</p>
      </section>

      <section v-if="creating || selected" data-tour="strategies-detail" class="min-w-0 space-y-4 lg:min-h-0 lg:overflow-y-auto" aria-labelledby="strategy-detail-heading">
        <div class="flex flex-wrap items-end justify-between gap-3 border-b border-border-subtle pb-3">
          <div class="min-w-0">
            <h3 id="strategy-detail-heading" class="text-base font-semibold">
              {{ creating ? '新建策略' : selected?.name }}
            </h3>
            <p class="mt-0.5 text-xs text-text-tertiary">
              <template v-if="creating">默认规则：20 日价格与成交量突破，跌破 10 日低点退出</template>
              <template v-else>
                {{ selected ? kindName(selected) : '' }}
                <template v-if="selected?.spec_hash"> · 规格 {{ selected.spec_hash.slice(0, 12) }}</template>
                <template v-if="selected?.evidence_backtest_count !== undefined">
                  · 同规格回测证据 {{ selected.evidence_backtest_count ?? 0 }} 条
                </template>
              </template>
            </p>
            <p v-if="!creating && selected" class="mt-1 flex flex-wrap items-center gap-2 text-xs">
              <span
                class="rounded px-1.5 py-0.5 font-medium"
                :class="selected.evidence_status === 'oos_passed'
                  ? 'bg-down/10 text-down'
                  : selected.evidence_status === 'rejected'
                    ? 'bg-up/10 text-up'
                    : 'bg-surface-muted text-text-secondary'"
              >证据状态:{{ evidenceStatusName(selected.evidence_status) }}</span>
              <button
                v-if="selected.editable && selected.evidence_actions?.includes('mark_design_complete')"
                type="button"
                :disabled="busy || !canMarkDesign"
                class="btn btn-secondary btn-sm"
                :title="canMarkDesign ? '标记为验证设计完成' : designBlockReason"
                @click="runEvidenceAction('mark_design_complete')"
              >标记设计完成</button>
              <button
                v-if="selected.editable && selected.evidence_actions?.includes('reset_rejected')"
                type="button"
                :disabled="busy || !canMarkDesign"
                class="btn btn-secondary btn-sm"
                :title="canMarkDesign ? '复位否决结论' : designBlockReason"
                @click="runEvidenceAction('reset_rejected')"
              >复位否决</button>
            </p>
            <p
              v-if="!creating && selected?.editable && selected.evidence_actions?.includes('mark_design_complete') && designBlockReason"
              class="mt-1 text-[11px] text-text-tertiary"
            >{{ designBlockReason }}</p>
            <div
              v-if="!creating && selected?.editable && (selected.evidence_actions?.length || designChecks.length)"
              class="mt-2 rounded border border-border-subtle bg-surface-muted px-2.5 py-2"
            >
              <div class="text-[11px] font-medium text-text-secondary">验证设计清单</div>
              <p class="mt-0.5 text-[10px] leading-4 text-text-tertiary">
                通过清单 ≠ 策略有效,仅表示可以开始严肃回测证据链。清单按<strong>已保存规格</strong>判定。
              </p>
              <p v-if="specDirty" class="mt-1 text-[11px] text-warning">
                当前有未保存修改。请先「校验并保存」后再标记设计完成。
              </p>
              <ul class="mt-1.5 space-y-0.5">
                <li
                  v-for="check in designChecks"
                  :key="check.id"
                  class="flex cursor-pointer items-start gap-1.5 text-[11px] leading-4"
                  :class="check.ok ? 'text-down' : 'text-up'"
                  @click="scrollToCheckField(check.id)"
                >
                  <span class="shrink-0 font-mono">{{ check.ok ? '✓' : '✗' }}</span>
                  <span>{{ check.id }} · {{ check.message }}</span>
                </li>
              </ul>
              <p v-if="designGateMessage" class="mt-1.5 text-[11px] text-up">{{ designGateMessage }}</p>
            </div>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <button
              v-if="!creating && selected"
              type="button"
              data-tour="strategies-save-as"
              :disabled="busy || quotaFull"
              class="btn btn-secondary"
              @click="saveAsMine"
            >
              <Copy :size="14" />
              另存为
            </button>
            <button
              v-if="!creating && selected"
              type="button"
              class="btn btn-secondary"
              @click="openBacktest"
            >
              <FlaskConical :size="14" />
              回测验证
            </button>
            <button
              v-if="creating"
              type="button"
              class="btn btn-secondary"
              @click="creating = false; selectedId = strategies[0]?.id ?? null"
            >
              取消
            </button>
          </div>
        </div>

        <div v-if="readonlyStrategy" class="flex items-start gap-2 rounded-md border border-border bg-info-soft px-4 py-3 text-sm leading-6 text-text-secondary">
          <Lock :size="16" class="mt-1 shrink-0 text-text-tertiary" />
          <span>公共策略只读。另存为自定义策略后可修改完整规格。</span>
        </div>
        <InlineFeedback v-else-if="!creating && usedByBacktests > 0" tone="warning">
          历史回测共 {{ usedByBacktests }} 条，其中 {{ selected?.evidence_backtest_count ?? 0 }} 条与当前规格哈希一致。
          修改会影响后续运行，既有回测继续使用创建时的规格快照。
        </InlineFeedback>

        <label class="block max-w-xl text-xs font-medium text-text-secondary">
          策略名称
          <input
            v-model="name"
            :disabled="readonlyStrategy"
            maxlength="64"
            class="mt-1 h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:opacity-55"
          />
        </label>

        <StrategySpecEditor
          v-model="form"
          :pools="pools"
          :disabled="readonlyStrategy"
          :id-prefix="creating ? 'create-spec' : 'edit-spec'"
        />

        <section class="rounded-md border border-border bg-surface-raised" aria-labelledby="capability-heading">
          <div class="flex flex-wrap items-center justify-between gap-3 border-b border-border-subtle px-4 py-3">
            <div class="flex items-center gap-2">
              <CheckCircle2 v-if="capability?.status === 'supported'" :size="17" class="text-down" />
              <TriangleAlert v-else :size="17" class="text-warning" />
              <div>
                <h4 id="capability-heading" class="text-sm font-semibold">校验与能力</h4>
                <p class="text-xs text-text-tertiary">
                  {{ capability ? capabilityText[capability.status] : '尚未校验当前修改' }}
                </p>
              </div>
            </div>
            <button
              type="button"
              :disabled="busy || validating"
              class="btn btn-secondary"
              @click="runAction(validateCurrent)"
            >
              <RefreshCw :size="14" :class="validating ? 'animate-spin' : ''" />
              校验当前规格
            </button>
          </div>
          <div class="p-4">
            <InlineFeedback v-if="validationError" tone="error">{{ validationError }}</InlineFeedback>
            <ul v-if="capability?.issues.length" class="space-y-2">
              <li v-for="issue in capability.issues" :key="`${issue.code}-${issue.path}`" class="rounded-md bg-surface-muted px-3 py-2 text-sm text-text-secondary">
                <span class="font-medium text-text-primary">{{ issue.message }}</span>
                <code class="ml-2 text-xs text-text-tertiary">{{ issue.path }}</code>
              </li>
            </ul>
            <p v-else-if="capability?.status === 'supported'" class="text-sm text-text-secondary">
              规格结构、数据依赖、受控操作符和日频研究边界均通过。
            </p>
            <p v-else-if="validating" class="text-sm text-text-tertiary">正在校验当前规格…</p>
            <p v-else class="text-sm text-text-tertiary">保存或启用前必须通过服务端校验。</p>
          </div>
        </section>

        <details class="rounded-md border border-border bg-surface-raised">
          <summary class="flex cursor-pointer list-none items-center gap-2 px-4 py-3 text-sm font-medium text-text-secondary hover:bg-hover">
            <Code2 :size="15" />
            规范化 JSON 只读预览
          </summary>
          <pre class="max-h-96 overflow-auto border-t border-border-subtle bg-surface-muted p-4 text-xs leading-5 text-text-secondary">{{ previewJson }}</pre>
        </details>

        <div class="flex flex-wrap items-center justify-between gap-3 border-t border-border-subtle pt-4">
          <div v-if="!creating && !readonlyStrategy" class="flex items-center gap-2">
            <button
              type="button"
              :disabled="busy"
              class="btn btn-secondary"
              @click="selected && toggleEnabled(selected)"
            >
              {{ selected?.enabled ? '停用策略' : '启用策略' }}
            </button>
            <button
              type="button"
              :disabled="busy || usedByBacktests > 0"
              class="btn btn-danger"
              :title="usedByBacktests > 0 ? '已有回测引用，不能删除，可改为停用' : '删除策略'"
              @click="deleteStrategy"
            >
              <Trash2 :size="14" />
              删除
            </button>
          </div>
          <span v-else />
          <button
            v-if="creating || !readonlyStrategy"
            type="button"
            :disabled="busy || validating"
            class="btn btn-primary"
            @click="creating ? createStrategy() : saveStrategy()"
          >
            {{ creating ? '校验并创建' : '校验并保存' }}
          </button>
        </div>
      </section>

      <section v-else class="rounded-md border border-dashed border-border px-5 py-12 text-center text-sm text-text-tertiary">
        选择左侧策略查看详情，或新建一个自定义策略。
      </section>
    </div>
  </div>
</template>
