<script setup lang="ts">
import { computed, ref } from 'vue'
import BottomSheet from '../components/BottomSheet.vue'
import AppIcon from '../components/ui/AppIcon.vue'
import { setThemeMode, themeMode, type ThemeMode } from '../composables/useTheme'

const options: {
  value: ThemeMode
  label: string
  icon: 'sun' | 'moon' | 'monitor'
}[] = [
  { value: 'system', label: '跟随系统', icon: 'monitor' },
  { value: 'light', label: '亮色', icon: 'sun' },
  { value: 'dark', label: '暗色', icon: 'moon' },
]

const open = ref(false)
const draft = ref<ThemeMode>(themeMode.value)

const current = computed(
  () => options.find((o) => o.value === themeMode.value) ?? options[0],
)

function openPicker() {
  draft.value = themeMode.value
  open.value = true
}

function confirm() {
  setThemeMode(draft.value)
  open.value = false
}
</script>

<template>
  <div>
    <h1 class="text-xl font-medium text-ink">设置</h1>

    <div class="mt-4 neu-card divide-y divide-(--shadow-lo)">
      <div class="flex items-center gap-3 px-4 py-3">
        <div class="min-w-0 flex-1">
          <div class="text-sm text-ink">主题</div>
          <div class="mt-0.5 text-xs text-ink-2">界面外观的亮色 / 暗色模式</div>
        </div>
        <button
          type="button"
          class="neu-chip shrink-0 px-3 py-1.5 text-xs"
          :title="`主题：${current.label}`"
          @click="openPicker"
        >
          <AppIcon :name="current.icon" class="text-ink-2" />
          <span>{{ current.label }}</span>
        </button>
      </div>
    </div>

    <BottomSheet v-model:open="open" title="主题" @confirm="confirm">
      <button
        v-for="o in options"
        :key="o.value"
        type="button"
        class="sheet-option"
        :class="draft === o.value ? 'selected' : ''"
        @click="draft = o.value"
      >
        <AppIcon :name="o.icon" class="shrink-0 text-ink-2" />
        <span class="flex-1 text-sm">{{ o.label }}</span>
        <AppIcon v-if="draft === o.value" name="check" class="text-ink-2" />
      </button>
    </BottomSheet>
  </div>
</template>
