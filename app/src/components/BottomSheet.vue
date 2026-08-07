<script setup lang="ts">
import { onUnmounted, watch } from 'vue'

const props = defineProps<{
  open: boolean
  title: string
  confirmDisabled?: boolean
}>()
const emit = defineEmits<{
  'update:open': [boolean]
  confirm: []
  cancel: []
}>()

function cancel() {
  emit('update:open', false)
  emit('cancel')
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') cancel()
}

watch(
  () => props.open,
  (open) => {
    document.body.style.overflow = open ? 'hidden' : ''
    if (open) window.addEventListener('keydown', onKeydown)
    else window.removeEventListener('keydown', onKeydown)
  },
)

onUnmounted(() => {
  document.body.style.overflow = ''
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <Teleport to="body">
    <Transition name="sheet-fade">
      <div v-if="open" class="sheet-backdrop" @click="cancel" />
    </Transition>
    <Transition name="sheet-slide">
      <div
        v-if="open"
        class="sheet-panel"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
      >
        <div class="sheet-header">
          <button type="button" class="sheet-action" @click="cancel">取消</button>
          <span class="sheet-title">{{ title }}</span>
          <button
            type="button"
            class="sheet-action sheet-confirm"
            :disabled="confirmDisabled"
            @click="emit('confirm')"
          >
            确定
          </button>
        </div>
        <div v-if="$slots.input" class="sheet-input">
          <slot name="input" />
        </div>
        <div class="sheet-options">
          <slot />
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
