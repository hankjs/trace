<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { Activity, ShieldCheck } from 'lucide-vue-next'
import { api, setAuth } from '../api'
import ThemeToggle from '../components/ThemeToggle.vue'

const router = useRouter()
const username = ref('')
const password = ref('')
const error = ref('')
const loading = ref(false)

async function submit() {
  if (!username.value || !password.value) {
    error.value = '请输入用户名和密码'
    return
  }
  loading.value = true
  error.value = ''
  try {
    const res = await api.login(username.value, password.value)
    setAuth(res.token, res.username)
    router.push('/')
  } catch (e) {
    error.value = e instanceof Error ? e.message : '登录失败'
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="app-shell flex h-screen min-h-[480px] flex-col overflow-hidden bg-surface">
    <header class="flex h-11 shrink-0 items-center justify-between border-b border-border bg-workbench px-3">
      <div class="flex items-center gap-2.5">
        <span class="flex h-7 w-7 items-center justify-center rounded bg-accent text-on-accent">
          <Activity :size="17" />
        </span>
        <div>
          <p class="text-sm font-semibold leading-4">Trace Quant</p>
          <p class="text-[10px] leading-4 text-text-tertiary">日频研究工作台</p>
        </div>
      </div>
      <ThemeToggle />
    </header>

    <main class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-4 py-10">
      <div class="w-full max-w-sm">
        <div class="mb-7">
          <h1 class="text-xl font-semibold">登录研究工作区</h1>
          <p class="mt-1 text-sm leading-6 text-text-secondary">使用 Trace 账号访问行情、选股、策略回测与手工持仓记录。</p>
        </div>

        <form class="space-y-4" @submit.prevent="submit">
          <label class="block text-sm">
            <span class="mb-1.5 block text-xs font-medium text-text-secondary">用户名</span>
            <input
              v-model="username"
              type="text"
              autocomplete="username"
              autofocus
              class="h-9 w-full rounded-md border border-border px-3"
            />
          </label>
          <label class="block text-sm">
            <span class="mb-1.5 block text-xs font-medium text-text-secondary">密码</span>
            <input
              v-model="password"
              type="password"
              autocomplete="current-password"
              class="h-9 w-full rounded-md border border-border px-3"
            />
          </label>
          <p v-if="error" role="alert" class="rounded-md bg-danger-soft px-3 py-2 text-sm text-up">{{ error }}</p>
          <button
            type="submit"
            :disabled="loading"
            class="flex h-9 w-full items-center justify-center rounded-md bg-accent px-4 text-sm font-medium text-on-accent transition-colors hover:bg-accent-hover disabled:opacity-50"
          >
            {{ loading ? '登录中…' : '登录' }}
          </button>
        </form>

        <div class="mt-7 flex items-start gap-2 border-t border-border pt-4 text-xs leading-5 text-text-tertiary">
          <ShieldCheck :size="15" class="mt-0.5 shrink-0 text-accent" />
          <p>系统仅提供日频研究、模拟回测与手工记账，不连接券商或提交订单。</p>
        </div>
      </div>
    </main>

    <footer class="flex h-6 shrink-0 items-center justify-between border-t border-border bg-workbench px-3 text-[10px] text-text-tertiary">
      <span>Trace Quant</span>
      <span>研究与模拟 · 外部手工决策</span>
    </footer>
  </div>
</template>
