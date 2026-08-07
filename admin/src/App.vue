<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { clearToken } from './composables/api'
import { useAiGenerate } from './composables/useAiGenerate'
import NavIcon from './components/NavIcon.vue'

const route = useRoute()
const router = useRouter()
const { visible, generating, output, close, generate, confirm } = useAiGenerate()
const aiPrompt = ref('')
// 品牌标：public/favicon.png，经 base `/admin/` 提供
const faviconUrl = `${import.meta.env.BASE_URL}favicon.png`

/** 移动端侧栏抽屉开关；≥md 时侧栏常驻，此状态不影响布局 */
const navOpen = ref(false)
/** 与 Tailwind md 对齐：桌面常驻侧栏，不参与抽屉 inert/aria 逻辑 */
const isMdUp = ref(false)
let mdMq: MediaQueryList | null = null
function syncMdMq() {
  isMdUp.value = mdMq?.matches ?? false
  if (isMdUp.value) navOpen.value = false
}
onMounted(() => {
  mdMq = window.matchMedia('(min-width: 768px)')
  syncMdMq()
  mdMq.addEventListener('change', syncMdMq)
})
onUnmounted(() => {
  mdMq?.removeEventListener('change', syncMdMq)
})
/** 抽屉关闭时对 AT 隐藏；桌面侧栏始终可见 */
const navInert = computed(() => !isMdUp.value && !navOpen.value)

const navGroups: { title?: string; items: { to: string; label: string; icon: string }[] }[] = [
  { items: [{ to: '/', label: '概览', icon: 'dashboard' }] },
  {
    title: '会话与追踪',
    items: [
      { to: '/sessions', label: '会话', icon: 'sessions' },
      { to: '/explore', label: '探索', icon: 'explore' },
    ],
  },
  {
    title: '渠道',
    items: [
      { to: '/feishu', label: '飞书机器人', icon: 'feishu' },
      { to: '/weixin', label: '微信机器人', icon: 'weixin' },
      { to: '/chat-records', label: '聊天记录', icon: 'records' },
    ],
  },
  {
    title: '任务',
    items: [
      { to: '/jobs', label: '定时任务', icon: 'jobs' },
      { to: '/interactions', label: '交互单', icon: 'interaction' },
    ],
  },
  {
    title: '模型与工具',
    items: [
      { to: '/providers', label: '供应商', icon: 'provider' },
      { to: '/image-providers', label: '生图供应商', icon: 'image' },
      { to: '/prompts', label: '提示词', icon: 'prompt' },
    ],
  },
  {
    title: '系统',
    items: [
      { to: '/users', label: '用户', icon: 'users' },
      { to: '/api-keys', label: 'API Keys', icon: 'key' },
    ],
  },
]

// 是否「占满视口高度、内部自行滚动」的页面（由路由 meta.fill 声明）
const isFill = computed(() => route.meta.fill === true)
// 内容区最大宽度：meta.width 为 'wide' 时放宽，'full' 时不限
const widthClass = computed(() => {
  if (route.meta.width === 'full') return 'w-full'
  if (route.meta.width === 'wide') return 'w-full max-w-[1400px]'
  return 'max-w-4xl'
})

/** 当前页标题：优先匹配最深路径的导航项，用于移动端顶栏 */
const pageTitle = computed(() => {
  const path = route.path
  let best = 'Trace'
  let bestLen = -1
  for (const group of navGroups) {
    for (const item of group.items) {
      if (item.to === '/') {
        if (path === '/') return item.label
        continue
      }
      if (path === item.to || path.startsWith(item.to + '/')) {
        if (item.to.length > bestLen) {
          best = item.label
          bestLen = item.to.length
        }
      }
    }
  }
  return best
})

function isActive(path: string) {
  if (path === '/') return route.path === '/'
  return route.path.startsWith(path)
}

function logout() {
  clearToken()
  router.push('/login')
}

function handleGenerate() {
  generate(aiPrompt.value)
}

function closeNav() {
  navOpen.value = false
}

function openNav() {
  navOpen.value = true
}

// 路由切换后关闭抽屉，避免遮挡新页
watch(() => route.fullPath, () => {
  navOpen.value = false
})
</script>

