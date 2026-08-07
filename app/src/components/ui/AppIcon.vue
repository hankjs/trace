<script setup lang="ts">
export type IconName =
  | 'check'
  | 'x'
  | 'chevron-left'
  | 'plus'
  | 'sun'
  | 'moon'
  | 'monitor'

const props = defineProps<{ name: IconName }>()

const icons: Record<IconName, string> = {
  check: '<path d="M20 6 9 17l-5-5" />',
  x: '<path d="M6 6l12 12M18 6L6 18" />',
  'chevron-left': '<path d="M15 18l-6-6 6-6" />',
  plus: '<path d="M12 5v14M5 12h14" />',
  sun: '<circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />',
  moon: '<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />',
  monitor:
    '<rect x="2" y="3" width="20" height="14" rx="2" /><path d="M8 21h8M12 17v4" />',
}
</script>

<template>
  <svg
    class="inline-block h-4 w-4"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    v-html="icons[props.name]"
  />
</template>
