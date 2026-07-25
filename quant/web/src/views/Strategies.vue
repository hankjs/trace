<script setup lang="ts">
/**
 * 策略管理:列表 + 新建 + 改名/调参 + 启停 + 另存为 + 删除。
 *
 * 策略 = 算法模板 + 一组参数 + 用户起的名字。公共策略全用户共享且只读,
 * 要调参就先「另存为我的策略」—— 与股票池的「另存为自定义池」同一套交互。
 * 已被回测引用的策略不能删,只能停用,否则历史回测会失去可追溯的策略行。
 */
import { computed, reactive, ref, watch } from 'vue'
import { Copy, Lock, Plus, Trash2 } from 'lucide-vue-next'
import {
  api,
  type CatalogEntry,
  type CatalogParameter,
  type Strategy,
  type StrategyParamValue,
} from '../api'
import { useStrategies } from '../strategies'

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

const templates = ref<CatalogEntry[]>([])
const selectedId = ref<number | null>(null)
const creating = ref(false)
const error = ref('')
const notice = ref('')
const busy = ref(false)

/** 新建表单 */
const draft = reactive({ name: '', template: '' })
const draftParams = reactive<Record<string, StrategyParamValue>>({})

/** 选中策略的编辑态,保存前不写回列表 */
const editName = ref('')
const editParams = reactive<Record<string, StrategyParamValue>>({})

const selected = computed<Strategy | null>(
  () => strategies.value.find((strategy) => strategy.id === selectedId.value) ?? null
)
const readonlyStrategy = computed(() => !!selected.value && !selected.value.editable)
/** 被回测引用时删除会 409,直接禁用按钮并说明原因 */
const usedByBacktests = computed(() => selected.value?.backtest_count ?? 0)
const quotaFull = computed(() => limits.value.max_total > 0 && customStrategies.value.length >= limits.value.max_total)

function templateOf(key: string): CatalogEntry | undefined {
  return templates.value.find((entry) => entry.key === key)
}

function paramsOf(key: string): CatalogParameter[] {
  return templateOf(key)?.params ?? []
}

/**
 * 只提交与模板默认值不同的键:后端刻意只存用户显式覆盖的参数,
 * 这样模板默认值调整后,用户没碰过的参数会跟着变。
 */
function overrides(template: string, values: Record<string, StrategyParamValue>): Record<string, StrategyParamValue> {
  const result: Record<string, StrategyParamValue> = {}
  for (const parameter of paramsOf(template)) {
    const value = values[parameter.key]
    if (value === undefined || value === '') continue
    if (parameter.default !== undefined && value === parameter.default) continue
    result[parameter.key] = value
  }
  return result
}

