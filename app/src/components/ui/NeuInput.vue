<script setup lang="ts">
const model = defineModel<string>()

withDefaults(
  defineProps<{
    type?: string
    monospace?: boolean
  }>(),
  { type: 'text' },
)
</script>

<template>
  <input
    v-model="model"
    :type="type"
    class="neu-input px-3 py-2 text-base sm:py-1.5 sm:text-sm"
    :class="monospace ? 'font-mono' : ''"
  />
</template>
