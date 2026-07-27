<script setup lang="ts">
defineProps<{
  title: string
  description?: string
  eyebrow?: string
}>()
</script>

<template>
  <header class="flex min-h-11 flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border pb-2.5">
    <div class="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
      <p v-if="eyebrow" class="text-xs font-medium text-accent">{{ eyebrow }}</p>
      <h1 class="text-base font-semibold text-text-primary">{{ title }}</h1>
      <p v-if="description" class="max-w-3xl text-xs leading-5 text-text-secondary">
        {{ description }}
      </p>
    </div>
    <div v-if="$slots.actions" class="flex shrink-0 items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
