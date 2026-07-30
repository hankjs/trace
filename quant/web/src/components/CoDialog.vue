<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { confirmDialogState, settleConfirmDialog } from '../confirmDialog'

const confirmButton = ref<HTMLButtonElement | null>(null)
const cancelButton = ref<HTMLButtonElement | null>(null)
let previouslyFocused: HTMLElement | null = null

// 危险操作默认聚焦「取消」，防止回车误触确认
watch(() => confirmDialogState.open, async (open) => {
  if (open) {
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null
    await nextTick()
    const target = confirmDialogState.tone === 'danger' ? cancelButton.value : confirmButton.value
    target?.focus()
  } else {
    previouslyFocused?.focus()
    previouslyFocused = null
  }
})

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    settleConfirmDialog(false)
    return
  }
  // 对话框只有取消/确认两个可聚焦元素，Tab 在两者之间循环
  if (event.key === 'Tab') {
    event.preventDefault()
    const target = document.activeElement === confirmButton.value ? cancelButton.value : confirmButton.value
    target?.focus()
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="co-dialog">
      <div
        v-if="confirmDialogState.open"
        class="fixed inset-0 z-[60] flex items-center justify-center p-4"
        @keydown="onKeydown"
      >
        <button
          type="button"
          class="absolute inset-0 cursor-default bg-overlay"
          aria-label="取消"
          tabindex="-1"
          @click="settleConfirmDialog(false)"
        />
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="co-dialog-title"
          aria-describedby="co-dialog-message"
          class="co-dialog-panel relative w-full max-w-sm rounded-md border border-border bg-surface-raised p-5 shadow-panel"
        >
          <h2 id="co-dialog-title" class="text-sm font-semibold leading-5 text-text-primary">
            {{ confirmDialogState.title }}
          </h2>
          <p id="co-dialog-message" class="mt-2 text-sm leading-6 text-text-secondary">
            {{ confirmDialogState.message }}
          </p>
          <div class="mt-5 flex justify-end gap-2">
            <button ref="cancelButton" type="button" class="btn btn-secondary" @click="settleConfirmDialog(false)">
              {{ confirmDialogState.cancelText }}
            </button>
            <button
              ref="confirmButton"
              type="button"
              class="btn"
              :class="confirmDialogState.tone === 'danger' ? 'btn-danger' : 'btn-primary'"
              @click="settleConfirmDialog(true)"
            >
              {{ confirmDialogState.confirmText }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.co-dialog-enter-active,
.co-dialog-leave-active {
  transition: opacity 180ms cubic-bezier(0.16, 1, 0.3, 1);
}

.co-dialog-enter-active .co-dialog-panel,
.co-dialog-leave-active .co-dialog-panel {
  transition: opacity 180ms cubic-bezier(0.16, 1, 0.3, 1),
    transform 180ms cubic-bezier(0.16, 1, 0.3, 1);
}

.co-dialog-enter-from,
.co-dialog-leave-to {
  opacity: 0;
}

.co-dialog-enter-from .co-dialog-panel,
.co-dialog-leave-to .co-dialog-panel {
  opacity: 0;
  transform: translateY(6px) scale(0.98);
}
</style>
