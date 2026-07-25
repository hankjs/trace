<script setup lang="ts">
/**
 * 统一策略选择器。取代各页面按算法 key 硬编码的策略下拉。
 *
 * v-model 绑 strategy_id(number | null)。null 表示尚未选定,
 * 列表加载完成后会自动落到默认策略(公共策略里 id 最小的那条)。
 */
import { computed, onMounted, watch } from 'vue'
import { Settings2 } from 'lucide-vue-next'
import type { Strategy, StrategyKind } from '../api'
import { defaultStrategyId, useStrategies } from '../strategies'

const props = withDefaults(defineProps<{
  label?: string
  /** 只列出该类型的策略;不传则全列 */
  kind?: StrategyKind | null
  /** 是否展示「管理策略」入口 */
  manageLink?: boolean
  /** 是否允许「全部策略」空选项(筛选场景用) */
  allowEmpty?: boolean
  emptyLabel?: string
  disabled?: boolean
}>(), {
  label: '策略',
  kind: null,
  manageLink: true,
  allowEmpty: false,
  emptyLabel: '全部策略',
  disabled: false,
})

const model = defineModel<number | null>({ required: true })
const { strategies, loading, error, load } = useStrategies()

const emit = defineEmits<{ change: [strategyId: number | null] }>()

const options = computed<Strategy[]>(() =>
  props.kind ? strategies.value.filter((strategy) => strategy.kind === props.kind) : [...strategies.value]
)

const selected = computed<Strategy | null>(() =>
  options.value.find((strategy) => strategy.id === model.value) ?? null
)

/** 自定义策略加后缀区分,与股票池下拉同口径;停用的策略同样标出来 */
function optionLabel(strategy: Strategy): string {
  const tags = [
    strategy.is_system ? '公共' : '自定义',
    ...(strategy.enabled ? [] : ['已停用']),
  ]
  return `${strategy.name}（${tags.join(' · ')}）`
}

// 策略列表就绪后:未选或所选策略已不存在(被删除/不属于该 kind)则重新落位。
// 筛选场景(allowEmpty)落回「全部策略」而不是某条具体策略,免得静默改变筛选条件
watch(options, (items) => {
  if (!items.length) return
  if (props.allowEmpty && model.value === null) return
  const stillExists = items.some((strategy) => strategy.id === model.value)
  if (!stillExists) {
    model.value = props.allowEmpty ? null : defaultStrategyId(props.kind ?? undefined, items as Strategy[])
    emit('change', model.value)
  }
}, { immediate: true })

function onSelect(value: string) {
  model.value = value === '' ? null : Number(value)
  emit('change', model.value)
}

onMounted(() => {
  void load().catch(() => { /* 错误已进 error,页面按需展示 */ })
})
</script>

<template>
  <div class="text-sm">
    <div class="flex items-end gap-2">
      <label class="block">
        <span class="mb-1 block text-xs text-text-tertiary">{{ label }}</span>
        <select
          :value="model ?? ''"
          :disabled="disabled || loading || (!options.length && !allowEmpty)"
          class="min-w-52 rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm disabled:opacity-50"
          :aria-describedby="selected && !selected.params_valid ? 'strategy-params-hint' : undefined"
          @change="onSelect(($event.target as HTMLSelectElement).value)"
        >
          <option v-if="loading" value="">加载中…</option>
          <option v-else-if="allowEmpty" value="">{{ emptyLabel }}</option>
          <option v-else-if="!options.length" value="">暂无可用策略</option>
          <option v-for="strategy in options" :key="strategy.id" :value="strategy.id">
            {{ optionLabel(strategy) }}
          </option>
        </select>
      </label>
      <router-link
        v-if="manageLink"
        :to="{ name: 'strategies', query: { tab: 'manage' } }"
        class="mb-0.5 inline-flex items-center gap-1 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-hover hover:text-text-primary"
      >
        <Settings2 :size="14" />
        管理策略
      </router-link>
    </div>

    <p v-if="error" class="mt-1 text-xs text-up">策略加载失败：{{ error }}</p>

    <p v-else-if="selected" class="mt-1 text-xs text-text-tertiary">
      算法模板：{{ selected.template_name }} · {{ selected.kind_name }}
    </p>

    <p
      v-if="selected && !selected.params_valid"
      id="strategy-params-hint"
      class="mt-1 text-xs text-up"
    >
      该策略的参数与当前算法模板不匹配，请到「管理策略」修正后再使用。
    </p>
  </div>
</template>
