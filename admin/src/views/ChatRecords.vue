<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import AgentTracePanel from '../components/AgentTracePanel.vue'
import { api, type AgentEventRecord, type ChannelConversation, type ChannelMessage } from '../composables/api'
import { backendSummary, backendTone } from '../utils/agentBackend'

const conversations = ref<ChannelConversation[]>([])
const selected = ref<ChannelConversation | null>(null)
const messages = ref<ChannelMessage[]>([])
const totalConversations = ref(0)
const totalMessages = ref(0)
const conversationPage = ref(1)
const messagePage = ref(0)
const searchInput = ref('')
const appliedSearch = ref('')
const conversationsLoading = ref(true)
const messagesLoading = ref(false)
const traceEvents = ref<AgentEventRecord[]>([])
const traceLoading = ref(false)
const traceError = ref('')
const detailMode = ref<'messages' | 'trace'>('messages')
const error = ref('')

const conversationPerPage = 30
const messagePerPage = 100

const hasOlderMessages = computed(() => messages.value.length < totalMessages.value)

function formatTime(value: string) {
  return new Date(value).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function shortId(value: string | null | undefined) {
  if (!value) return '—'
  return value.length > 24 ? `${value.slice(0, 12)}…${value.slice(-8)}` : value
}

function conversationLabel(item: ChannelConversation) {
  if (item.username) return item.username
  if (item.peer_id) return shortId(item.peer_id)
  return shortId(item.conversation_id)
}

function topicLabel(item: Pick<ChannelConversation, 'topic_id'>) {
  return item.topic_id === 'main' ? '主会话' : `话题 ${shortId(item.topic_id)}`
}

function directionLabel(direction: ChannelMessage['direction']) {
  return direction === 'inbound' ? '用户' : 'Trace'
}

async function loadConversations() {
  conversationsLoading.value = true
  error.value = ''
  try {
    const result = await api.chatRecordConversations(
      conversationPage.value,
      conversationPerPage,
      appliedSearch.value,
    )
    conversations.value = result.data
    totalConversations.value = result.total
    if (selected.value) {
      const current = result.data.find(
        (item) => item.account_id === selected.value?.account_id
          && item.conversation_id === selected.value?.conversation_id
          && item.topic_id === selected.value?.topic_id,
      )
      if (current) selected.value = current
      else {
        selected.value = null
        messages.value = []
        totalMessages.value = 0
      }
    }
  } catch (e: any) {
    error.value = e?.message || '聊天记录加载失败'
  } finally {
    conversationsLoading.value = false
  }
}

async function selectConversation(item: ChannelConversation) {
  selected.value = item
  messages.value = []
  traceEvents.value = []
  traceError.value = ''
  totalMessages.value = 0
  messagePage.value = 0
  detailMode.value = item.session_id ? 'trace' : 'messages'
  await Promise.all([
    loadMessages(1, true),
    item.session_id ? loadTrace(item.session_id) : Promise.resolve(),
  ])
}

async function loadTrace(sessionId: string) {
  traceLoading.value = true
  traceError.value = ''
  try {
    traceEvents.value = await api.sessionEvents(sessionId)
  } catch (e: any) {
    traceError.value = e?.message || '调用链加载失败'
  } finally {
    traceLoading.value = false
  }
}

async function loadMessages(page: number, replace = false) {
  if (!selected.value || messagesLoading.value) return
  messagesLoading.value = true
  error.value = ''
  try {
    const result = await api.chatRecordMessages(selected.value, page, messagePerPage)
    messages.value = replace ? result.data : [...result.data, ...messages.value]
    totalMessages.value = result.total
    messagePage.value = page
  } catch (e: any) {
    error.value = e?.message || '消息加载失败'
  } finally {
    messagesLoading.value = false
  }
}

function applySearch() {
  appliedSearch.value = searchInput.value.trim()
  conversationPage.value = 1
  loadConversations()
}

function clearSearch() {
  searchInput.value = ''
  applySearch()
}

function previousPage() {
  if (conversationPage.value <= 1) return
  conversationPage.value -= 1
  loadConversations()
}

function nextPage() {
  if (conversationPage.value * conversationPerPage >= totalConversations.value) return
  conversationPage.value += 1
  loadConversations()
}

onMounted(loadConversations)
</script>

<template>
  <div class="flex h-full min-h-0 flex-col sm:min-h-[480px] lg:min-h-[620px]">
    <div class="mb-4 flex flex-col gap-3 sm:mb-5 sm:flex-row sm:flex-wrap sm:items-start sm:justify-between">
      <div>
        <h1 class="text-lg font-semibold text-text-primary">聊天记录</h1>
        <p class="mt-1 text-xs text-text-tertiary">飞书渠道 · {{ totalConversations }} 个会话</p>
      </div>
      <div class="flex w-full flex-col gap-2 sm:w-auto sm:flex-row sm:flex-wrap sm:items-center">
        <select
          aria-label="聊天渠道"
          disabled
          class="h-10 w-full rounded-md border border-border bg-transparent px-2 text-xs text-text-secondary disabled:opacity-70 sm:h-8 sm:w-auto"
        >
          <option>飞书</option>
        </select>
        <div class="flex h-10 min-w-0 flex-1 items-center rounded-md border border-border bg-transparent sm:h-8 sm:flex-none">
          <input
            v-model="searchInput"
            placeholder="搜索会话、用户或内容"
            class="min-w-0 flex-1 bg-transparent px-2.5 text-xs text-text-primary outline-none placeholder:text-text-tertiary sm:w-48 sm:flex-none"
            @keydown.enter="applySearch"
          />
          <button
            v-if="searchInput"
            type="button"
            aria-label="清除搜索"
            class="min-h-8 px-2 text-xs text-text-tertiary hover:text-text-secondary"
            @click="clearSearch"
          >×</button>
          <button
            type="button"
            class="min-h-8 border-l border-border-subtle px-2.5 text-xs text-text-secondary hover:text-text-primary"
            @click="applySearch"
          >搜索</button>
        </div>
        <button
          type="button"
          class="h-10 w-full rounded-md border border-border px-2.5 text-xs text-text-secondary transition-colors hover:bg-hover hover:text-text-primary sm:h-8 sm:w-auto"
          @click="loadConversations"
        >刷新</button>
      </div>
    </div>

    <p v-if="error" class="mb-3 rounded-md border border-red-300/50 bg-red-50 px-3 py-2 text-xs text-red-600">{{ error }}</p>

    <div class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border lg:flex-row">
      <!-- 选中会话时移动端可隐藏列表，给消息区更大空间 -->
      <aside
        class="flex max-h-[40vh] w-full shrink-0 flex-col border-b border-border lg:max-h-none lg:w-72 lg:border-b-0 lg:border-r"
        :class="selected ? 'hidden lg:flex' : 'flex'"
      >
        <div class="flex h-10 items-center justify-between border-b border-border-subtle px-3">
          <span class="text-xs font-medium text-text-secondary">会话</span>
          <span class="font-mono text-[11px] text-text-tertiary">{{ totalConversations }}</span>
        </div>
        <div v-if="conversationsLoading" class="px-3 py-8 text-xs text-text-tertiary">加载中...</div>
        <div v-else-if="!conversations.length" class="px-3 py-8 text-xs text-text-tertiary">暂无记录</div>
        <div v-else class="min-h-0 flex-1 divide-y divide-border-subtle overflow-y-auto">
          <button
            v-for="item in conversations"
            :key="`${item.account_id}:${item.conversation_id}:${item.topic_id}`"
            class="block w-full px-3 py-2.5 text-left transition-colors hover:bg-hover"
            :class="selected && selected.account_id === item.account_id && selected.conversation_id === item.conversation_id && selected.topic_id === item.topic_id ? 'bg-active' : ''"
            @click="selectConversation(item)"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="truncate text-[13px] font-medium text-text-primary">{{ conversationLabel(item) }}</span>
              <span class="shrink-0 font-mono text-[10px] text-text-tertiary">{{ item.message_count }}</span>
            </div>
            <div class="mt-1 flex items-center justify-between gap-2 text-[11px] text-text-tertiary">
              <span class="truncate">{{ topicLabel(item) }} · {{ item.account_name }}</span>
              <span class="shrink-0">{{ formatTime(item.last_message_at) }}</span>
            </div>
            <div v-if="item.session_id" class="mt-1 text-[11px]" :class="backendTone(item.agent_provider)">
              {{ backendSummary(item.agent_provider, item.agent_model) }}
            </div>
            <p class="mt-1 truncate text-xs text-text-secondary">{{ item.last_content || '—' }}</p>
          </button>
        </div>
        <div class="flex items-center justify-between border-t border-border-subtle px-3 py-2 text-[11px] text-text-tertiary">
          <button class="hover:text-text-secondary disabled:opacity-30" :disabled="conversationPage <= 1" @click="previousPage">上一页</button>
          <span class="font-mono tabular-nums">{{ conversationPage }}</span>
          <button class="hover:text-text-secondary disabled:opacity-30" :disabled="conversationPage * conversationPerPage >= totalConversations" @click="nextPage">下一页</button>
        </div>
      </aside>

      <section class="flex min-h-0 min-w-0 flex-1 flex-col" :class="!selected ? 'hidden lg:flex' : ''">
        <template v-if="selected">
          <header class="border-b border-border-subtle px-3 py-3 sm:px-4">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <button
                    type="button"
                    class="flex size-9 shrink-0 items-center justify-center rounded-md text-text-secondary hover:bg-hover lg:hidden"
                    aria-label="返回会话列表"
                    @click="selected = null; messages = []; traceEvents = []"
                  >
                    <svg class="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true">
                      <path d="M15 18 9 12l6-6" />
                    </svg>
                  </button>
                  <div class="min-w-0">
                    <h2 class="truncate text-[13px] font-semibold text-text-primary">{{ conversationLabel(selected) }}</h2>
                    <p class="mt-1 truncate font-mono text-[11px] text-text-tertiary">{{ selected.conversation_id }} · {{ topicLabel(selected) }}</p>
                    <p v-if="selected.session_id" class="mt-1 truncate text-[11px]" :class="backendTone(selected.agent_provider)">
                      执行后端：{{ backendSummary(selected.agent_provider, selected.agent_model) }}
                    </p>
                  </div>
                </div>
              </div>
              <div class="flex w-full shrink-0 flex-wrap items-center gap-2 sm:w-auto">
                <div class="flex h-9 flex-1 items-center rounded border border-border-subtle bg-surface sm:h-7 sm:flex-none" role="tablist" aria-label="记录视图">
                  <button
                    type="button"
                    class="h-full flex-1 px-2.5 text-[11px] transition-colors sm:flex-none"
                    :class="detailMode === 'messages' ? 'bg-active font-medium text-text-primary' : 'text-text-tertiary hover:text-text-secondary'"
                    role="tab"
                    :aria-selected="detailMode === 'messages'"
                    @click="detailMode = 'messages'"
                  >消息</button>
                  <button
                    v-if="selected.session_id"
                    type="button"
                    class="h-full flex-1 border-l border-border-subtle px-2.5 text-[11px] transition-colors sm:flex-none"
                    :class="detailMode === 'trace' ? 'bg-active font-medium text-text-primary' : 'text-text-tertiary hover:text-text-secondary'"
                    role="tab"
                    :aria-selected="detailMode === 'trace'"
                    @click="detailMode = 'trace'"
                  >调用链</button>
                </div>
                <RouterLink
                  v-if="selected.session_id"
                  :to="`/sessions/${selected.session_id}`"
                  class="min-h-9 text-[11px] text-accent hover:text-accent-hover sm:min-h-0"
                >Session ↗</RouterLink>
              </div>
            </div>
          </header>
          <div v-if="detailMode === 'messages'" class="min-h-0 flex-1 overflow-y-auto" role="tabpanel">
            <div v-if="hasOlderMessages" class="border-b border-border-subtle px-4 py-2 text-center">
              <button type="button" class="min-h-9 text-[11px] text-accent hover:text-accent-hover disabled:opacity-40" :disabled="messagesLoading" @click="loadMessages(messagePage + 1)">
                {{ messagesLoading ? '加载中...' : '加载更早记录' }}
              </button>
            </div>
            <div v-if="messagesLoading && !messages.length" class="px-4 py-10 text-xs text-text-tertiary">加载中...</div>
            <div v-else-if="!messages.length" class="px-4 py-10 text-xs text-text-tertiary">暂无消息</div>
            <div v-else class="divide-y divide-border-subtle">
              <article
                v-for="message in messages"
                :key="message.id"
                class="grid grid-cols-1 gap-1 px-3 py-3 sm:grid-cols-[58px_minmax(0,1fr)] sm:gap-3 sm:px-4"
              >
                <div class="text-[11px] text-text-tertiary sm:pt-0.5">{{ formatTime(message.created_at) }}</div>
                <div class="min-w-0">
                  <div class="flex flex-wrap items-center gap-2">
                    <span class="text-xs font-medium" :class="message.direction === 'inbound' ? 'text-text-primary' : 'text-accent'">{{ directionLabel(message.direction) }}</span>
                    <span class="rounded bg-active px-1.5 py-0.5 text-[10px] text-text-tertiary">{{ message.message_type }}</span>
                    <span v-if="message.username" class="text-[11px] text-text-tertiary">{{ message.username }}</span>
                  </div>
                  <p class="mt-1 whitespace-pre-wrap break-words text-[13px] leading-relaxed text-text-primary">{{ message.content }}</p>
                  <div class="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[10px] text-text-tertiary">
                    <span v-if="message.session_id">Session {{ shortId(message.session_id) }}</span>
                    <span v-if="message.reply_to_external_id">回复 {{ shortId(message.reply_to_external_id) }}</span>
                    <span class="font-mono">{{ shortId(message.external_message_id) }}</span>
                  </div>
                </div>
              </article>
            </div>
          </div>
          <div v-else class="min-h-0 flex-1 overflow-y-auto" role="tabpanel">
            <AgentTracePanel :events="traceEvents" :loading="traceLoading" :error="traceError" />
          </div>
        </template>
        <div v-else class="hidden flex-1 items-center justify-center px-6 text-xs text-text-tertiary lg:flex">选择一个会话查看记录</div>
      </section>
    </div>
  </div>
</template>
