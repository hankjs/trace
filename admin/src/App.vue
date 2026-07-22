<script setup lang="ts">
import { ref } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { clearToken } from './composables/api'
import { useAiGenerate } from './composables/useAiGenerate'

const route = useRoute()
const router = useRouter()
const { visible, generating, output, close, generate, confirm } = useAiGenerate()
const aiPrompt = ref('')

const nav = [
  { to: '/', label: '概览', icon: '◫' },
  { to: '/sessions', label: '会话', icon: '☰' },
  { to: '/explore', label: '探索', icon: '⊙' },
  { to: '/prompts', label: '提示词', icon: '✎' },
  { to: '/providers', label: '供应商', icon: '⚡' },
  { to: '/image-providers', label: '生图供应商', icon: '🖼' },
  { to: '/weixin', label: '微信机器人', icon: '✆' },
  { to: '/terminals', label: '终端', icon: '▸' },
  { to: '/notifications', label: '通知', icon: '◉' },
  { to: '/users', label: '用户', icon: '⚇' },
]

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
  <div v-else class="flex min-h-screen">
    <aside class="w-52 shrink-0 border-r border-border-subtle px-3 py-5 flex flex-col">
      <div class="px-2 mb-5 text-sm font-medium text-text-secondary tracking-tight">Trace</div>
      <nav class="flex flex-col gap-1 flex-1">
        <RouterLink
          v-for="item in nav"
          :key="item.to"
          :to="item.to"
          class="flex items-center gap-2.5 px-2 py-1.5 rounded-md text-[13px] transition-colors duration-100"
          :class="isActive(item.to) ? 'bg-active text-text-primary font-medium' : 'text-text-secondary hover:bg-hover'"
        >
          <span class="text-sm opacity-70">{{ item.icon }}</span>
          {{ item.label }}
        </RouterLink>
      </nav>
      <button
        @click="logout"
        class="px-2 py-1.5 text-[12px] text-text-tertiary hover:text-text-secondary transition-colors text-left"
      >退出登录</button>
    </aside>
    <main
      class="flex-1 min-w-0 px-10 py-8"
      :class="route.path === '/terminals' ? 'flex flex-col h-screen overflow-hidden' : 'max-w-4xl'"
    >
      <RouterView />
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
