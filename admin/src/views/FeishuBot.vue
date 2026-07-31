<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type FeishuAccount, type FeishuBinding } from '../composables/api'

const accounts = ref<FeishuAccount[]>([])
const bindings = ref<FeishuBinding[]>([])
const loading = ref(true)
const bindingsLoading = ref(true)

// 添加应用表单
const showAdd = ref(false)
const addName = ref('')
const addAppId = ref('')
const addAppSecret = ref('')
const adding = ref(false)
const addError = ref('')

async function load() {
  loading.value = true
  try {
    accounts.value = await api.listFeishuAccounts()
  } catch (e) { /* ignore */ }
  loading.value = false
}

async function loadBindings() {
  bindingsLoading.value = true
  try {
    bindings.value = await api.listFeishuBindings()
  } catch (e) { /* ignore */ }
  bindingsLoading.value = false
}

async function addAccount() {
  if (!addAppId.value.trim() || !addAppSecret.value.trim() || adding.value) return
  adding.value = true
  addError.value = ''
  try {
    await api.createFeishuAccount({
      name: addName.value.trim() || undefined,
      app_id: addAppId.value.trim(),
      app_secret: addAppSecret.value.trim(),
    })
    showAdd.value = false
    addName.value = ''
    addAppId.value = ''
    addAppSecret.value = ''
    await load()
  } catch (e: any) {
    addError.value = e?.message || '添加失败'
  } finally {
    adding.value = false
  }
}

async function toggleEnabled(a: FeishuAccount) {
  try {
    await api.updateFeishuAccount(a.id, { enabled: !a.enabled })
  } catch (e) { /* ignore */ }
  await load()
}

async function remove(id: string) {
  if (!confirm('确定删除该飞书应用？绑定与话题会话映射会一并删除。')) return
  await api.deleteFeishuAccount(id)
  await load()
  await loadBindings()
}

async function unbind(id: string) {
  if (!confirm('确定解除该绑定？')) return
  await api.deleteFeishuBinding(id)
  await loadBindings()
}

onMounted(() => {
  load()
  loadBindings()
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">飞书机器人</h1>
      <button @click="showAdd = !showAdd" class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90">添加应用</button>
    </div>

    <!-- 添加应用 -->
    <div v-if="showAdd" class="mb-6 p-4 border border-border-subtle rounded-lg space-y-3 max-w-xl">
      <p class="text-xs text-text-tertiary">
        飞书开放平台创建企业自建应用（启用机器人、事件与回调均选长连接、订阅 im.message.receive_v1 与 card.action.trigger）后填入凭证。
        保存前会先校验凭证有效性；保存成功即启动 WS 长连接。
      </p>
      <input v-model="addName" placeholder="备注名（可选）" class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary" />
      <input v-model="addAppId" placeholder="App ID（cli_ 开头）" class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary font-mono" />
      <input v-model="addAppSecret" type="password" placeholder="App Secret" class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary font-mono" />
      <div class="flex items-center gap-3">
        <button
          @click="addAccount"
          :disabled="!addAppId.trim() || !addAppSecret.trim() || adding"
          class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
        >{{ adding ? '校验并保存中...' : '保存' }}</button>
        <button @click="showAdd = false" class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-text-secondary hover:bg-hover">取消</button>
        <span v-if="addError" class="text-xs text-red-400">{{ addError }}</span>
      </div>
    </div>

    <!-- 账号列表 -->
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="accounts.length === 0" class="text-sm text-text-tertiary mb-10">暂无飞书应用，点击右上角"添加应用"。</div>
    <table v-else class="w-full text-sm mb-10">
      <thead>
        <tr class="text-left text-xs text-text-tertiary border-b border-border-subtle">
          <th class="py-2 pr-3">备注</th>
          <th class="py-2 pr-3">App ID</th>
          <th class="py-2 pr-3">状态（即长连接开关）</th>
          <th class="py-2 pr-3">创建时间</th>
          <th class="py-2">操作</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="a in accounts" :key="a.id" class="border-b border-border-subtle">
          <td class="py-2 pr-3 text-text-primary">{{ a.name || '—' }}</td>
          <td class="py-2 pr-3 text-text-secondary font-mono text-xs">{{ a.app_id }}</td>
          <td class="py-2 pr-3">
            <button @click="toggleEnabled(a)" class="flex items-center gap-2">
              <span class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors" :class="a.enabled ? 'bg-green-500' : 'bg-border-subtle'">
                <span class="inline-block h-3 w-3 rounded-full bg-white transition-transform" :class="a.enabled ? 'translate-x-3.5' : 'translate-x-0.5'"></span>
              </span>
              <span :class="a.enabled ? 'text-green-500' : 'text-text-tertiary'">{{ a.enabled ? '已启用' : '已禁用' }}</span>
            </button>
          </td>
          <td class="py-2 pr-3 text-text-tertiary text-xs">{{ a.created_at }}</td>
          <td class="py-2">
            <button @click="remove(a.id)" class="text-xs text-red-400 hover:underline">删除</button>
          </td>
        </tr>
      </tbody>
    </table>

    <!-- 用户绑定 -->
    <h2 class="text-sm font-semibold text-text-primary mb-3">用户绑定</h2>
    <p class="text-xs text-text-tertiary mb-3">用户在 Trace client「设置 → 飞书绑定」生成绑定码后，在飞书里向机器人发送 bind + 绑定码完成绑定。</p>
    <div v-if="bindingsLoading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="bindings.length === 0" class="text-sm text-text-tertiary">暂无绑定。</div>
    <table v-else class="w-full text-sm">
      <thead>
        <tr class="text-left text-xs text-text-tertiary border-b border-border-subtle">
          <th class="py-2 pr-3">用户名</th>
          <th class="py-2 pr-3">open_id</th>
          <th class="py-2 pr-3">绑定时间</th>
          <th class="py-2">操作</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="b in bindings" :key="b.id" class="border-b border-border-subtle">
          <td class="py-2 pr-3 text-text-primary font-medium">{{ b.username }}</td>
          <td class="py-2 pr-3 text-text-secondary font-mono text-xs">{{ b.open_id }}</td>
          <td class="py-2 pr-3 text-text-tertiary text-xs">{{ b.created_at }}</td>
          <td class="py-2">
            <button @click="unbind(b.id)" class="text-xs text-red-400 hover:underline">解绑</button>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
