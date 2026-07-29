<script setup lang="ts">
/** 通用递归表达式编辑器:按算子注册表渲染节点,槽位按 number/bool 类型过滤可选算子。
 * 结构性约束(节点数/深度/字段声明)只做温和提示,硬校验交给后端 validate。
 */
import { computed } from 'vue'
import { Plus, X } from 'lucide-vue-next'
import type { StrategyAstNode } from '../api'
import QuSelect from './QuSelect.vue'
import {
  MAX_EXPRESSION_DEPTH,
  MAX_EXPRESSION_NODES,
  SUPPORTED_FIELDS,
  defaultSlotNode,
  expressionStats,
  opDef,
  opsForType,
  switchNodeOp,
  type ExpressionSlot,
} from '../specExpression'

const props = withDefaults(defineProps<{
  /** 该槽位要求的求值类型:bool 槽只给布尔算子,number 槽只给数值算子 */
  expectedType: 'number' | 'bool'
  disabled?: boolean
  /** 组合策略放行 rank/top_n 横截面算子 */
  crossSectional?: boolean
  /** 递归深度,内部使用 */
  depth?: number
}>(), {
  disabled: false,
  crossSectional: false,
  depth: 0,
})

const model = defineModel<StrategyAstNode>({ required: true })

const def = computed(() => opDef(model.value.op))

// 当前算子不在候选集时(如横截面算子出现在单标的表达式)仍保留展示,避免静默改写
const options = computed(() => {
  const list = opsForType(props.expectedType, props.crossSectional)
  if (def.value && !list.some((item) => item.op === def.value!.op)) return [...list, def.value]
  return list
})

const stats = computed(() => expressionStats(model.value))
const overLimit = computed(
  () => stats.value.nodes > MAX_EXPRESSION_NODES || stats.value.depth > MAX_EXPRESSION_DEPTH,
)
const tooDeep = computed(() => props.depth >= MAX_EXPRESSION_DEPTH)

// QuSelect 选项:算子/字段/布尔常量/排序方向
const opOptions = computed(() => options.value.map((option) => ({ value: option.op, label: option.label })))
const fieldOptions = SUPPORTED_FIELDS.map((item) => ({ value: item.name, label: `${item.label} (${item.name})` }))
const boolLiteralOptions = [
  { value: true, label: '真 (true)' },
  { value: false, label: '假 (false)' },
]
const ascendingOptions = [
  { value: false, label: '降序(值大在前)' },
  { value: true, label: '升序(值小在前)' },
]

const literalNumber = computed({
  get: () => (typeof model.value.value === 'number' ? model.value.value : 0),
  set: (value: number) => {
    model.value.value = Number.isFinite(value) ? value : 0
  },
})

const literalBool = computed({
  get: () => model.value.value === true,
  set: (value: boolean) => {
    model.value.value = value
  },
})

function hasParam(name: string) {
  return (def.value?.params as string[] | undefined)?.includes(name) ?? false
}

function onOpChange(op: string) {
  model.value = switchNodeOp(model.value, op, props.expectedType)
}

function slotChildren(slot: ExpressionSlot): StrategyAstNode[] {
  return slot.list && Array.isArray(model.value.args) ? model.value.args : []
}

function addChild(slot: ExpressionSlot) {
  if (!Array.isArray(model.value.args)) model.value.args = []
  model.value.args.push(defaultSlotNode(slot.type))
}

function removeChild(slot: ExpressionSlot, index: number) {
  const children = slotChildren(slot)
  if (children.length > 1) children.splice(index, 1)
}

function ensureChild(slot: ExpressionSlot) {
  if (!model.value[slot.key]) updateSlot(slot, defaultSlotNode(slot.type))
}

function updateSlot(slot: ExpressionSlot, value: StrategyAstNode) {
  ;(model.value as unknown as Record<string, unknown>)[slot.key] = value
}

function slotChild(slot: ExpressionSlot): StrategyAstNode | null {
  if (slot.list) return null
  const child = model.value[slot.key]
  return child && !Array.isArray(child) ? child : null
}

const selectClass = 'h-7 rounded border border-border bg-surface-raised px-1.5 text-xs text-text-primary outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed disabled:opacity-55'
const numberClass = 'h-7 w-20 rounded border border-border bg-surface-raised px-1.5 text-xs text-text-primary outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed disabled:opacity-55'
</script>

