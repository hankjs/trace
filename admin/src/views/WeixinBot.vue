<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import QRCode from 'qrcode'
import { api, type WeixinAccount, type WeixinBinding } from '../composables/api'

const accounts = ref<WeixinAccount[]>([])
const bindings = ref<WeixinBinding[]>([])
const loading = ref(true)
const bindingsLoading = ref(true)

// 扫码登录状态
const showLogin = ref(false)
const loginId = ref('')
const qrcodeImage = ref('')
const loginStatus = ref<'waiting' | 'scanned' | 'confirmed' | 'expired' | 'error' | 'loading'>('loading')
const loginMsg = ref('')
let pollTimer: ReturnType<typeof setTimeout> | null = null

async function load() {
  loading.value = true
  try {
    accounts.value = await api.listWeixinAccounts()
  } catch (e) { /* ignore */ }
  loading.value = false
}

async function loadBindings() {
  bindingsLoading.value = true
  try {
    bindings.value = await api.listWeixinBindings()
  } catch (e) { /* ignore */ }
  bindingsLoading.value = false
}

async function toggleEnabled(a: WeixinAccount) {
  try {
    await api.updateWeixinAccount(a.id, { enabled: !a.enabled })
  } catch (e) { /* ignore */ }
  await load()
}

async function remove(id: string) {
  if (!confirm('确定删除该机器人账号？')) return
  await api.deleteWeixinAccount(id)
  await load()
}

async function startLogin() {
  stopPolling()
  showLogin.value = true
  loginStatus.value = 'loading'
  loginMsg.value = ''
  qrcodeImage.value = ''
  try {
    const res = await api.weixinLoginStart()
    loginId.value = res.login_id
    qrcodeImage.value = await QRCode.toDataURL(res.qrcode_url, { width: 220, margin: 1 })
    loginStatus.value = 'waiting'
    poll()
  } catch (e: any) {
    loginStatus.value = 'error'
    loginMsg.value = e?.message || '获取二维码失败'
  }
}

function stopPolling() {
  if (pollTimer) {
    clearTimeout(pollTimer)
    pollTimer = null
  }
}

async function poll() {
  stopPolling()
  if (!loginId.value) return
  try {
    const res = await api.weixinLoginStatus(loginId.value)
    loginStatus.value = res.status
    if (res.status === 'confirmed') {
      stopPolling()
      showLogin.value = false
      await load()
      return
    }
    if (res.status === 'expired' || res.status === 'error') {
      loginMsg.value = res.msg || (res.status === 'expired' ? '二维码已过期' : '登录出错')
      return
    }
    pollTimer = setTimeout(poll, 1500)
  } catch (e: any) {
    loginStatus.value = 'error'
    loginMsg.value = e?.message || '查询登录状态失败'
  }
}

function cancelLogin() {
  stopPolling()
  showLogin.value = false
}

async function unbind(id: string) {
  if (!confirm('确定解除该绑定？')) return
  await api.deleteWeixinBinding(id)
  await loadBindings()
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
    await api.weixinSend(sendBindingId.value, sendText.value.trim())
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
})

onUnmounted(stopPolling)
</script>

