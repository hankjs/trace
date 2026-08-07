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
    <div class="mb-6 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
      <h1 class="text-lg font-semibold text-text-primary">API Keys</h1>
      <button
        type="button"
        class="min-h-10 self-start text-[13px] text-accent transition-colors hover:text-accent-hover sm:min-h-0"
        @click="showForm = !showForm"
      >{{ showForm ? '取消' : '+ 新建 API Key' }}</button>
    </div>

    <div v-if="showForm" class="mb-8 space-y-3">
      <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <input
          v-model="newUsername"
          placeholder="归属用户名"
          class="rounded-md border border-border bg-transparent px-3 py-2 text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
        />
        <input
          v-model="newName"
          placeholder="Key 名称（用途备注）"
          class="rounded-md border border-border bg-transparent px-3 py-2 text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
        />
      </div>
      <div class="flex flex-wrap items-center gap-3">
        <button
          type="button"
          class="min-h-10 rounded-md bg-text-primary px-3.5 py-2 text-[13px] text-surface-raised transition-opacity hover:opacity-80 sm:min-h-0 sm:py-1.5"
          @click="createKey"
        >创建</button>
        <span v-if="createError" class="text-[12px] text-red-500">{{ createError }}</span>
      </div>
    </div>

    <div v-if="createdKey" class="mb-8 space-y-2 rounded-md border border-border p-4">
      <div class="text-[13px] font-medium text-text-primary">API Key「{{ createdKey.name }}」已创建</div>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <code class="flex-1 break-all rounded bg-active px-3 py-2 font-mono text-[12px] text-text-primary">{{ createdKey.key }}</code>
        <button
          type="button"
          class="min-h-10 shrink-0 px-3 py-1.5 text-[12px] text-accent transition-colors hover:text-accent-hover sm:min-h-0"
          @click="copyKey"
        >{{ copied ? '已复制' : '复制' }}</button>
      </div>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <span class="text-[12px] text-text-tertiary">明文密钥只显示这一次，请立即复制保存，离开后将无法再次查看。</span>
        <button
          type="button"
          class="min-h-10 shrink-0 self-start text-[12px] text-text-tertiary transition-colors hover:text-text-primary sm:min-h-0"
          @click="dismissCreatedKey"
        >我已保存，关闭</button>
      </div>
    </div>

    <!-- 桌面表头 -->
    <div class="hidden text-[12px] font-medium text-text-tertiary lg:grid lg:grid-cols-[1fr_140px_150px_150px_60px_60px] lg:gap-2 lg:border-b lg:border-border-subtle lg:px-2 lg:pb-2">
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
        class="flex flex-col gap-1.5 py-3 lg:grid lg:grid-cols-[1fr_140px_150px_150px_60px_60px] lg:items-center lg:gap-2 lg:px-2 lg:py-2.5"
        :class="key.revoked ? 'opacity-50' : ''"
      >
        <div class="flex items-center justify-between gap-2">
          <span class="truncate text-[13px] text-text-primary">{{ key.name }}</span>
          <div class="flex shrink-0 items-center gap-2 lg:hidden">
            <span
              class="rounded px-2 py-0.5 text-[12px]"
              :class="key.revoked ? 'text-text-tertiary' : 'bg-active text-text-primary'"
            >{{ key.revoked ? '已吊销' : '有效' }}</span>
            <button
              v-if="!key.revoked"
              type="button"
              class="min-h-9 text-[12px] text-text-tertiary transition-colors hover:text-red-500"
              @click="revokeKey(key)"
            >吊销</button>
          </div>
        </div>
        <span class="truncate font-mono text-[12px] text-text-tertiary">{{ key.user_id }}</span>
        <span class="text-[12px] text-text-tertiary">
          <span class="lg:hidden">创建 </span>{{ formatTime(key.created_at) }}
        </span>
        <span class="text-[12px] text-text-tertiary">
          <span class="lg:hidden">最近使用 </span>{{ formatTime(key.last_used_at) }}
        </span>
        <span class="hidden text-center lg:block">
          <span
            class="rounded px-2 py-0.5 text-[12px]"
            :class="key.revoked ? 'text-text-tertiary' : 'bg-active text-text-primary'"
          >{{ key.revoked ? '已吊销' : '有效' }}</span>
        </span>
        <span class="hidden text-right lg:block">
          <button
            v-if="!key.revoked"
            type="button"
            class="text-[12px] text-text-tertiary transition-colors hover:text-red-500"
            @click="revokeKey(key)"
          >吊销</button>
        </span>
      </div>
    </div>

    <div v-if="!keys.length" class="py-12 text-center text-[13px] text-text-tertiary">暂无 API Key</div>
  </div>
</template>