<template>
  <div
    class="rounded-md border"
    :class="depth === 0 ? 'border-border bg-surface-muted/50' : 'border-border-subtle bg-surface-raised/70'"
  >
    <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-2.5 py-2">
      <QuSelect
        :model-value="model.op"
        :options="opOptions"
        :disabled="disabled"
        :class="selectClass"
        aria-label="算子"
        @change="onOpChange"
      />

      <QuSelect v-if="model.op === 'field'" v-model="model.name" :options="fieldOptions" :disabled="disabled" :class="selectClass" aria-label="字段" />

      <template v-else-if="model.op === 'literal'">
        <QuSelect v-if="expectedType === 'bool'" v-model="literalBool" :options="boolLiteralOptions" :disabled="disabled" :class="selectClass" aria-label="常量值" />
        <input v-else v-model.number="literalNumber" type="number" step="any" :disabled="disabled" :class="numberClass" aria-label="常量值" />
      </template>

      <label v-if="hasParam('window')" class="flex items-center gap-1 text-xs text-text-tertiary">
        窗口
        <input v-model.number="model.window" type="number" min="2" max="500" :disabled="disabled" :class="numberClass" />
      </label>
      <label v-if="hasParam('shift')" class="flex items-center gap-1 text-xs text-text-tertiary">
        位移
        <input v-model.number="model.shift" type="number" min="0" max="500" :disabled="disabled" :class="numberClass" />
      </label>
      <label v-if="hasParam('periods')" class="flex items-center gap-1 text-xs text-text-tertiary">
        前移日数
        <input v-model.number="model.periods" type="number" min="1" max="500" :disabled="disabled" :class="numberClass" />
      </label>
      <label v-if="hasParam('n')" class="flex items-center gap-1 text-xs text-text-tertiary">
        N
        <input v-model.number="model.n" type="number" min="1" max="500" :disabled="disabled" :class="numberClass" />
      </label>
      <label v-if="hasParam('ascending')" class="flex items-center gap-1 text-xs text-text-tertiary">
        排序
        <QuSelect v-model="model.ascending" :options="ascendingOptions" :disabled="disabled" :class="selectClass" />
      </label>

      <span v-if="!def" class="text-xs text-warning">未知算子 {{ model.op }},后端校验会拒绝</span>
      <span v-if="depth === 0" class="ml-auto text-[11px]" :class="overLimit ? 'text-warning' : 'text-text-tertiary'">
        节点 {{ stats.nodes }}/{{ MAX_EXPRESSION_NODES }} · 深度 {{ stats.depth }}/{{ MAX_EXPRESSION_DEPTH }}
      </span>
    </div>

    <p v-if="tooDeep" class="border-t border-border-subtle px-2.5 py-2 text-xs text-warning">
      嵌套已达最大深度 {{ MAX_EXPRESSION_DEPTH }},请收敛表达式结构。
    </p>

    <div v-for="slot in def?.slots ?? []" :key="slot.key" class="border-t border-border-subtle px-2.5 py-2">
      <template v-if="slot.list">
        <div v-for="(child, index) in slotChildren(slot)" :key="index" class="mb-1.5 flex items-start gap-1.5 last:mb-0">
          <SpecExpressionEditor
            :model-value="child"
            :expected-type="slot.type"
            :cross-sectional="crossSectional"
            :disabled="disabled"
            :depth="depth + 1"
            class="min-w-0 flex-1"
            @update:model-value="slotChildren(slot)[index] = $event"
          />
          <button
            type="button"
            :disabled="disabled || slotChildren(slot).length <= 1"
            class="mt-1 inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-border text-text-tertiary hover:bg-hover disabled:opacity-40"
            :aria-label="`删除${slot.label} ${index + 1}`"
            @click="removeChild(slot, index)"
          >
            <X :size="12" />
          </button>
        </div>
        <button
          type="button"
          :disabled="disabled || tooDeep"
          class="mt-1.5 inline-flex h-6 items-center gap-1 rounded border border-dashed border-border px-2 text-xs text-text-secondary hover:bg-hover disabled:opacity-40"
          @click="addChild(slot)"
        >
          <Plus :size="12" />
          添加{{ slot.label }}
        </button>
      </template>
      <template v-else>
        <div class="mb-1 text-[11px] text-text-tertiary">{{ slot.label }}</div>
        <SpecExpressionEditor
          v-if="slotChild(slot)"
          :model-value="slotChild(slot)!"
          :expected-type="slot.type"
          :cross-sectional="crossSectional"
          :disabled="disabled"
          :depth="depth + 1"
          @update:model-value="updateSlot(slot, $event)"
        />
        <button
          v-else
          type="button"
          :disabled="disabled"
          class="inline-flex h-6 items-center gap-1 rounded border border-dashed border-border px-2 text-xs text-text-secondary hover:bg-hover disabled:opacity-40"
          @click="ensureChild(slot)"
        >
          <Plus :size="12" />
          补全{{ slot.label }}
        </button>
      </template>
    </div>
  </div>
</template>
