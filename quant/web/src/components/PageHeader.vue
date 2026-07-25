<script setup lang="ts">
defineProps<{
  title: string
  description?: string
  eyebrow?: string
}>()
</script>

<template>
  <header class="flex flex-wrap items-end justify-between gap-4 border-b border-border-subtle pb-4">
    <div class="min-w-0">
      <p v-if="eyebrow" class="mb-1 text-xs font-medium text-accent">{{ eyebrow }}</p>
      <h1 class="text-xl font-semibold text-text-primary">{{ title }}</h1>
      <p v-if="description" class="mt-1 max-w-3xl text-sm leading-6 text-text-secondary">
        {{ description }}
      </p>
    </div>
    <div v-if="$slots.actions" class="flex shrink-0 items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
