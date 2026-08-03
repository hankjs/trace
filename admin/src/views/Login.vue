<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { api, setToken } from '../composables/api'

const router = useRouter()
const username = ref('')
const password = ref('')
const error = ref('')
const loading = ref(false)
const faviconUrl = `${import.meta.env.BASE_URL}favicon.png`

async function handleLogin() {
  if (!username.value || !password.value) return
  loading.value = true
  error.value = ''

  try {
    const res = await api.login(username.value, password.value)
    const json = await res.json()
    if (json.code === 0) {
      setToken(json.data.token)
      router.push('/')
    } else {
      error.value = json.msg || 'Invalid credentials'
    }
  } catch {
    error.value = 'Network error'
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="min-h-screen flex items-center justify-center bg-surface">
    <div class="w-72">
      <div class="flex items-center gap-2.5 mb-8">
        <img :src="faviconUrl" alt="" width="28" height="28" class="rounded-[7px] shrink-0" />
        <span class="text-sm font-medium text-text-secondary tracking-tight">Trace Admin</span>
      </div>

      <form @submit.prevent="handleLogin" class="space-y-3">
        <input
          v-model="username"
          type="text"
          placeholder="Username"
          autocomplete="username"
          class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
        />
        <input
          v-model="password"
          type="password"
          placeholder="Password"
          autocomplete="current-password"
          class="w-full bg-transparent border border-border rounded-md px-3 py-2 text-[13px] placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
        />
        <button
          type="submit"
          :disabled="loading"
          class="w-full px-3.5 py-2 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 disabled:opacity-40 transition-opacity"
        >{{ loading ? 'Signing in...' : 'Sign in' }}</button>
      </form>

      <div v-if="error" class="mt-3 text-[12px] text-red-500">{{ error }}</div>
    </div>
  </div>
</template>
