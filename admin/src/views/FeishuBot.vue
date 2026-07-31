<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { api, type FeishuAccount, type FeishuBinding, type User } from '../composables/api'

const accounts = ref<FeishuAccount[]>([])
const bindings = ref<FeishuBinding[]>([])
const loading = ref(true)
const bindingsLoading = ref(true)
const users = ref<User[]>([])
const usersLoading = ref(true)

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

async function loadUsers() {
  usersLoading.value = true
  try {
    users.value = await api.listUsers()
    if (!bindUserId.value && users.value.length > 0) {
      bindUserId.value = users.value[0].id
    }
  } catch (e: any) {
    bindCodeError.value = e?.message || '用户列表加载失败'
  }
  usersLoading.value = false
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
  if (!confirm('确定删除该飞书应用？绑定与话题会话映射会一并删除，已留档的聊天记录将继续保留。')) return
  await api.deleteFeishuAccount(id)
  await load()
  await loadBindings()
}

async function unbind(id: string) {
  if (!confirm('确定解除该绑定？')) return
  await api.deleteFeishuBinding(id)
  await loadBindings()
}

// 管理员生成绑定码
const bindUserId = ref('')
const bindCode = ref('')
const bindCodeUsername = ref('')
const bindCodeExpiresAt = ref(0)
const bindCodeNow = ref(Date.now())
const bindCodeGenerating = ref(false)
const bindCodeCopied = ref(false)
const bindCodeError = ref('')
let bindCodeTimer: ReturnType<typeof setInterval> | undefined
let copiedTimer: ReturnType<typeof setTimeout> | undefined

const bindCodeRemainingSeconds = computed(() =>
  Math.max(0, Math.ceil((bindCodeExpiresAt.value - bindCodeNow.value) / 1000)),
)
const bindCodeCountdown = computed(() => {
  const minutes = Math.floor(bindCodeRemainingSeconds.value / 60)
  const seconds = bindCodeRemainingSeconds.value % 60
  return `${minutes}:${String(seconds).padStart(2, '0')}`
})
const bindCodeExpired = computed(() => !!bindCode.value && bindCodeRemainingSeconds.value === 0)

function startBindCodeTimer() {
  clearInterval(bindCodeTimer)
  bindCodeNow.value = Date.now()
  bindCodeTimer = setInterval(() => {
    bindCodeNow.value = Date.now()
    if (bindCodeNow.value >= bindCodeExpiresAt.value) clearInterval(bindCodeTimer)
  }, 1000)
}

async function generateBindCode() {
  if (!bindUserId.value || bindCodeGenerating.value) return
  bindCodeGenerating.value = true
  bindCodeError.value = ''
  bindCodeCopied.value = false
  try {
    const result = await api.createFeishuBindCode(bindUserId.value)
    bindCode.value = result.code
    bindCodeExpiresAt.value = result.expires_at
    bindCodeUsername.value = users.value.find((user) => user.id === bindUserId.value)?.username || ''
    startBindCodeTimer()
  } catch (e: any) {
    bindCodeError.value = e?.message || '绑定码生成失败'
  } finally {
    bindCodeGenerating.value = false
  }
}

async function copyBindCommand() {
  if (!bindCode.value || bindCodeExpired.value) return
  try {
    await navigator.clipboard.writeText(`bind ${bindCode.value}`)
    bindCodeCopied.value = true
    clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => { bindCodeCopied.value = false }, 2000)
  } catch {
    bindCodeError.value = '复制失败，请手动复制绑定命令'
  }
}

// 主动发送
const sendBindingId = ref('')
const sendText = ref('')
const sending = ref(false)
const sendResult = ref<{ ok: boolean; msg: string } | null>(null)

async function sendMessage() {
  if (!sendBindingId.value || !sendText.value.trim() || sending.value) return
  sending.value = true
  sendResult.value = null
  try {
    await api.feishuSend(sendBindingId.value, sendText.value.trim())
    sendResult.value = { ok: true, msg: '已发送' }
    sendText.value = ''
  } catch (e: any) {
    sendResult.value = { ok: false, msg: e?.message || '发送失败' }
  }
  sending.value = false
}

