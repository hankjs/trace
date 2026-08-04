<script setup lang="ts">
import { computed, ref } from 'vue'
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
</script>

<template>
  <div v-if="route.path === '/login'" class="min-h-screen">
    <RouterView />
  </div>
  <div v-else class="flex h-screen overflow-hidden">
    <aside class="flex h-full w-52 shrink-0 flex-col overflow-hidden border-r border-border-subtle">
      <!-- 品牌标：固定不滚动 -->
      <div class="flex shrink-0 items-center gap-2 px-5 py-5">
        <img :src="faviconUrl" alt="" width="20" height="20" class="shrink-0 rounded-[5px]" />
        <span class="text-sm font-medium tracking-tight text-text-secondary">Trace</span>
      </div>

      <!-- 菜单：侧边栏内部独立滚动 -->
      <nav class="thin-scrollbar min-h-0 flex-1 overflow-y-auto px-3 pb-2">
        <div v-for="(group, gi) in navGroups" :key="gi" :class="gi > 0 ? 'mt-4' : ''">
          <p
            v-if="group.title"
            class="px-2 pb-1 text-[11px] font-medium tracking-wide text-text-tertiary"
          >{{ group.title }}</p>
          <RouterLink
            v-for="item in group.items"
            :key="item.to"
            :to="item.to"
            class="flex items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] transition-colors duration-100"
            :class="isActive(item.to) ? 'bg-active text-text-primary font-medium' : 'text-text-secondary hover:bg-hover'"
          >
            <NavIcon :name="item.icon" class="size-4 shrink-0 opacity-70" />
            {{ item.label }}
          </RouterLink>
        </div>
      </nav>

      <!-- 退出登录：固定在底部 -->
      <div class="shrink-0 border-t border-border-subtle px-3 py-2">
        <button
          @click="logout"
          class="w-full px-2 py-1.5 text-left text-[12px] text-text-tertiary transition-colors hover:text-text-secondary"
        >退出登录</button>
      </div>
    </aside>

    <main
      class="min-w-0 flex-1"
      :class="isFill ? 'flex flex-col overflow-hidden' : 'thin-scrollbar overflow-y-auto'"
    >
      <div
        class="px-10 py-8"
        :class="[
          widthClass,
          isFill ? 'flex min-h-0 w-full flex-1 flex-col' : '',
        ]"
      >
        <RouterView />
      </div>
    </main>
  </div>

  <!-- AI Generate Floating Panel -->
  <div v-if="visible" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
    <div class="bg-surface-raised border border-border rounded-xl shadow-xl w-full max-w-lg mx-4 p-5">
      <div class="flex items-center justify-between mb-4">
        <span class="text-[13px] font-medium text-text-primary">AI 生成</span>
        <button @click="close" class="text-text-tertiary hover:text-text-secondary text-sm transition-colors">✕</button>
      </div>
      <textarea
        v-model="aiPrompt"
        placeholder="输入提示词..."
        rows="3"
        class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] font-mono leading-relaxed placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors resize-y mb-3"
      ></textarea>
      <button
        @click="handleGenerate"
        :disabled="generating || !aiPrompt.trim()"
        class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 disabled:opacity-40 transition-opacity mb-3"
      >{{ generating ? '生成中...' : '生成' }}</button>
      <div v-if="output" class="border border-border-subtle rounded-md p-3 mb-3 max-h-60 overflow-y-auto">
        <pre class="text-[12px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed">{{ output }}</pre>
      </div>
      <div v-if="output && !generating" class="flex gap-2 justify-end">
        <button @click="close" class="px-3 py-1.5 text-[12px] text-text-tertiary hover:text-text-secondary transition-colors">取消</button>
        <button @click="confirm" class="px-3.5 py-1.5 bg-accent text-white text-[12px] rounded-md hover:opacity-80 transition-opacity">确认回填</button>
      </div>
    </div>
  </div>
</template>
