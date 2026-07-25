<script setup lang="ts">
import { computed } from 'vue'
import { AlertCircle, AlertTriangle, Info } from 'lucide-vue-next'

const props = withDefaults(defineProps<{
  tone?: 'error' | 'warning' | 'info'
}>(), {
  tone: 'info',
})

const role = computed(() => props.tone === 'error' ? 'alert' : 'status')
const icon = computed(() => {
  if (props.tone === 'error') return AlertCircle
  if (props.tone === 'warning') return AlertTriangle
  return Info
})
const toneClass = computed(() => {
  if (props.tone === 'error') return 'border-up/30 bg-danger-soft text-up'
  if (props.tone === 'warning') return 'border-warning/30 bg-warning-soft text-warning'
  return 'border-border bg-info-soft text-text-secondary'
})
</script>

<template>
  <p
    :role="role"
    :aria-live="tone === 'error' ? 'assertive' : 'polite'"
    class="flex items-start gap-2 rounded-md border px-4 py-2 text-sm leading-5"
    :class="toneClass"
  >
    <component :is="icon" :size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
    <span><slot /></span>
  </p>
</template>
