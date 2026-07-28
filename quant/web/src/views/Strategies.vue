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
  type StrategySpec,
  type StrategyValidationResult,
} from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import StrategySpecEditor from '../components/StrategySpecEditor.vue'
import { useStrategies } from '../strategies'
import {
  buildStrategySpec,
  defaultStrategySpecForm,
  isVolumeBreakoutSpec,
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
const baseSpec = ref<StrategySpec | undefined>()
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
const draftSpec = computed(() => buildStrategySpec(form.value, baseSpec.value))
const supportsStructuredEditor = computed(() => creating.value || isVolumeBreakoutSpec(selected.value?.spec))
const previewSpec = computed(() => validation.value?.normalized_spec
  ?? (supportsStructuredEditor.value ? draftSpec.value : selected.value?.spec)
  ?? draftSpec.value)
const previewJson = computed(() => JSON.stringify(previewSpec.value, null, 2))
const capability = computed(() => validation.value?.capability ?? selected.value?.capability ?? null)

const capabilityText: Record<StrategyCapabilityStatus, string> = {
  supported: '当前数据与引擎支持',
  missing_data: '缺少所需数据',
  missing_engine: '引擎尚未支持',
  subjective_only: '仅适合作为主观研究记录',
  boundary_denied: '超出日频研究边界',
}

function kindName(strategy: Strategy) {
  return strategy.kind === 'portfolio' ? '组合策略' : '单标的策略'
}

function researchStatusName(strategy: Strategy) {
  return strategy.research_status === 'verified' ? '已验证' : strategy.research_status === 'rejected' ? '已否决' : '未验证'
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
  baseSpec.value = strategy.spec
  form.value = strategySpecToForm(strategy.spec)
  validation.value = null
  void validateSaved(strategy.id)
}, { immediate: true })

watch(strategies, (items) => {
  if (selectedId.value === null && items.length) selectedId.value = items[0].id
}, { immediate: true })

watch(selectedId, () => clear())

function startCreate() {
  creating.value = true
  selectedId.value = null
  name.value = '20 日放量突破'
  baseSpec.value = undefined
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
  const strategy = selected.value
  if (strategy && !supportsStructuredEditor.value) {
    validating.value = true
    validationError.value = ''
    try {
      const result = await api.validateStrategy(strategy.id)
      validation.value = result
      if (!result.valid) throw new Error(result.errors.join('；') || '策略规格未通过校验')
      return result
    } finally {
      validating.value = false
    }
  }
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
    if (!supportsStructuredEditor.value) {
      await api.updateStrategy(strategy.id, { name: normalizedName })
      await refreshStrategies(strategy.id)
      return
    }
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
  await router.push({ name: 'strategies', query: { tab: 'backtest', strategy: String(strategy.id) } })
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
  <div class="space-y-4">
    <div class="flex flex-wrap items-end justify-between gap-3">
      <div>
        <h2 class="text-base font-semibold">策略管理</h2>
        <p class="mt-0.5 text-xs text-text-tertiary">数据库规格是当前策略定义的唯一来源，修改后原地生效。</p>
      </div>
      <p v-if="limits.max_total" class="text-xs text-text-tertiary">
        我的策略 {{ customStrategies.length }} / {{ limits.max_total }}
        · 启用 {{ enabledCount }} / {{ limits.max_enabled }}
      </p>
    </div>

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice">{{ notice }}</InlineFeedback>

    <div class="grid gap-5 lg:grid-cols-[18rem_minmax(0,1fr)]">
      <section class="space-y-3" aria-labelledby="strategy-list-heading">
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
                  class="w-full rounded-md border px-3 py-2 text-left text-sm"
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
                    {{ kindName(strategy) }} · {{ researchStatusName(strategy) }}
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
          class="inline-flex h-9 w-full items-center justify-center gap-1.5 rounded-md bg-accent px-3 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
          @click="startCreate"
        >
          <Plus :size="15" />
          新建策略
        </button>
        <p v-if="quotaFull" class="text-xs text-text-tertiary">策略数量已达上限 {{ limits.max_total }}。</p>
      </section>

      <section v-if="creating || selected" class="min-w-0 space-y-4" aria-labelledby="strategy-detail-heading">
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
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <button
              v-if="!creating && selected"
              type="button"
              :disabled="busy || quotaFull"
              class="inline-flex h-9 items-center gap-1.5 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="saveAsMine"
            >
              <Copy :size="14" />
              另存为
            </button>
            <button
              v-if="!creating && selected"
              type="button"
              class="inline-flex h-9 items-center gap-1.5 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover"
              @click="openBacktest"
            >
              <FlaskConical :size="14" />
              回测验证
            </button>
            <button
              v-if="creating"
              type="button"
              class="h-9 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover"
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

        <InlineFeedback v-if="!supportsStructuredEditor" tone="warning">
          该策略使用当前最小编辑器尚未覆盖的受控组件。页面保持原规格只读，可校验、回测或另存，不会将其改写为突破规则。
        </InlineFeedback>

        <StrategySpecEditor
          v-if="supportsStructuredEditor"
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
              class="inline-flex h-9 items-center gap-1.5 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
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
              class="h-9 rounded-md border border-border px-3 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="selected && toggleEnabled(selected)"
            >
              {{ selected?.enabled ? '停用策略' : '启用策略' }}
            </button>
            <button
              type="button"
              :disabled="busy || usedByBacktests > 0"
              class="inline-flex h-9 items-center gap-1.5 rounded-md border border-up/40 px-3 text-sm text-up hover:bg-danger-soft disabled:opacity-40"
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
            class="h-9 rounded-md bg-accent px-4 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
            @click="creating ? createStrategy() : saveStrategy()"
          >
            {{ creating ? '校验并创建' : supportsStructuredEditor ? '校验并保存' : '保存名称' }}
          </button>
        </div>
      </section>

      <section v-else class="rounded-md border border-dashed border-border px-5 py-12 text-center text-sm text-text-tertiary">
        选择左侧策略查看详情，或新建一个自定义策略。
      </section>
    </div>
  </div>
</template>