<template>
  <div v-if="route.path === '/login'" class="min-h-dvh">
    <RouterView />
  </div>
  <div v-else class="flex h-dvh flex-col overflow-hidden md:flex-row">
    <!-- 遮罩：仅移动端抽屉打开时 -->
    <div
      v-if="navOpen"
      class="fixed inset-0 z-40 bg-black/40 md:hidden"
      aria-hidden="true"
      @click="closeNav"
    />

    <!-- 侧栏：移动端为抽屉，md+ 常驻 -->
    <aside
      id="admin-nav"
      class="fixed inset-y-0 left-0 z-50 flex w-[min(16.5rem,85vw)] max-w-full flex-col overflow-hidden border-r border-border-subtle bg-surface transition-transform duration-200 ease-out md:static md:z-auto md:w-52 md:max-w-none md:translate-x-0 md:shrink-0"
      :class="navOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'"
      :aria-hidden="navInert || undefined"
      :inert="navInert || undefined"
    >
      <!-- 品牌标：固定不滚动 -->
      <div class="flex shrink-0 items-center justify-between gap-2 px-4 py-4 md:px-5 md:py-5">
        <div class="flex items-center gap-2">
          <img :src="faviconUrl" alt="" width="20" height="20" class="shrink-0 rounded-[5px]" />
          <span class="text-sm font-medium tracking-tight text-text-secondary">Trace</span>
        </div>
        <button
          type="button"
          class="flex size-9 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-hover hover:text-text-secondary md:hidden"
          aria-label="关闭菜单"
          @click="closeNav"
        >
          <svg class="size-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true">
            <path d="M18 6 6 18M6 6l12 12" />
          </svg>
        </button>
      </div>

      <!-- 菜单：侧边栏内部独立滚动 -->
      <nav class="thin-scrollbar min-h-0 flex-1 overflow-y-auto px-3 pb-2" aria-label="主导航">
        <div v-for="(group, gi) in navGroups" :key="gi" :class="gi > 0 ? 'mt-4' : ''">
          <p
            v-if="group.title"
            class="px-2 pb-1 text-[11px] font-medium tracking-wide text-text-tertiary"
          >{{ group.title }}</p>
          <RouterLink
            v-for="item in group.items"
            :key="item.to"
            :to="item.to"
            class="flex min-h-10 items-center gap-2.5 rounded-md px-2 py-2 text-[13px] transition-colors duration-100 md:min-h-0 md:py-1.5"
            :class="isActive(item.to) ? 'bg-active text-text-primary font-medium' : 'text-text-secondary hover:bg-hover'"
            @click="closeNav"
          >
            <NavIcon :name="item.icon" class="size-4 shrink-0 opacity-70" />
            {{ item.label }}
          </RouterLink>
        </div>
      </nav>

      <!-- 退出登录：固定在底部 -->
      <div class="shrink-0 border-t border-border-subtle px-3 py-2 safe-pb">
        <button
          type="button"
          class="w-full min-h-10 px-2 py-2 text-left text-[12px] text-text-tertiary transition-colors hover:text-text-secondary md:min-h-0 md:py-1.5"
          @click="logout"
        >退出登录</button>
      </div>
    </aside>

    <!-- 主列：移动端顶栏 + 内容 -->
    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        class="flex shrink-0 items-center gap-2 border-b border-border-subtle bg-surface px-3 py-2 safe-pt md:hidden"
      >
        <button
          type="button"
          class="flex size-10 shrink-0 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-hover"
          aria-label="打开菜单"
          :aria-expanded="navOpen"
          aria-controls="admin-nav"
          @click="openNav"
        >
          <svg class="size-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true">
            <path d="M4 7h16M4 12h16M4 17h16" />
          </svg>
        </button>
        <div class="flex min-w-0 flex-1 items-center gap-2">
          <img :src="faviconUrl" alt="" width="16" height="16" class="shrink-0 rounded-[3px]" />
          <span class="truncate text-[13px] font-medium text-text-primary">{{ pageTitle }}</span>
        </div>
      </header>

      <main
        class="min-h-0 min-w-0 flex-1"
        :class="isFill ? 'flex flex-col overflow-hidden' : 'thin-scrollbar overflow-y-auto'"
      >
        <div
          class="px-4 py-4 sm:px-6 sm:py-6 md:px-8 md:py-7 lg:px-10 lg:py-8"
          :class="[
            widthClass,
            isFill ? 'flex min-h-0 w-full flex-1 flex-col' : '',
          ]"
        >
          <RouterView />
        </div>
      </main>
    </div>
  </div>

  <!-- AI Generate Floating Panel -->
  <div
    v-if="visible"
    class="fixed inset-0 z-[60] flex items-end justify-center bg-black/40 sm:items-center"
  >
    <div
      class="max-h-[min(90dvh,100%)] w-full overflow-y-auto border border-border bg-surface-raised p-4 shadow-xl sm:mx-4 sm:max-w-lg sm:rounded-xl sm:p-5 max-sm:rounded-t-xl safe-pb"
    >
      <div class="mb-4 flex items-center justify-between">
        <span class="text-[13px] font-medium text-text-primary">AI 生成</span>
        <button
          type="button"
          class="flex size-9 items-center justify-center text-sm text-text-tertiary transition-colors hover:text-text-secondary"
          aria-label="关闭"
          @click="close"
        >✕</button>
      </div>
      <textarea
        v-model="aiPrompt"
        placeholder="输入提示词..."
        rows="3"
        class="mb-3 w-full resize-y rounded-md border border-border bg-transparent px-3 py-2 text-[13px] font-mono leading-relaxed placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
      ></textarea>
      <button
        type="button"
        :disabled="generating || !aiPrompt.trim()"
        class="mb-3 min-h-10 rounded-md bg-text-primary px-3.5 py-2 text-[13px] text-surface-raised transition-opacity hover:opacity-80 disabled:opacity-40 sm:min-h-0 sm:py-1.5"
        @click="handleGenerate"
      >{{ generating ? '生成中...' : '生成' }}</button>
      <div v-if="output" class="mb-3 max-h-60 overflow-y-auto rounded-md border border-border-subtle p-3">
        <pre class="whitespace-pre-wrap font-mono text-[12px] leading-relaxed text-text-secondary">{{ output }}</pre>
      </div>
      <div v-if="output && !generating" class="flex justify-end gap-2">
        <button
          type="button"
          class="min-h-10 px-3 py-2 text-[12px] text-text-tertiary transition-colors hover:text-text-secondary sm:min-h-0 sm:py-1.5"
          @click="close"
        >取消</button>
        <button
          type="button"
          class="min-h-10 rounded-md bg-accent px-3.5 py-2 text-[12px] text-white transition-opacity hover:opacity-80 sm:min-h-0 sm:py-1.5"
          @click="confirm"
        >确认回填</button>
      </div>
    </div>
  </div>
</template>
