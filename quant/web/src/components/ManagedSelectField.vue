<script setup lang="ts">
import { computed } from 'vue'
import { Settings2 } from 'lucide-vue-next'
import type { RouteLocationRaw } from 'vue-router'
import QuSelect from './QuSelect.vue'

export interface ManagedSelectOption {
  value: number
  label: string
}

const props = withDefaults(defineProps<{
  label: string
  options: ManagedSelectOption[]
  loading?: boolean
  error?: string
  disabled?: boolean
  allowEmpty?: boolean
  emptyLabel?: string
  unavailableLabel?: string
  manageLink?: boolean
  manageTo: RouteLocationRaw
  manageLabel: string
  describedBy?: string
}>(), {
  loading: false,
  error: '',
  disabled: false,
  allowEmpty: false,
  emptyLabel: '全部',
  unavailableLabel: '暂无可用选项',
  manageLink: true,
  describedBy: undefined,
})

const model = defineModel<number | null>({ required: true })
const emit = defineEmits<{ change: [value: number | null] }>()

// 空选项(筛选场景的「全部」)作为 value=null 的真实选项;loading/无选项走占位文案 + 禁用
const selectOptions = computed(() => [
  ...(props.allowEmpty ? [{ value: null, label: props.emptyLabel }] : []),
  ...props.options,
])
const placeholder = computed(() => {
  if (props.loading) return '加载中…'
  if (!props.options.length && !props.allowEmpty) return props.unavailableLabel
  return '请选择'
})
</script>

<template>
  <div class="text-sm">
    <div class="flex items-end gap-2">
      <label class="block">
        <span v-if="label" class="mb-1 block text-xs text-text-tertiary">{{ label }}</span>
        <QuSelect
          v-model="model"
          :options="selectOptions"
          :placeholder="placeholder"
          :disabled="disabled || loading || (!options.length && !allowEmpty)"
          :aria-describedby="describedBy"
          class="min-w-52 rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm disabled:opacity-50"
          @change="emit('change', $event)"
        />
      </label>
      <router-link
        v-if="manageLink"
        :to="manageTo"
        class="mb-0.5 inline-flex items-center gap-1 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-hover hover:text-text-primary"
      >
        <Settings2 :size="14" />
        {{ manageLabel }}
      </router-link>
    </div>

    <p v-if="error" role="alert" class="mt-1 text-xs text-up">{{ error }}</p>
    <slot v-else />
  </div>
</template>
