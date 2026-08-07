<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type User } from '../composables/api'

const users = ref<User[]>([])
const newUsername = ref('')
const newPassword = ref('')
const newCanAdmin = ref(false)
const newCanClient = ref(true)
const showForm = ref(false)

async function load() {
  users.value = await api.listUsers()
}

async function createUser() {
  if (!newUsername.value || !newPassword.value) return
  await api.createUser(newUsername.value, newPassword.value, newCanAdmin.value, newCanClient.value)
  newUsername.value = ''
  newPassword.value = ''
  newCanAdmin.value = false
  newCanClient.value = true
  showForm.value = false
  await load()
}

async function togglePermission(user: User, field: 'can_login_admin' | 'can_login_client') {
  const update = { [field]: !user[field] }
  await api.updateUser(user.id, update)
  await load()
}

async function deleteUser(user: User) {
  if (!confirm(`确定删除用户 "${user.username}"？`)) return
  await api.deleteUser(user.id)
  await load()
}

onMounted(load)
</script>

<template>
  <div>
    <div class="mb-6 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
      <h1 class="text-lg font-semibold text-text-primary">用户管理</h1>
      <button
        type="button"
        class="min-h-10 self-start text-[13px] text-accent transition-colors hover:text-accent-hover sm:min-h-0"
        @click="showForm = !showForm"
      >{{ showForm ? '取消' : '+ 新建用户' }}</button>
    </div>

    <div v-if="showForm" class="mb-8 space-y-3">
      <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <input
          v-model="newUsername"
          placeholder="用户名"
          class="rounded-md border border-border bg-transparent px-3 py-2 text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
        />
        <input
          v-model="newPassword"
          type="password"
          placeholder="密码"
          class="rounded-md border border-border bg-transparent px-3 py-2 text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none"
        />
      </div>
      <div class="flex flex-wrap items-center gap-5 text-[13px] text-text-secondary">
        <label class="flex min-h-10 items-center gap-1.5 cursor-pointer sm:min-h-0">
          <input v-model="newCanAdmin" type="checkbox" class="rounded" />
          管理后台
        </label>
        <label class="flex min-h-10 items-center gap-1.5 cursor-pointer sm:min-h-0">
          <input v-model="newCanClient" type="checkbox" class="rounded" />
          客户端
        </label>
      </div>
      <button
        type="button"
        class="min-h-10 rounded-md bg-text-primary px-3.5 py-2 text-[13px] text-surface-raised transition-opacity hover:opacity-80 sm:min-h-0 sm:py-1.5"
        @click="createUser"
      >创建</button>
    </div>

    <!-- 桌面表头 -->
    <div class="hidden text-[12px] font-medium text-text-tertiary md:grid md:grid-cols-[1fr_80px_80px_60px] md:gap-2 md:border-b md:border-border-subtle md:px-2 md:pb-2">
      <span>用户名</span>
      <span class="text-center">管理后台</span>
      <span class="text-center">客户端</span>
      <span></span>
    </div>

    <div class="divide-y divide-border-subtle">
      <div
        v-for="user in users"
        :key="user.id"
        class="flex flex-col gap-2 px-0 py-3 md:grid md:grid-cols-[1fr_80px_80px_60px] md:items-center md:gap-2 md:px-2 md:py-2.5"
      >
        <span class="text-[13px] text-text-primary">{{ user.username }}</span>
        <div class="flex flex-wrap items-center gap-2 md:contents">
          <span class="flex items-center gap-1.5 md:justify-center">
            <span class="text-[11px] text-text-tertiary md:hidden">管理后台</span>
            <button
              type="button"
              class="min-h-9 rounded px-2.5 py-1 text-[12px] transition-colors md:min-h-0 md:py-0.5"
              :class="user.can_login_admin ? 'bg-active text-text-primary' : 'text-text-tertiary hover:bg-hover'"
              @click="togglePermission(user, 'can_login_admin')"
            >{{ user.can_login_admin ? '是' : '否' }}</button>
          </span>
          <span class="flex items-center gap-1.5 md:justify-center">
            <span class="text-[11px] text-text-tertiary md:hidden">客户端</span>
            <button
              type="button"
              class="min-h-9 rounded px-2.5 py-1 text-[12px] transition-colors md:min-h-0 md:py-0.5"
              :class="user.can_login_client ? 'bg-active text-text-primary' : 'text-text-tertiary hover:bg-hover'"
              @click="togglePermission(user, 'can_login_client')"
            >{{ user.can_login_client ? '是' : '否' }}</button>
          </span>
          <span class="md:text-right">
            <button
              type="button"
              class="min-h-9 text-[12px] text-text-tertiary transition-colors hover:text-red-500 md:min-h-0"
              @click="deleteUser(user)"
            >删除</button>
          </span>
        </div>
      </div>
    </div>

    <div v-if="!users.length" class="py-12 text-center text-[13px] text-text-tertiary">暂无用户</div>
  </div>
</template>
