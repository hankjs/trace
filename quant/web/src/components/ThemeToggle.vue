<script setup lang="ts">
import { Moon, Sun } from 'lucide-vue-next'
import { useTheme } from '../theme'

const { isDark, toggleTheme } = useTheme()
</script>

<template>
  <button
    type="button"
    class="icon-button shrink-0"
    :title="isDark ? '切换到浅色模式' : '切换到暗色模式'"
    :aria-label="isDark ? '切换到浅色模式' : '切换到暗色模式'"
    :aria-pressed="isDark"
    @click="toggleTheme"
  >
    <Sun v-if="isDark" :size="17" />
    <Moon v-else :size="17" />
  </button>
</template>
