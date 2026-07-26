<script setup lang="ts">
export interface WorkspaceTab {
  key: string
  label: string
  description?: string
}

defineProps<{
  tabs: WorkspaceTab[]
  active: string
}>()

const emit = defineEmits<{
  change: [key: string]
}>()
</script>

<template>
  <div class="border-b border-border" role="tablist" aria-label="工作区视图">
    <div class="flex gap-1 overflow-x-auto">
      <button
        v-for="tab in tabs"
        :key="tab.key"
        type="button"
        role="tab"
        :aria-selected="active === tab.key"
        class="relative shrink-0 px-3 py-2 text-sm transition-colors"
        :class="active === tab.key ? 'font-medium text-accent' : 'text-text-secondary hover:bg-hover hover:text-text-primary'"
        @click="emit('change', tab.key)"
      >
        {{ tab.label }}
        <span v-if="active === tab.key" class="absolute inset-x-0 bottom-0 h-0.5 bg-accent" />
      </button>
    </div>
  </div>
</template>
