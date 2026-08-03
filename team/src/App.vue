<script setup lang="ts">
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { clearToken } from './composables/api'

const route = useRoute()
const router = useRouter()

function logout() {
  clearToken()
  router.push('/login')
}
</script>

<template>
  <div v-if="route.path === '/login'" class="min-h-screen">
    <RouterView />
  </div>
  <div v-else class="min-h-screen flex flex-col">
    <header class="border-b border-border-subtle px-6 py-3 flex items-center justify-between">
      <div class="flex items-center gap-4">
        <RouterLink to="/" class="text-sm font-medium text-text-secondary tracking-tight hover:text-text-primary">
          Trace Team
        </RouterLink>
        <span class="text-[12px] text-text-tertiary">团队任务看板</span>
      </div>
      <button
        @click="logout"
        class="text-[12px] text-text-tertiary hover:text-text-secondary transition-colors"
      >退出登录</button>
    </header>
    <main class="flex-1 px-6 py-6">
      <RouterView />
    </main>
  </div>
</template>
