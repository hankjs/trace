<script setup lang="ts" generic="T extends string | number | boolean | null | undefined">
/** 通用下拉选择:替代原生 <select>,样式与交互对齐设计系统(参考 StockSearchInput 的下拉模式)。
 *
 * - v-model 泛型值,支持 string / number / boolean / null;
 * - 透传的 class 落在触发按钮上并整体替换默认外观(宽度/高度/字号类按调用方保留);
 * - 键盘:Enter/Space/ArrowDown 打开,ArrowUp/ArrowDown 移动高亮,Enter 选中,Escape 关闭;
 * - 无障碍:触发按钮 aria-haspopup/aria-expanded/aria-activedescendant,弹出层 role="listbox",
 *   选项 role="option" + aria-selected,选中项左侧显示 Check 图标;
 * - 选项上的 data-value 供测试定位(String(value),null/undefined 为空串)。
 */
import { computed, nextTick, ref, useAttrs } from 'vue'
import { Check, ChevronDown } from 'lucide-vue-next'

interface QuSelectOption {
  value: T
  label: string
  disabled?: boolean
  /** 分组标题:与前一选项分组不同时渲染分隔行(替代原生 optgroup) */
  group?: string
}

const props = withDefaults(defineProps<{
  options: QuSelectOption[]
  placeholder?: string
  disabled?: boolean
  ariaLabel?: string
}>(), {
  placeholder: '请选择',
  disabled: false,
  ariaLabel: undefined,
})

defineOptions({ inheritAttrs: false })

const model = defineModel<T>()
const emit = defineEmits<{ change: [value: T] }>()

const attrs = useAttrs()
const open = ref(false)
const activeIndex = ref(-1)

const listId = `qu-select-${Math.random().toString(36).slice(2)}`

// class 单独处理(整体替换默认外观),其余 attr(id、aria-describedby 等)透传到触发按钮
const buttonAttrs = computed(() => {
  const { class: _class, ...rest } = attrs
  return rest
})

const defaultTriggerClass = 'rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm text-text-primary focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55'

const selectedIndex = computed(() => props.options.findIndex((option) => option.value === model.value))
const selectedOption = computed(() => (selectedIndex.value >= 0 ? props.options[selectedIndex.value] : null))
const triggerLabel = computed(() => selectedOption.value?.label ?? props.placeholder)
const activeDescendant = computed(() => (open.value && activeIndex.value >= 0 ? `${listId}-${activeIndex.value}` : undefined))

function stringifyValue(value: T): string {
  return value === null || value === undefined ? '' : String(value)
}

function firstEnabledIndex(): number {
  return props.options.findIndex((option) => !option.disabled)
}

async function scrollActiveIntoView() {
  await nextTick()
  document.getElementById(`${listId}-${activeIndex.value}`)?.scrollIntoView?.({ block: 'nearest' })
}

function openList() {
  if (props.disabled) return
  open.value = true
  activeIndex.value = selectedIndex.value >= 0 ? selectedIndex.value : firstEnabledIndex()
  void scrollActiveIntoView()
}

function closeList() {
  open.value = false
  activeIndex.value = -1
}

function toggleOpen() {
  if (open.value) closeList()
  else openList()
}

function moveActive(step: 1 | -1) {
  const count = props.options.length
  if (!count) return
  let index = activeIndex.value
  for (let i = 0; i < count; i++) {
    index = (index + step + count) % count
    if (!props.options[index].disabled) break
  }
  activeIndex.value = index
  void scrollActiveIntoView()
}

function selectIndex(index: number) {
  const option = props.options[index]
  if (!option || option.disabled) return
  model.value = option.value
  emit('change', option.value)
  closeList()
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled) return
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    if (open.value) moveActive(1)
    else openList()
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    if (open.value) moveActive(-1)
    else openList()
  } else if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    if (open.value) {
      if (activeIndex.value >= 0) selectIndex(activeIndex.value)
    } else openList()
  } else if (event.key === 'Escape' && open.value) {
    event.preventDefault()
    event.stopPropagation()
    closeList()
  }
}
</script>

<template>
  <div class="relative">
    <button
      v-bind="buttonAttrs"
      type="button"
      :disabled="disabled"
      :aria-label="ariaLabel"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :aria-controls="listId"
      :aria-activedescendant="activeDescendant"
      class="flex items-center justify-between gap-2 text-left"
      :class="attrs.class ?? defaultTriggerClass"
      @click="toggleOpen"
      @keydown="onKeydown"
      @blur="closeList"
    >
      <span class="min-w-0 flex-1 truncate" :class="selectedOption ? '' : 'text-text-tertiary'">{{ triggerLabel }}</span>
      <ChevronDown :size="15" class="shrink-0 text-text-tertiary transition-transform" :class="open ? 'rotate-180' : ''" />
    </button>

    <div
      v-if="open"
      :id="listId"
      role="listbox"
      class="absolute left-0 top-full z-30 mt-1 max-h-64 w-max min-w-full max-w-80 overflow-y-auto rounded-md bg-surface-raised py-1 shadow-panel"
    >
      <template v-for="(option, index) in options" :key="index">
        <div
          v-if="option.group && (index === 0 || options[index - 1].group !== option.group)"
          class="px-3 pb-1 pt-1.5 text-[11px] text-text-tertiary"
        >
          {{ option.group }}
        </div>
        <button
          :id="`${listId}-${index}`"
          type="button"
          role="option"
          :aria-selected="index === selectedIndex"
          :aria-disabled="option.disabled || undefined"
          :data-value="stringifyValue(option.value)"
          class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm"
          :class="option.disabled
            ? 'cursor-not-allowed text-text-tertiary opacity-60'
            : index === activeIndex
              ? 'bg-active'
              : 'hover:bg-hover'"
          @mouseenter="!option.disabled && (activeIndex = index)"
          @mousedown.prevent
          @click="selectIndex(index)"
        >
          <span class="flex w-4 shrink-0 items-center justify-center">
            <Check v-if="index === selectedIndex" :size="14" class="text-accent" />
          </span>
          <span class="min-w-0 flex-1 truncate">{{ option.label }}</span>
        </button>
      </template>
      <p v-if="!options.length" class="px-3 py-2 text-xs text-text-tertiary">{{ placeholder }}</p>
    </div>
  </div>
</template>
