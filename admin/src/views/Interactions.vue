<script setup lang="ts">
/**
 * 交互单管理：观测 pending→answered→done 流转，并在卡片丢失等边缘情况手动应答/取消。
 *
 * 手动应答会代替用户做选择并真的派发 resume（不是只改库状态）；
 * 高成本量化操作会真实消耗配额，确认前请核对标题与目标。
 * 不默认轮询——pending 是常态，避免永久刷表；用刷新按钮即可。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import InteractionDetail from '../components/InteractionDetail.vue'
import { api, type AgentInteraction } from '../composables/api'

const route = useRoute()
const loading = ref(true)
const actionError = ref('')
const notice = ref('')
const items = ref<AgentInteraction[]>([])
const total = ref(0)
const page = ref(1)
const perPage = 30

const filterStatus = ref('')
const filterKind = ref('')
const filterChannel = ref('')
const expandedId = ref<string | null>(null)
/** 深链 id 不在当前页时，单独拉详情展开 */
const detailExtra = ref<AgentInteraction | null>(null)
const actionBusy = ref(false)

const STATUS_LABELS: Record<string, string> = {
  pending: '待确认', answered: '已应答', executing: '执行中', done: '已完成',
  failed: '失败', expired: '已过期', cancelled: '已取消',
}
const KIND_LABELS: Record<string, string> = {
  quant_confirm: '量化确认', ask_user: '询问用户', task_gate: '任务闸门',
}
const CHANNEL_LABELS: Record<string, string> = {
  feishu: '飞书', weixin: '微信', trace_chat: '网页会话',
}

const totalPages = computed(() => Math.max(1, Math.ceil(total.value / perPage)))
const statusLabel = (s: string) => STATUS_LABELS[s] ?? s
const kindLabel = (k: string) => KIND_LABELS[k] ?? k
const channelLabel = (c: string) => CHANNEL_LABELS[c] ?? c
const shortId = (id: string) => id.slice(0, 8)

function statusClass(status: string): string {
  if (status === 'pending') return 'text-yellow-500'
  if (status === 'failed' || status === 'expired') return 'text-red-400'
  if (status === 'done') return 'text-text-primary'
  return 'text-text-secondary'
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '—'
  const d = new Date(value)
  return Number.isNaN(d.getTime()) ? value : d.toLocaleString('zh-CN', { hour12: false })
}

function parseOptions(raw: string): string[] {
  try {
    const v = JSON.parse(raw)
    return Array.isArray(v) ? v.map(String) : []
  } catch {
    return []
  }
}

function toggleExpand(id: string) {
  if (expandedId.value === id) {
    expandedId.value = null
    if (detailExtra.value?.id === id) detailExtra.value = null
  } else {
    expandedId.value = id
  }
}

async function load() {
  loading.value = true
  actionError.value = ''
  try {
    const res = await api.listInteractions({
      status: filterStatus.value || undefined,
      kind: filterKind.value || undefined,
      channel: filterChannel.value || undefined,
      page: page.value,
      per_page: perPage,
    })
    items.value = res.data
    total.value = res.total
    await ensureDeepLinkExpanded()
  } catch (e: any) {
    actionError.value = e?.message || '加载失败'
  } finally {
    loading.value = false
  }
}

/** 路由带 :id 时自动展开；不在当前列表则单独拉详情 */
async function ensureDeepLinkExpanded() {
  const id = typeof route.params.id === 'string' ? route.params.id : ''
  if (!id) return
  expandedId.value = id
  if (items.value.some((r) => r.id === id)) {
    detailExtra.value = null
    return
  }
  try {
    detailExtra.value = await api.getInteraction(id)
  } catch (e: any) {
    actionError.value = e?.message || `交互单 ${id} 加载失败`
    detailExtra.value = null
  }
}

function onFilterChange() {
  page.value = 1
  void load()
}

async function withAction(confirmMsg: string, run: () => Promise<void>, ok: string, fail: string) {
  if (!confirm(confirmMsg)) return
  actionBusy.value = true
  actionError.value = ''
  notice.value = ''
  try {
    await run()
    notice.value = ok
    await load()
  } catch (e: any) {
    actionError.value = e?.message || fail
  } finally {
    actionBusy.value = false
  }
}

async function answer(row: AgentInteraction, option: string) {
  await withAction(
    `确定以「${option}」应答交互单 ${shortId(row.id)}？\n这将代替用户推进任务，高成本操作会真实执行。`,
    () => api.answerInteraction(row.id, option).then(() => undefined),
    `已应答「${option}」，任务已派发。`,
    '应答失败',
  )
}

