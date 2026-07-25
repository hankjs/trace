<script setup lang="ts">
import { Settings2 } from 'lucide-vue-next'
import type { RouteLocationRaw } from 'vue-router'

export interface ManagedSelectOption {
  value: number
  label: string
}

withDefaults(defineProps<{
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

function onSelect(value: string) {
  model.value = value === '' ? null : Number(value)
  emit('change', model.value)
}
</script>

<template>
  <div class="text-sm">
    <div class="flex items-end gap-2">
      <label class="block">
        <span class="mb-1 block text-xs text-text-tertiary">{{ label }}</span>
        <select
          :value="model ?? ''"
          :disabled="disabled || loading || (!options.length && !allowEmpty)"
          :aria-describedby="describedBy"
          class="min-w-52 rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm disabled:opacity-50"
          @change="onSelect(($event.target as HTMLSelectElement).value)"
        >
          <option v-if="loading" value="">加载中…</option>
          <option v-else-if="allowEmpty" value="">{{ emptyLabel }}</option>
          <option v-else-if="!options.length" value="">{{ unavailableLabel }}</option>
          <option v-for="option in options" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
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
