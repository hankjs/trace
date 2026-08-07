<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api, getToken } from '../api'
import NeuButton from '../components/ui/NeuButton.vue'
import NeuInput from '../components/ui/NeuInput.vue'

const router = useRouter()
const username = ref('')
const password = ref('')
const busy = ref(false)
const error = ref('')

onMounted(() => {
  if (getToken()) void router.replace('/')
})

async function signIn() {
  busy.value = true
  error.value = ''
  try {
    await api.login(username.value.trim(), password.value)
    await router.replace('/')
  } catch (err) {
    error.value = err instanceof Error ? err.message : '登录失败'
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="flex min-h-dvh items-center justify-center px-4 sm:px-6">
    <div class="w-full max-w-sm">
      <h1 class="text-2xl font-medium text-ink">App</h1>
      <p class="mt-2 text-sm leading-relaxed text-ink-2">
        用 Trace 账号登录，远程操作本机桌面终端。请先在 Trace 客户端开启「允许远程终端」。
      </p>

      <label class="mt-6 block text-sm text-ink-2">
        用户名
        <NeuInput
          v-model="username"
          type="text"
          autocomplete="username"
          class="mt-1 w-full"
          @keyup.enter="signIn"
        />
      </label>

      <label class="mt-3 block text-sm text-ink-2">
        密码
        <NeuInput
          v-model="password"
          type="password"
          autocomplete="current-password"
          class="mt-1 w-full"
          @keyup.enter="signIn"
        />
      </label>

      <NeuButton
        variant="primary"
        class="mt-4 w-full"
        :disabled="busy || !username.trim() || !password"
        @click="signIn"
      >
        {{ busy ? '登录中…' : '登录' }}
      </NeuButton>

      <p v-if="error" class="mt-3 text-sm text-danger">{{ error }}</p>
    </div>
  </div>
</template>