async function cancel(row: AgentInteraction) {
  await withAction(
    `确定取消交互单 ${shortId(row.id)}？\n取消后渠道卡片再点会被拒绝。`,
    () => api.cancelInteraction(row.id).then(() => undefined),
    '已取消。',
    '取消失败',
  )
}

const displayRows = computed(() => {
  if (!detailExtra.value) return items.value
  if (items.value.some((r) => r.id === detailExtra.value!.id)) return items.value
  return [detailExtra.value, ...items.value]
})

watch(() => route.params.id, () => { void ensureDeepLinkExpanded() })
onMounted(() => { void load() })
</script>

<template>
  <div>
    <div class="mb-6 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
      <h1 class="text-lg font-semibold text-text-primary">交互单</h1>
      <button
        type="button"
        class="min-h-10 self-start text-xs text-accent hover:underline disabled:opacity-40 sm:min-h-0"
        :disabled="loading"
        @click="load"
      >刷新</button>
    </div>

    <p v-if="actionError" class="mb-4 text-xs text-red-400">{{ actionError }}</p>
    <p v-else-if="notice" class="mb-4 text-xs text-green-500">{{ notice }}</p>

    <div class="mb-6 rounded-lg border border-border-subtle p-3 text-xs leading-5 text-text-secondary sm:p-4">
      <p>
        交互单是 Agent 向用户索取确认或输入的待办记录（量化高成本确认、ask_user 等）。
        状态会从「待确认」经「已应答」到「已完成 / 失败 / 取消」。
      </p>
      <p class="mt-1 text-text-tertiary">
        手动应答会代替用户做出选择并<strong class="font-medium text-text-secondary">真的推进任务执行</strong>；
        高成本量化操作（回测 / trial / 因子评估）会真实消耗配额，确认前请核对标题与目标。
        取消只把状态标为已取消，不改飞书卡片外观，但再点卡片会被拒绝。
      </p>
    </div>

    <div class="mb-4 flex flex-col gap-3 text-xs sm:flex-row sm:flex-wrap sm:items-center">
      <label class="flex min-h-10 items-center gap-1.5 text-text-secondary sm:min-h-0">
        状态
        <select
          v-model="filterStatus"
          class="min-w-0 flex-1 rounded border border-border bg-transparent px-2 py-2 text-text-primary sm:flex-none sm:py-1"
          @change="onFilterChange"
        >
          <option value="">全部</option>
          <option v-for="(label, key) in STATUS_LABELS" :key="key" :value="key">{{ label }}</option>
        </select>
      </label>
      <label class="flex min-h-10 items-center gap-1.5 text-text-secondary sm:min-h-0">
        类型
        <select
          v-model="filterKind"
          class="min-w-0 flex-1 rounded border border-border bg-transparent px-2 py-2 text-text-primary sm:flex-none sm:py-1"
          @change="onFilterChange"
        >
          <option value="">全部</option>
          <option v-for="(label, key) in KIND_LABELS" :key="key" :value="key">{{ label }}</option>
        </select>
      </label>
      <label class="flex min-h-10 items-center gap-1.5 text-text-secondary sm:min-h-0">
        渠道
        <select
          v-model="filterChannel"
          class="min-w-0 flex-1 rounded border border-border bg-transparent px-2 py-2 text-text-primary sm:flex-none sm:py-1"
          @change="onFilterChange"
        >
          <option value="">全部</option>
          <option v-for="(label, key) in CHANNEL_LABELS" :key="key" :value="key">{{ label }}</option>
        </select>
      </label>
    </div>

    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>
    <div v-else-if="displayRows.length === 0" class="text-sm text-text-tertiary">暂无交互单。</div>

    <!-- 移动端卡片列表 -->
    <div v-else class="space-y-3 md:hidden">
      <div
        v-for="row in displayRows"
        :key="row.id"
        class="rounded-md border border-border-subtle p-3"
      >
        <div class="flex items-start justify-between gap-2">
          <button
            type="button"
            class="font-mono text-xs text-accent hover:underline"
            :title="row.id"
            @click="toggleExpand(row.id)"
          >{{ shortId(row.id) }}</button>
          <span class="shrink-0 text-xs" :class="statusClass(row.status)">{{ statusLabel(row.status) }}</span>
        </div>
        <p class="mt-1.5 text-[13px] text-text-primary">{{ row.title || '—' }}</p>
        <div class="mt-1.5 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-text-tertiary">
          <span>{{ kindLabel(row.kind) }}</span>
          <span>{{ channelLabel(row.channel) }}</span>
          <span>{{ formatTime(row.created_at) }}</span>
        </div>
        <div class="mt-2 flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="min-h-9 text-xs text-text-secondary hover:underline"
            @click="toggleExpand(row.id)"
          >{{ expandedId === row.id ? '收起' : '详情' }}</button>
          <template v-if="row.status === 'pending'">
            <button
              v-for="opt in parseOptions(row.options)"
              :key="opt"
              type="button"
              class="min-h-9 rounded-md border border-border px-2.5 text-xs text-accent disabled:opacity-40"
              :disabled="actionBusy"
              @click="answer(row, opt)"
            >{{ opt }}</button>
            <button
              type="button"
              class="min-h-9 text-xs text-red-400 hover:underline disabled:opacity-40"
              :disabled="actionBusy"
              @click="cancel(row)"
            >取消</button>
          </template>
        </div>
        <div v-if="expandedId === row.id" class="mt-3 border-t border-border-subtle pt-3">
          <InteractionDetail :row="row" />
        </div>
      </div>
    </div>

    <!-- 桌面表格 -->
    <div v-if="!loading && displayRows.length > 0" class="hidden table-scroll md:block">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border-subtle text-left text-xs text-text-tertiary">
            <th class="py-2 pr-3">任务编号</th>
            <th class="py-2 pr-3">类型</th>
            <th class="py-2 pr-3">状态</th>
            <th class="py-2 pr-3">标题</th>
            <th class="py-2 pr-3">渠道</th>
            <th class="py-2 pr-3">创建时间</th>
            <th class="py-2 pr-3">应答时间</th>
            <th class="py-2">操作</th>
          </tr>
        </thead>
        <tbody>
          <template v-for="row in displayRows" :key="row.id">
            <tr class="border-b border-border-subtle">
              <td class="py-2 pr-3 align-top">
                <button
                  type="button"
                  class="font-mono text-xs text-accent hover:underline"
                  :title="row.id"
                  @click="toggleExpand(row.id)"
                >{{ shortId(row.id) }}</button>
              </td>
              <td class="whitespace-nowrap py-2 pr-3 align-top text-xs text-text-secondary">
                {{ kindLabel(row.kind) }}
              </td>
              <td class="whitespace-nowrap py-2 pr-3 align-top text-xs" :class="statusClass(row.status)">
                {{ statusLabel(row.status) }}
              </td>
              <td class="max-w-48 truncate py-2 pr-3 align-top text-text-primary" :title="row.title">
                {{ row.title || '—' }}
              </td>
              <td class="whitespace-nowrap py-2 pr-3 align-top text-xs text-text-secondary">
                {{ channelLabel(row.channel) }}
              </td>
              <td class="whitespace-nowrap py-2 pr-3 align-top text-xs text-text-tertiary">
                {{ formatTime(row.created_at) }}
              </td>
              <td class="whitespace-nowrap py-2 pr-3 align-top text-xs text-text-tertiary">
                {{ formatTime(row.answered_at) }}
              </td>
              <td class="whitespace-nowrap py-2 align-top">
                <button
                  type="button"
                  class="mr-2 text-xs text-text-secondary hover:underline"
                  @click="toggleExpand(row.id)"
                >{{ expandedId === row.id ? '收起' : '详情' }}</button>
                <template v-if="row.status === 'pending'">
                  <button
                    v-for="opt in parseOptions(row.options)"
                    :key="opt"
                    type="button"
                    class="mr-2 text-xs text-accent hover:underline disabled:opacity-40"
                    :disabled="actionBusy"
                    @click="answer(row, opt)"
                  >{{ opt }}</button>
                  <button
                    type="button"
                    class="text-xs text-red-400 hover:underline disabled:opacity-40"
                    :disabled="actionBusy"
                    @click="cancel(row)"
                  >取消</button>
                </template>
              </td>
            </tr>
            <tr v-if="expandedId === row.id" class="border-b border-border-subtle bg-surface">
              <td colspan="8">
                <InteractionDetail :row="row" />
              </td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>

    <div
      v-if="!loading && total > 0"
      class="mt-6 flex flex-col gap-3 text-[12px] text-text-tertiary sm:flex-row sm:items-center sm:justify-between"
    >
      <span>{{ total }} 条 · 第 {{ page }} / {{ totalPages }} 页</span>
      <div class="flex items-center gap-1">
        <button
          type="button"
          class="min-h-9 rounded px-3 py-1.5 transition-colors hover:bg-hover disabled:opacity-30"
          :disabled="page <= 1"
          @click="page = Math.max(1, page - 1); load()"
        >← 上一页</button>
        <button
          type="button"
          class="min-h-9 rounded px-3 py-1.5 transition-colors hover:bg-hover disabled:opacity-30"
          :disabled="page >= totalPages"
          @click="page += 1; load()"
        >下一页 →</button>
      </div>
    </div>
  </div>
</template>
