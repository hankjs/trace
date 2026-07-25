<script setup lang="ts">
withDefaults(defineProps<{
  rows?: number
}>(), {
  rows: 4,
})
</script>

<template>
  <div class="animate-pulse overflow-hidden rounded-md border border-border bg-surface-raised" aria-label="正在加载">
    <div v-for="row in rows" :key="row" class="flex items-center gap-4 border-b border-border-subtle px-4 py-3 last:border-0">
      <span class="h-3 w-24 rounded-sm bg-active" />
      <span class="h-3 flex-1 rounded-sm bg-active" />
      <span class="h-3 w-16 rounded-sm bg-active" />
    </div>
  </div>
</template>
