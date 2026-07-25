<script setup lang="ts">
import type { CatalogParameter, StrategyParamValue } from '../api'

const props = withDefaults(defineProps<{
  parameters: readonly CatalogParameter[]
  errors?: Record<string, string>
  disabled?: boolean
  idPrefix?: string
}>(), {
  errors: () => ({}),
  disabled: false,
  idPrefix: 'strategy-param',
})

const model = defineModel<Record<string, StrategyParamValue>>({ required: true })

function fieldId(parameter: CatalogParameter): string {
  return `${props.idPrefix}-${parameter.key.replace(/[^a-zA-Z0-9_-]/g, '-')}`
}

function parameterType(parameter: CatalogParameter): NonNullable<CatalogParameter['value_type']> {
  if (parameter.value_type) return parameter.value_type
  if (typeof parameter.default === 'boolean') return 'boolean'
  if (typeof parameter.default === 'string') return 'string'
  return 'number'
}

function isBoolean(parameter: CatalogParameter): boolean {
  return parameterType(parameter) === 'boolean'
}

function isText(parameter: CatalogParameter): boolean {
  return parameterType(parameter) === 'string'
}
</script>

<template>
  <div class="flex flex-wrap gap-3">
    <label
      v-for="parameter in parameters"
      :key="parameter.key"
      :for="fieldId(parameter)"
      class="text-sm"
    >
      <span class="mb-1 block text-xs text-text-tertiary">
        {{ parameter.name }}<template v-if="parameter.unit">（{{ parameter.unit }}）</template>
      </span>
      <span v-if="isBoolean(parameter)" class="flex h-9 items-center gap-2">
        <input
          :id="fieldId(parameter)"
          v-model="model[parameter.key]"
          type="checkbox"
          :disabled="disabled"
          :aria-invalid="errors[parameter.key] ? 'true' : undefined"
          :aria-describedby="errors[parameter.key] ? `${fieldId(parameter)}-error` : undefined"
          class="h-4 w-4 rounded border-border disabled:opacity-50"
        />
        <span class="text-sm text-text-secondary">启用</span>
      </span>
      <input
        v-else-if="isText(parameter)"
        :id="fieldId(parameter)"
        v-model.trim="model[parameter.key]"
        type="text"
        required
        :disabled="disabled"
        :aria-invalid="errors[parameter.key] ? 'true' : undefined"
        :aria-describedby="errors[parameter.key] ? `${fieldId(parameter)}-error` : undefined"
        class="w-48 rounded-md border border-border px-2 py-1.5 disabled:opacity-50"
      />
      <input
        v-else
        :id="fieldId(parameter)"
        v-model.number="model[parameter.key]"
        type="number"
        required
        :min="parameter.minimum"
        :max="parameter.maximum"
        :step="parameter.value_type === 'integer' ? 1 : (parameter.step ?? 'any')"
        :disabled="disabled"
        :aria-invalid="errors[parameter.key] ? 'true' : undefined"
        :aria-describedby="errors[parameter.key] ? `${fieldId(parameter)}-error` : undefined"
        class="w-32 rounded-md border border-border px-2 py-1.5 disabled:opacity-50"
      />
      <span
        v-if="errors[parameter.key]"
        :id="`${fieldId(parameter)}-error`"
        class="mt-1 block max-w-56 text-xs leading-5 text-up"
      >
        {{ errors[parameter.key] }}
      </span>
      <span v-else-if="parameter.description" class="mt-1 block max-w-56 text-xs leading-5 text-text-tertiary">
        {{ parameter.description }}
      </span>
    </label>
  </div>
</template>