function fillParams(target: Record<string, StrategyParamValue>, template: string, source: Record<string, StrategyParamValue> = {}) {
  for (const key of Object.keys(target)) delete target[key]
  for (const parameter of paramsOf(template)) {
    const value = source[parameter.key] ?? parameter.default
    if (value !== undefined) target[parameter.key] = value
  }
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

/** 参数表单值:number 型走数字输入,布尔走勾选框 */
function isBooleanParam(parameter: CatalogParameter): boolean {
  return parameter.value_type === 'boolean'
}

// 也监听 templates:参数表单要有模板的参数定义才能填初值,
// 而模板元数据可能比策略列表后到
watch([selected, templates], ([strategy]) => {
  if (!strategy) return
  editName.value = strategy.name
  fillParams(editParams, strategy.template, strategy.effective_params)
})

watch(selectedId, () => {
  error.value = ''
  notice.value = ''
})

watch(strategies, (items) => {
  if (selectedId.value === null && items.length) selectedId.value = items[0].id
}, { immediate: true })

watch(() => draft.template, (template) => {
  fillParams(draftParams, template)
})

function startCreate() {
  creating.value = true
  error.value = ''
  notice.value = ''
  draft.name = ''
  draft.template = templates.value[0]?.key ?? ''
  fillParams(draftParams, draft.template)
}

async function createStrategy() {
  const name = draft.name.trim()
  if (!name) {
    error.value = '请填写策略名称'
    return
  }
  if (!draft.template) {
    error.value = '请选择算法模板'
    return
  }
  busy.value = true
  error.value = ''
  try {
    const strategy = await api.createStrategy({
      name,
      template: draft.template,
      params: overrides(draft.template, draftParams),
    })
    notice.value = `已创建「${strategy.name}」，可在回测页选用。`
    await refreshStrategies(strategy.id)
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

/** 另存为我的策略:公共策略只读,调参前先复制一份 */
async function saveAsMine() {
  const source = selected.value
  if (!source) return
  busy.value = true
  error.value = ''
  try {
    const copy = await api.duplicateStrategy(source.id, {
      // 模板元数据未就绪时不传 params,让后端沿用源策略的参数而不是重置为默认值
      ...(paramsOf(source.template).length ? { params: overrides(source.template, editParams) } : {}),
    })
    notice.value = `已另存为「${copy.name}」，可自由改名和调参。`
    await refreshStrategies(copy.id)
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

async function saveName() {
  const strategy = selected.value
  if (!strategy || readonlyStrategy.value) return
  const name = editName.value.trim()
  if (!name) {
    error.value = '请填写策略名称'
    return
  }
  busy.value = true
  error.value = ''
  try {
    await api.updateStrategy(strategy.id, { name })
    notice.value = '已保存策略名称。'
    await refreshStrategies(strategy.id)
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

async function saveParams() {
  const strategy = selected.value
  if (!strategy || readonlyStrategy.value) return
  if (!paramsOf(strategy.template).length) {
    // 模板元数据还没到时 overrides() 会算出空对象,提交上去等于把参数重置为默认值
    error.value = '算法模板元数据尚未加载完成，请稍后重试'
    return
  }
  busy.value = true
  error.value = ''
  try {
    await api.updateStrategy(strategy.id, { params: overrides(strategy.template, editParams) })
    notice.value = '已保存策略参数。历史回测保留当时的参数快照，不受影响。'
    await refreshStrategies(strategy.id)
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

async function toggleEnabled(strategy: Strategy) {
  if (!strategy.editable) return
  busy.value = true
  error.value = ''
  try {
    await api.updateStrategy(strategy.id, { enabled: !strategy.enabled })
    notice.value = strategy.enabled
      ? `已停用「${strategy.name}」，不再参与每日信号计算。`
      : `已启用「${strategy.name}」，将参与每日信号计算。`
    await refreshStrategies(strategy.id)
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

async function deleteStrategy() {
  const strategy = selected.value
  if (!strategy || readonlyStrategy.value || usedByBacktests.value > 0) return
  if (!window.confirm(`确认删除策略「${strategy.name}」？该策略的信号记录会一并删除，操作不可撤销。`)) return
  busy.value = true
  error.value = ''
  try {
    await api.deleteStrategy(strategy.id)
    notice.value = `已删除「${strategy.name}」。`
    selectedId.value = null
    await refreshStrategies()
  } catch (caught) {
    error.value = (caught as Error).message
  } finally {
    busy.value = false
  }
}

async function init() {
  try {
    const [templateResult] = await Promise.all([api.strategyTemplates(), loadStrategies(true)])
    templates.value = templateResult.items ?? []
    if (!draft.template) draft.template = templates.value[0]?.key ?? ''
  } catch (caught) {
    error.value = (caught as Error).message
  }
}

void init()
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-end justify-between gap-3">
      <div>
        <h2 class="text-base font-semibold">策略管理</h2>
        <p class="mt-0.5 text-xs text-text-tertiary">
          策略 = 算法模板 + 一组参数 + 你起的名字。公共策略由系统维护、全部用户共享，调参请先「另存为我的策略」。
        </p>
      </div>
      <p v-if="limits.max_total" class="text-xs text-text-tertiary">
        我的策略 {{ customStrategies.length }} / {{ limits.max_total }}
        · 启用 {{ enabledCount }} / {{ limits.max_enabled }}
      </p>
    </div>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="notice" class="rounded-md border border-border bg-info-soft px-4 py-2 text-sm text-text-secondary">{{ notice }}</p>

    <div class="grid gap-5 lg:grid-cols-[18rem_1fr]">
      <!-- 策略列表 + 新建入口 -->
      <section class="space-y-3" aria-labelledby="strategy-list-heading">
        <h3 id="strategy-list-heading" class="text-sm font-semibold">全部策略</h3>
        <p v-if="strategiesLoading" class="text-sm text-text-tertiary">加载中…</p>
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
                    {{ strategy.template_name }} · {{ strategy.kind_name }}
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
          class="inline-flex w-full items-center justify-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
          @click="startCreate"
        >
          <Plus :size="15" />
          新建策略
        </button>
        <p v-if="quotaFull" class="text-xs text-text-tertiary">
          策略数量已达上限 {{ limits.max_total }}，请先删除不用的策略。
        </p>
      </section>
      <!-- 新建策略:先选算法模板,再按模板的参数定义填参数 -->
      <section v-if="creating" class="space-y-4" aria-labelledby="strategy-create-heading">
        <div class="flex flex-wrap items-end justify-between gap-3 border-b border-border-subtle pb-3">
          <h3 id="strategy-create-heading" class="text-base font-semibold">新建策略</h3>
          <button
            type="button"
            class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover"
            @click="creating = false"
          >
            取消
          </button>
        </div>

        <form class="space-y-4" @submit.prevent="createStrategy">
          <div class="flex flex-wrap items-end gap-3">
            <label class="text-sm">
              <span class="mb-1 block text-xs text-text-tertiary">策略名称</span>
              <input
                v-model="draft.name"
                placeholder="名称，如 我的双均线（快5慢30）"
                class="w-64 rounded-md border border-border px-2.5 py-1.5 text-sm"
              />
            </label>
            <label class="text-sm">
              <span class="mb-1 block text-xs text-text-tertiary">算法模板</span>
              <select v-model="draft.template" class="min-w-56 rounded-md border border-border px-2.5 py-1.5 text-sm">
                <option v-for="entry in templates" :key="entry.key" :value="entry.key">{{ entry.name }}</option>
              </select>
            </label>
            <button
              type="submit"
              :disabled="busy"
              class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
            >
              创建
            </button>
          </div>

          <div v-if="templateOf(draft.template)" class="max-w-3xl text-xs leading-5 text-text-secondary">
            <p>{{ templateOf(draft.template)?.description }}</p>
            <p v-if="templateOf(draft.template)?.caveat ?? templateOf(draft.template)?.limits" class="mt-1 text-text-tertiary">
              限制：{{ templateOf(draft.template)?.caveat ?? templateOf(draft.template)?.limits }}
            </p>
            <p v-if="templateOf(draft.template)?.constraints?.length" class="mt-1 text-text-tertiary">
              参数约束：{{ templateOf(draft.template)?.constraints?.join('；') }}
            </p>
          </div>

          <div v-if="paramsOf(draft.template).length" class="rounded-md border border-border bg-surface-raised p-4">
            <span class="mb-2 block text-xs font-medium text-text-secondary">策略参数</span>
            <div class="flex flex-wrap gap-3">
              <label v-for="parameter in paramsOf(draft.template)" :key="parameter.key" class="text-sm">
                <span class="mb-1 block text-xs text-text-tertiary">
                  {{ parameter.name }}<template v-if="parameter.unit">（{{ parameter.unit }}）</template>
                </span>
                <input
                  v-if="isBooleanParam(parameter)"
                  v-model="draftParams[parameter.key]"
                  type="checkbox"
                  class="h-4 w-4 rounded border-border"
                />
                <input
                  v-else
                  v-model.number="draftParams[parameter.key]"
                  type="number"
                  :min="parameter.minimum"
                  :max="parameter.maximum"
                  :step="parameter.step ?? 'any'"
                  class="w-32 rounded-md border border-border px-2 py-1.5"
                />
                <span v-if="parameter.description" class="mt-1 block max-w-56 text-xs leading-5 text-text-tertiary">
                  {{ parameter.description }}
                </span>
              </label>
            </div>
          </div>
        </form>
      </section>

      <!-- 选中策略详情 -->
      <section v-else-if="selected" class="space-y-4" aria-labelledby="strategy-detail-heading">
        <div class="flex flex-wrap items-end justify-between gap-3 border-b border-border-subtle pb-3">
          <div>
            <h3 id="strategy-detail-heading" class="text-base font-semibold">{{ selected.name }}</h3>
            <p class="mt-0.5 text-xs text-text-tertiary">
              算法模板：{{ selected.template_name }} · {{ selected.kind_name }}
              <template v-if="selected.created_at"> · 创建于 {{ selected.created_at }}</template>
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <button
              type="button"
              :disabled="busy || quotaFull"
              class="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="saveAsMine"
            >
              <Copy :size="14" />
              另存为我的策略
            </button>
            <button
              v-if="!readonlyStrategy"
              type="button"
              :disabled="busy"
              class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="toggleEnabled(selected)"
            >
              {{ selected.enabled ? '停用' : '启用' }}
            </button>
            <button
              v-if="!readonlyStrategy"
              type="button"
              :disabled="busy || usedByBacktests > 0"
              class="inline-flex items-center gap-1.5 rounded-md border border-up/40 px-3 py-1.5 text-sm text-up hover:bg-up/5 disabled:opacity-40"
              :title="usedByBacktests > 0
                ? `已被 ${usedByBacktests} 条回测记录引用，不能删除，可改为停用`
                : '删除该策略'"
              @click="deleteStrategy"
            >
              <Trash2 :size="14" />
              删除策略
            </button>
          </div>
        </div>

        <div v-if="readonlyStrategy" class="flex items-start gap-2 rounded-md border border-border bg-info-soft px-4 py-3 text-sm leading-6 text-text-secondary">
          <Lock :size="16" class="mt-0.5 shrink-0 text-text-tertiary" />
          <span>公共策略由系统维护、全部用户共享，不可改名、调参或删除。如需自己的参数，请「另存为我的策略」。</span>
        </div>

        <p v-else-if="usedByBacktests > 0" class="rounded-md border border-border bg-warning-soft px-4 py-3 text-sm leading-6 text-text-secondary">
          该策略已被 <strong class="font-medium text-text-primary">{{ usedByBacktests }}</strong> 条回测记录引用，
          因此不能删除，只能<strong class="font-medium text-text-primary">停用</strong>。停用后不再参与每日信号计算，历史回测保持可查。
        </p>

        <p v-if="!selected.params_valid" class="rounded-md border border-up/30 bg-up/5 px-4 py-3 text-sm leading-6 text-up">
          该策略保存的参数与当前算法模板不匹配（模板参数可能已调整）。请在下方确认参数后重新保存。
        </p>

        <div v-if="templateOf(selected.template)" class="max-w-3xl text-xs leading-5 text-text-secondary">
          <p>{{ templateOf(selected.template)?.description }}</p>
          <p v-if="templateOf(selected.template)?.caveat ?? templateOf(selected.template)?.limits" class="mt-1 text-text-tertiary">
            限制：{{ templateOf(selected.template)?.caveat ?? templateOf(selected.template)?.limits }}
          </p>
        </div>

        <template v-if="!readonlyStrategy">
          <div class="flex flex-wrap items-end gap-3 rounded-md border border-border bg-surface-raised p-4">
            <label class="text-sm">
              <span class="mb-1 block text-xs text-text-tertiary">策略名称</span>
              <input v-model="editName" class="w-64 rounded-md border border-border px-2.5 py-1.5 text-sm" />
            </label>
            <button
              type="button"
              :disabled="busy"
              class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="saveName"
            >
              保存名称
            </button>
          </div>
        </template>

        <div v-if="paramsOf(selected.template).length" class="space-y-3 rounded-md border border-border bg-surface-raised p-4">
          <div class="flex items-baseline justify-between gap-3">
            <h4 class="text-sm font-semibold">策略参数</h4>
            <span v-if="templateOf(selected.template)?.constraints?.length" class="text-xs text-text-tertiary">
              约束：{{ templateOf(selected.template)?.constraints?.join('；') }}
            </span>
          </div>
          <div class="flex flex-wrap gap-3">
            <label v-for="parameter in paramsOf(selected.template)" :key="parameter.key" class="text-sm">
              <span class="mb-1 block text-xs text-text-tertiary">
                {{ parameter.name }}<template v-if="parameter.unit">（{{ parameter.unit }}）</template>
              </span>
              <input
                v-if="isBooleanParam(parameter)"
                v-model="editParams[parameter.key]"
                type="checkbox"
                :disabled="readonlyStrategy"
                class="h-4 w-4 rounded border-border disabled:opacity-50"
              />
              <input
                v-else
                v-model.number="editParams[parameter.key]"
                type="number"
                :min="parameter.minimum"
                :max="parameter.maximum"
                :step="parameter.step ?? 'any'"
                :disabled="readonlyStrategy"
                class="w-32 rounded-md border border-border px-2 py-1.5 disabled:opacity-50"
              />
              <span v-if="parameter.description" class="mt-1 block max-w-56 text-xs leading-5 text-text-tertiary">
                {{ parameter.description }}
              </span>
            </label>
          </div>
          <button
            v-if="!readonlyStrategy"
            type="button"
            :disabled="busy"
            class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
            @click="saveParams"
          >
            保存参数
          </button>
        </div>
      </section>

      <section v-else class="rounded-md border border-dashed border-border px-5 py-12 text-center text-sm text-text-tertiary">
        选择左侧策略查看详情，或新建一个自己的策略。
      </section>
    </div>
  </div>
</template>
