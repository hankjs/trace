<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { useSession } from "../composables/useSession";

const { login } = useSession();
const router = useRouter();
const username = ref("");
const password = ref("");
const error = ref("");
const loading = ref(false);

async function handleLogin() {
  if (!username.value || !password.value) return;
  loading.value = true;
  error.value = "";

  try {
    const result = await login(username.value, password.value);
    if (!result.ok) {
      error.value = result.error || "用户名或密码错误";
    } else {
      router.push({ name: "sessions" });
    }
  } catch {
    error.value = "网络错误";
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="login-page">
    <div class="login-form">
      <span class="login-brand">Trace</span>

      <form @submit.prevent="handleLogin">
        <input
          v-model="username"
          type="text"
          placeholder="用户名"
          autocomplete="username"
          class="login-input"
        />
        <input
          v-model="password"
          type="password"
          placeholder="密码"
          autocomplete="current-password"
          class="login-input"
        />
        <button
          type="submit"
          :disabled="loading"
          class="login-btn"
        >{{ loading ? '...' : '登录' }}</button>
      </form>

      <div v-if="error" class="login-error">{{ error }}</div>
    </div>
  </div>
</template>

<style scoped>
.login-page {
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--color-surface-0);
}

.login-form {
  width: 260px;
}

.login-brand {
  display: block;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-secondary);
  margin-bottom: var(--space-8);
}

form {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.login-input {
  width: 100%;
  background: transparent;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  font-size: 13px;
  color: var(--color-text-primary);
  outline: none;
  transition: border-color var(--duration-fast);
}

.login-input::placeholder {
  color: var(--color-text-muted);
}

.login-input:focus {
  border-color: var(--color-accent);
}

.login-btn {
  width: 100%;
  padding: var(--space-2) var(--space-3);
  background: var(--color-text-primary);
  color: var(--color-surface-0);
  border: none;
  border-radius: var(--radius-md);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: opacity var(--duration-fast);
}

.login-btn:hover {
  opacity: 0.85;
}

.login-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.login-error {
  margin-top: var(--space-3);
  font-size: 12px;
  color: var(--color-error);
}
</style>
