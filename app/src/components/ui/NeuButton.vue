<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    variant?: 'primary' | 'plain' | 'text' | 'text-danger'
    size?: 'md' | 'sm' | 'icon'
    type?: 'button' | 'submit' | 'reset'
    disabled?: boolean
  }>(),
  { variant: 'plain', size: 'md', type: 'button' },
)

const classes = computed(() => {
  if (props.variant === 'text') {
    return 'px-2 py-1 text-xs text-ink-2 transition-colors hover:text-ink'
  }
  if (props.variant === 'text-danger') {
    return 'px-2 py-1 text-xs text-ink-3 transition-colors hover:text-danger'
  }
  const surface = props.variant === 'primary' ? 'neu-btn-primary' : 'neu-btn'
  if (props.size === 'icon') {
    return `${surface} flex h-9 w-9 items-center justify-center rounded-full`
  }
  const size =
    props.size === 'sm' ? 'px-2.5 py-1.5 text-xs' : 'px-3 py-2 text-sm sm:py-1.5'
  return `${surface} ${size}`
})
</script>

<template>
  <button :type="type" :disabled="disabled" :class="classes">
    <slot />
  </button>
</template>
