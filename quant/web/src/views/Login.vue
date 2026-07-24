<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { api, setAuth } from '../api'

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
  <div class="flex min-h-[60vh] items-center justify-center">
    <form class="w-80 rounded-lg border border-border bg-surface-raised p-6" @submit.prevent="submit">
      <h1 class="mb-6 text-center text-lg font-semibold">quant 量化看板</h1>
      <label class="mb-1 block text-sm text-text-secondary">用户名</label>
      <input
        v-model="username"
        type="text"
        autocomplete="username"
        class="mb-4 w-full rounded-md border border-border px-2 py-1.5"
      />
      <label class="mb-1 block text-sm text-text-secondary">密码</label>
      <input
        v-model="password"
        type="password"
        autocomplete="current-password"
        class="mb-4 w-full rounded-md border border-border px-2 py-1.5"
      />
      <p v-if="error" class="mb-4 text-sm text-red-500">{{ error }}</p>
      <button
        type="submit"
        :disabled="loading"
        class="w-full rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover disabled:opacity-50"
      >
        {{ loading ? '登录中…' : '登录' }}
      </button>
    </form>
  </div>
</template>
