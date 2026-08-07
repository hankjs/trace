<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { api, clearToken, getToken } from './api'
import NeuButton from './components/ui/NeuButton.vue'
import './composables/useTheme'

const route = useRoute()
const router = useRouter()
const displayName = ref('')
const menuOpen = ref(false)

const bare = computed(() => route.path === '/login')

const nav = [
  { to: '/', label: '节点', icon: 'server' as const },
  { to: '/settings', label: '设置', icon: 'sliders' as const },
]

function isActive(to: string) {
  return to === '/' ? route.path === '/' || route.path.startsWith('/nodes') : route.path.startsWith(to)
}

watch(
  () => route.path,
  () => {
    menuOpen.value = false
  },
)

onMounted(async () => {
  if (!getToken()) return
  try {
    const me = await api.whoami()
    displayName.value = me.username || '我'
  } catch {
    // 路由守卫会处理
  }
})

function logout() {
  menuOpen.value = false
  clearToken()
  void router.push('/login')
}
</script>

<template>
  <div class="min-h-screen bg-canvas text-ink">
    <header
      v-if="!bare"
      class="sticky top-0 z-(--z-sticky) border-b border-(--shadow-lo) bg-raise"
    >
      <div class="mx-auto flex max-w-6xl items-center gap-3 px-4 py-3 sm:gap-6 sm:px-6">
        <button
          class="-ml-1.5 shrink-0 neu-icon-btn p-1.5 sm:hidden"
          aria-label="打开菜单"
          @click="menuOpen = true"
        >
          <svg
            class="h-5 w-5"
            fill="none"
            viewBox="0 0 24 24"
            stroke-width="1.8"
            stroke="currentColor"
            stroke-linecap="round"
          >
            <path d="M4 6h16M4 12h16M4 18h16" />
          </svg>
        </button>
        <RouterLink to="/" class="text-lg font-medium text-ink">App</RouterLink>
        <nav class="hidden gap-1 sm:flex">
          <RouterLink
            v-for="item in nav"
            :key="item.to"
            :to="item.to"
            class="rounded-md px-3 py-1.5 text-sm transition hover:bg-canvas"
            :class="
              isActive(item.to)
                ? 'bg-canvas shadow-(--neu-inset) text-ink'
                : 'text-ink-2'
            "
          >
            {{ item.label }}
          </RouterLink>
        </nav>
        <div class="ml-auto flex items-center gap-3 text-sm text-ink-2">
          <span v-if="displayName" class="max-w-32 truncate">{{ displayName }}</span>
          <NeuButton variant="text" @click="logout">退出</NeuButton>
        </div>
      </div>
    </header>

    <main :class="bare ? '' : 'mx-auto max-w-6xl px-4 py-4 sm:px-6 sm:py-6'">
      <RouterView />
    </main>

    <Teleport to="body">
      <Transition
        enter-active-class="transition duration-200 ease-out"
        enter-from-class="opacity-0"
        leave-active-class="transition duration-150 ease-in"
        leave-to-class="opacity-0"
      >
        <div
          v-if="menuOpen && !bare"
          class="sheet-backdrop sm:hidden"
          @click="menuOpen = false"
        />
      </Transition>
      <Transition
        enter-active-class="transition duration-200 ease-out"
        enter-from-class="-translate-x-full"
        leave-active-class="transition duration-150 ease-in"
        leave-to-class="-translate-x-full"
      >
        <div
          v-if="menuOpen && !bare"
          class="fixed inset-y-0 left-0 z-(--z-overlay) flex w-64 flex-col bg-raise shadow-(--neu-raised) sm:hidden"
          :style="{ paddingLeft: 'env(safe-area-inset-left)' }"
        >
          <div class="flex items-center justify-between border-b border-(--shadow-lo) px-4 py-3">
            <span class="font-medium text-ink">App</span>
            <button class="neu-icon-btn p-1.5" aria-label="关闭菜单" @click="menuOpen = false">
              <svg
                class="h-5 w-5"
                fill="none"
                viewBox="0 0 24 24"
                stroke-width="1.8"
                stroke="currentColor"
                stroke-linecap="round"
              >
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </button>
          </div>
          <nav class="flex flex-col gap-0.5 p-2">
            <RouterLink
              v-for="item in nav"
              :key="item.to"
              :to="item.to"
              class="flex items-center gap-3 rounded-md px-3 py-2.5 text-sm"
              :class="
                isActive(item.to)
                  ? 'bg-canvas shadow-(--neu-inset) text-ink'
                  : 'text-ink-2 hover:bg-canvas'
              "
            >
              <span>{{ item.label }}</span>
            </RouterLink>
          </nav>
          <div
            class="mt-auto flex items-center justify-between border-t border-(--shadow-lo) px-4 py-3 text-sm text-ink-2"
            :style="{ paddingBottom: 'calc(0.75rem + env(safe-area-inset-bottom))' }"
          >
            <span v-if="displayName" class="truncate">{{ displayName }}</span>
            <NeuButton variant="text" @click="logout">退出</NeuButton>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