<template>
  <div>
    <div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <h1 class="text-lg font-semibold text-text-primary">微信机器人</h1>
      <button type="button" @click="startLogin" class="min-h-10 self-start rounded-md bg-accent px-3 py-2 text-xs text-white hover:opacity-90 sm:min-h-0 sm:py-1.5">扫码添加</button>
    </div>

    <!-- 扫码登录 -->
    <div v-if="showLogin" class="mb-6 rounded-lg border border-border-subtle p-4">
      <div class="flex flex-col items-center gap-4 sm:flex-row sm:items-start sm:gap-6">
        <div class="flex h-[min(220px,70vw)] w-[min(220px,70vw)] items-center justify-center rounded border border-border-subtle bg-white">
          <img v-if="qrcodeImage" :src="qrcodeImage" class="h-full w-full" alt="微信登录二维码" />
          <span v-else class="text-xs text-text-tertiary">{{ loginStatus === 'loading' ? '获取二维码中...' : '—' }}</span>
        </div>
        <div class="w-full space-y-2 pt-0 text-center sm:pt-2 sm:text-left">
          <p v-if="loginStatus === 'waiting'" class="text-sm text-text-primary">请用微信扫码</p>
          <p v-else-if="loginStatus === 'scanned'" class="text-sm text-text-primary">已扫码，请在手机上确认</p>
          <p v-else-if="loginStatus === 'expired' || loginStatus === 'error'" class="text-sm text-red-400">{{ loginMsg }}</p>
          <p v-else-if="loginStatus === 'loading'" class="text-sm text-text-tertiary">正在初始化登录...</p>
          <div class="flex flex-wrap justify-center gap-2 pt-1 sm:justify-start">
            <button v-if="loginStatus === 'expired' || loginStatus === 'error'" type="button" @click="startLogin" class="min-h-10 rounded-md bg-accent px-3 py-2 text-xs text-white hover:opacity-90 sm:min-h-0 sm:py-1.5">重试</button>
            <button type="button" @click="cancelLogin" class="min-h-10 rounded-md border border-border-subtle px-3 py-2 text-xs text-text-secondary hover:bg-hover sm:min-h-0 sm:py-1.5">取消</button>
          </div>
        </div>
      </div>
    </div>

    <!-- 账号列表 -->
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="accounts.length === 0" class="mb-10 text-sm text-text-tertiary">暂无机器人账号，点击「扫码添加」。</div>
    <div v-else class="table-scroll mb-10">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border-subtle text-left text-xs text-text-tertiary">
            <th class="py-2 pr-3">Bot ID</th>
            <th class="py-2 pr-3">接口地址</th>
            <th class="py-2 pr-3">用户 ID</th>
            <th class="py-2 pr-3">状态</th>
            <th class="py-2 pr-3">创建时间</th>
            <th class="py-2">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="a in accounts" :key="a.id" class="border-b border-border-subtle">
            <td class="py-2 pr-3 font-mono text-xs text-text-primary">{{ a.ilink_bot_id }}</td>
            <td class="py-2 pr-3 font-mono text-xs text-text-secondary">{{ a.base_url }}</td>
            <td class="py-2 pr-3 font-mono text-xs text-text-secondary">{{ a.bot_user_id }}</td>
            <td class="py-2 pr-3">
              <button type="button" @click="toggleEnabled(a)" class="flex min-h-9 items-center gap-2">
                <span class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors" :class="a.enabled ? 'bg-green-500' : 'bg-border-subtle'">
                  <span class="inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform" :class="a.enabled ? 'translate-x-4' : 'translate-x-0.5'"></span>
                </span>
                <span :class="a.enabled ? 'text-green-500' : 'text-text-tertiary'">{{ a.enabled ? '已启用' : '已禁用' }}</span>
              </button>
              <p v-if="!a.enabled" class="mt-1 text-xs text-red-400">已失效，请重新扫码</p>
            </td>
            <td class="py-2 pr-3 text-xs text-text-tertiary">{{ a.created_at }}</td>
            <td class="py-2">
              <button type="button" @click="remove(a.id)" class="min-h-8 text-xs text-red-400 hover:underline">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 用户绑定 -->
    <h2 class="mb-3 text-sm font-semibold text-text-primary">用户绑定</h2>
    <div v-if="bindingsLoading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="bindings.length === 0" class="text-sm text-text-tertiary">暂无绑定。</div>
    <div v-else class="table-scroll">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border-subtle text-left text-xs text-text-tertiary">
            <th class="py-2 pr-3">用户名</th>
            <th class="py-2 pr-3">ilink 用户 ID</th>
            <th class="py-2 pr-3">绑定时间</th>
            <th class="py-2">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="b in bindings" :key="b.id" class="border-b border-border-subtle">
            <td class="py-2 pr-3 font-medium text-text-primary">{{ b.username }}</td>
            <td class="py-2 pr-3 font-mono text-xs text-text-secondary">{{ b.ilink_user_id }}</td>
            <td class="py-2 pr-3 text-xs text-text-tertiary">{{ b.created_at }}</td>
            <td class="py-2">
              <button type="button" @click="unbind(b.id)" class="min-h-8 text-xs text-red-400 hover:underline">解绑</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 主动发送 -->
    <template v-if="!bindingsLoading && bindings.length > 0">
      <h2 class="text-sm font-semibold text-text-primary mt-10 mb-3">主动发送</h2>
      <div class="p-4 border border-border-subtle rounded-lg space-y-3">
        <select v-model="sendBindingId" class="w-full px-3 py-1.5 text-sm bg-transparent border border-border-subtle rounded-md text-text-primary">
          <option value="" disabled>选择接收用户</option>
          <option v-for="b in bindings" :key="b.id" :value="b.id">{{ b.username }}（{{ b.ilink_user_id }}）</option>
        </select>
        <textarea
          v-model="sendText"
          rows="3"
          placeholder="要发送的消息内容"
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
        <p class="text-xs text-text-tertiary">仅支持给最近与机器人有过对话的绑定用户发送（依赖微信会话凭证）。</p>
      </div>
    </template>
  </div>
</template>
