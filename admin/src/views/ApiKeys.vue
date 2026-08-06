<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type ApiKey } from '../composables/api'

const keys = ref<ApiKey[]>([])
const newUsername = ref('')
const newName = ref('')
const showForm = ref(false)
const createError = ref('')
const createdKey = ref<{ name: string; key: string } | null>(null)
const copied = ref(false)

async function load() {
  keys.value = await api.listApiKeys()
}

async function createKey() {
  if (!newUsername.value || !newName.value) return
  createError.value = ''
  try {
    const res = await api.createApiKey(newUsername.value.trim(), newName.value.trim())
    createdKey.value = { name: res.name, key: res.key }
    copied.value = false
    newUsername.value = ''
    newName.value = ''
    showForm.value = false
    await load()
  } catch (err) {
    createError.value = err instanceof Error ? err.message : String(err)
  }
}

async function copyKey() {
  if (!createdKey.value) return
  await navigator.clipboard.writeText(createdKey.value.key)
  copied.value = true
}

function dismissCreatedKey() {
  createdKey.value = null
}

async function revokeKey(key: ApiKey) {
  if (!confirm(`确定吊销 API Key "${key.name}"？吊销后使用该 key 的请求将立即失效。`)) return
  await api.revokeApiKey(key.id)
  await load()
}

function formatTime(s: string | null) {
  if (!s) return '—'
  return new Date(s).toLocaleString()
}

onMounted(load)
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">API Keys</h1>
      <button
        @click="showForm = !showForm"
        class="text-[13px] text-accent hover:text-accent-hover transition-colors"
      >{{ showForm ? '取消' : '+ 新建 API Key' }}</button>
    </div>

    <div v-if="showForm" class="mb-8 space-y-3">
      <div class="grid grid-cols-2 gap-3">
        <input
          v-model="newUsername"
          placeholder="归属用户名"
          class="bg-transparent border border-border rounded-md px-3 py-2 text-[13px] placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
        />
        <input
          v-model="newName"
          placeholder="Key 名称（用途备注）"
          class="bg-transparent border border-border rounded-md px-3 py-2 text-[13px] placeholder:text-text-tertiary focus:outline-none focus:border-accent transition-colors"
        />
      </div>
      <div class="flex items-center gap-3">
        <button
          @click="createKey"
          class="px-3.5 py-1.5 bg-text-primary text-surface-raised text-[13px] rounded-md hover:opacity-80 transition-opacity"
        >创建</button>
        <span v-if="createError" class="text-[12px] text-red-500">{{ createError }}</span>
      </div>
    </div>

    <div v-if="createdKey" class="mb-8 border border-border rounded-md p-4 space-y-2">
      <div class="text-[13px] text-text-primary font-medium">API Key「{{ createdKey.name }}」已创建</div>
      <div class="flex items-center gap-2">
        <code class="flex-1 bg-active rounded px-3 py-2 text-[12px] text-text-primary font-mono break-all">{{ createdKey.key }}</code>
        <button
          @click="copyKey"
          class="shrink-0 px-3 py-1.5 text-[12px] text-accent hover:text-accent-hover transition-colors"
        >{{ copied ? '已复制' : '复制' }}</button>
      </div>
      <div class="flex items-center justify-between">
        <span class="text-[12px] text-text-tertiary">明文密钥只显示这一次，请立即复制保存，离开后将无法再次查看。</span>
        <button
          @click="dismissCreatedKey"
          class="shrink-0 text-[12px] text-text-tertiary hover:text-text-primary transition-colors"
        >我已保存，关闭</button>
      </div>
    </div>

    <div class="text-[12px] text-text-tertiary grid grid-cols-[1fr_140px_150px_150px_60px_60px] gap-2 px-2 pb-2 border-b border-border-subtle font-medium">
      <span>名称</span>
      <span>用户 ID</span>
      <span>创建时间</span>
      <span>最近使用</span>
      <span class="text-center">状态</span>
      <span></span>
    </div>

    <div class="divide-y divide-border-subtle">
      <div
        v-for="key in keys"
        :key="key.id"
        class="grid grid-cols-[1fr_140px_150px_150px_60px_60px] gap-2 items-center px-2 py-2.5"
        :class="key.revoked ? 'opacity-50' : ''"
      >
        <span class="text-[13px] text-text-primary truncate">{{ key.name }}</span>
        <span class="text-[12px] text-text-tertiary font-mono truncate">{{ key.user_id }}</span>
        <span class="text-[12px] text-text-tertiary">{{ formatTime(key.created_at) }}</span>
        <span class="text-[12px] text-text-tertiary">{{ formatTime(key.last_used_at) }}</span>
        <span class="text-center">
          <span
            class="text-[12px] px-2 py-0.5 rounded"
            :class="key.revoked ? 'text-text-tertiary' : 'bg-active text-text-primary'"
          >{{ key.revoked ? '已吊销' : '有效' }}</span>
        </span>
        <span class="text-right">
          <button
            v-if="!key.revoked"
            @click="revokeKey(key)"
            class="text-[12px] text-text-tertiary hover:text-red-500 transition-colors"
          >吊销</button>
        </span>
      </div>
    </div>

    <div v-if="!keys.length" class="py-12 text-center text-[13px] text-text-tertiary">暂无 API Key</div>
  </div>
</template>