onMounted(() => {
  load()
  loadBindings()
  loadUsers()
})

onUnmounted(() => {
  clearInterval(bindCodeTimer)
  clearTimeout(copiedTimer)
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">飞书机器人</h1>
      <div class="flex items-center gap-3">
        <a
          href="https://open.feishu.cn/document/home/index"
          target="_blank"
          rel="noopener noreferrer"
          class="text-xs text-text-secondary hover:text-text-primary hover:underline"
        >飞书开放平台文档 ↗</a>
        <button @click="showAdd = !showAdd" class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90">添加应用</button>
      </div>
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
    <h2 class="text-sm font-semibold text-text-primary mb-2">用户绑定</h2>
    <p class="text-xs text-text-tertiary mb-3">选择 Trace 用户生成绑定码，然后在飞书里向机器人发送绑定命令。</p>
    <div class="flex items-end gap-3 max-w-xl mb-3">
      <label class="flex-1 text-xs text-text-secondary">
        Trace 用户
        <select
          v-model="bindUserId"
          :disabled="usersLoading || users.length === 0"
          class="mt-1 w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary disabled:opacity-40"
        >
          <option value="" disabled>{{ usersLoading ? '加载用户中...' : '选择用户' }}</option>
          <option v-for="user in users" :key="user.id" :value="user.id">{{ user.username }}</option>
        </select>
      </label>
      <button
        @click="generateBindCode"
        :disabled="!bindUserId || bindCodeGenerating"
        class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
      >{{ bindCodeGenerating ? '生成中...' : bindCode ? '重新生成' : '生成绑定码' }}</button>
    </div>
    <div v-if="bindCode" class="flex flex-wrap items-center gap-3 max-w-xl mb-4 px-3 py-2.5 border border-border-subtle rounded-md">
      <span class="text-xs text-text-tertiary">发送给机器人</span>
      <code class="px-2 py-1 rounded bg-active text-sm text-text-primary">bind {{ bindCode }}</code>
      <button
        @click="copyBindCommand"
        :disabled="bindCodeExpired"
        class="text-xs text-accent hover:text-accent-hover disabled:text-text-tertiary disabled:cursor-not-allowed"
      >{{ bindCodeCopied ? '已复制' : '复制命令' }}</button>
      <span class="ml-auto text-xs" :class="bindCodeExpired ? 'text-red-400' : 'text-text-tertiary'">
        {{ bindCodeUsername }} · {{ bindCodeExpired ? '已过期' : `${bindCodeCountdown} 后过期` }}
      </span>
    </div>
    <p v-if="bindCodeError" class="text-xs text-red-400 mb-3">{{ bindCodeError }}</p>
    <p v-else-if="!usersLoading && users.length === 0" class="text-xs text-text-tertiary mb-3">暂无 Trace 用户，请先在“用户”页面创建。</p>
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

    <!-- 主动发送 -->
    <template v-if="!bindingsLoading && bindings.length > 0">
      <h2 class="text-sm font-semibold text-text-primary mt-10 mb-3">主动发送</h2>
      <div class="p-4 border border-border-subtle rounded-lg space-y-3">
        <select v-model="sendBindingId" class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary">
          <option value="" disabled>选择接收用户</option>
          <option v-for="b in bindings" :key="b.id" :value="b.id">{{ b.username }}（{{ b.open_id }}）</option>
        </select>
        <textarea
          v-model="sendText"
          rows="3"
          placeholder="要发送的消息内容（以单聊形式发送给该用户）"
          class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary resize-y"
          @keydown.meta.enter="sendMessage"
          @keydown.ctrl.enter="sendMessage"
        ></textarea>
        <div class="flex items-center gap-3">
          <button
            @click="sendMessage"
            :disabled="!sendBindingId || !sendText.trim() || sending"
            class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
          >{{ sending ? '发送中...' : '发送' }}</button>
          <span v-if="sendResult" class="text-xs" :class="sendResult.ok ? 'text-green-500' : 'text-red-400'">{{ sendResult.msg }}</span>
        </div>
        <p class="text-xs text-text-tertiary">以机器人单聊形式发送；接收用户需在应用的可用范围内。</p>
      </div>
    </template>
  </div>
</template>
