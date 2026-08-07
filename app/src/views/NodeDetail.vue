<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import TerminalView from '../components/TerminalView.vue'
import AppIcon from '../components/ui/AppIcon.vue'
import NeuButton from '../components/ui/NeuButton.vue'
import NeuInput from '../components/ui/NeuInput.vue'
import { api, type ClientInfo, type TermInfo } from '../api'

const route = useRoute()
const router = useRouter()
const clientId = computed(() => String(route.params.id))

const client = ref<ClientInfo | null>(null)
const terminals = ref<TermInfo[]>([])
const activeTermId = ref('')
const cwd = ref('')
const error = ref('')
const loading = ref(true)
const creating = ref(false)
const channelMode = ref<'rtc' | 'relay' | ''>('')

const activeTerm = computed(
  () => terminals.value.find((t) => t.id === activeTermId.value) ?? null,
)

function tabLabel(term: TermInfo): string {
  return term.foreground_cmd || term.shell || term.title || term.id.slice(0, 8)
}

function normalizeTerm(raw: unknown): TermInfo | null {
  if (!raw || typeof raw !== 'object') return null
  const t = raw as Record<string, unknown>
  const id = String(t.id ?? t.term_id ?? '')
  if (!id) return null
  return {
    id,
    cols: typeof t.cols === 'number' ? t.cols : undefined,
    rows: typeof t.rows === 'number' ? t.rows : undefined,
    cwd: typeof t.cwd === 'string' ? t.cwd : undefined,
    shell: typeof t.shell === 'string' ? t.shell : undefined,
    foreground_cmd:
      typeof t.foreground_cmd === 'string' ? t.foreground_cmd : undefined,
    alive: typeof t.alive === 'boolean' ? t.alive : true,
    title: typeof t.title === 'string' ? t.title : undefined,
  }
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [list, termList] = await Promise.all([
      api.clients(),
      api.listTerminals(clientId.value),
    ])
    client.value = list.clients.find((c) => c.id === clientId.value) ?? null
    terminals.value = termList
      .map((t) => normalizeTerm(t))
      .filter((t): t is TermInfo => t !== null)
    if (!activeTermId.value || !terminals.value.some((t) => t.id === activeTermId.value)) {
      activeTermId.value = terminals.value[0]?.id ?? ''
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载失败'
  } finally {
    loading.value = false
  }
}

/** 按当前视口估算终端网格，让 PTY 以 app 尺寸启动（避免 120×30 后再 resize 闪一下） */
function estimateAppTermSize(): { cols: number; rows: number } {
  // 与 TerminalView 字体 13px mono 大致对齐：~7.8×16 单元格
  const cols = Math.max(40, Math.floor((window.innerWidth - 48) / 7.8))
  const rows = Math.max(12, Math.floor((Math.min(window.innerHeight * 0.7, 32 * 16) - 24) / 16))
  return { cols, rows }
}

async function createTerminal() {
  creating.value = true
  error.value = ''
  try {
    const { cols, rows } = estimateAppTermSize()
    const { terminal } = await api.createTerminal(clientId.value, {
      cwd: cwd.value.trim() || undefined,
      cols,
      rows,
    })
    const t = normalizeTerm(terminal)
    if (t) {
      terminals.value.push(t)
      activeTermId.value = t.id
    }
    cwd.value = ''
  } catch (err) {
    error.value = err instanceof Error ? err.message : '新建失败'
  } finally {
    creating.value = false
  }
}

async function closeTerm(term: TermInfo) {
  try {
    await api.closeTerminal(clientId.value, term.id)
    terminals.value = terminals.value.filter((t) => t.id !== term.id)
    if (activeTermId.value === term.id) {
      activeTermId.value = terminals.value[0]?.id ?? ''
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '关闭失败'
  }
}

onMounted(load)
</script>

<template>
  <div v-if="!loading">
    <button
      type="button"
      class="mb-3 inline-flex items-center gap-1 text-sm text-ink-2 hover:text-ink"
      @click="router.push('/')"
    >
      <AppIcon name="chevron-left" />
      节点列表
    </button>

    <template v-if="client">
      <div class="flex items-center gap-3">
        <span
          class="status-dot shrink-0"
          :class="client.online ? 'on' : 'off'"
        />
        <h1 class="min-w-0 truncate text-xl font-medium text-ink">
          {{ client.hostname || client.id.slice(0, 12) }}
        </h1>
        <span
          class="shrink-0 text-sm"
          :class="client.online ? 'text-ok' : 'text-ink-3'"
        >
          {{ client.online ? '在线' : '离线' }}
        </span>
        <span v-if="channelMode" class="shrink-0 text-xs text-ink-3">
          {{ channelMode === 'rtc' ? 'P2P' : '中转' }}
        </span>
      </div>
      <p class="mt-1 truncate text-sm text-ink-2">
        {{ client.work_dir || '未上报工作目录' }}
      </p>

      <p v-if="error" class="mt-3 text-sm text-danger">{{ error }}</p>

      <div
        v-if="!client.online"
        class="mt-4 neu-card px-4 py-3 text-sm text-ink-2"
      >
        节点离线，无法新建或操作终端。请确认本机 Trace 客户端在线且已开启远程终端。
      </div>

      <div class="mt-4 neu-card p-4">
        <div class="flex items-center gap-2">
          <NeuInput
            v-model="cwd"
            placeholder="工作目录（可选）"
            class="min-w-0 flex-1"
            :disabled="!client.online"
          />
          <NeuButton
            variant="primary"
            class="shrink-0"
            :disabled="creating || !client.online"
            @click="createTerminal"
          >
            新建终端
          </NeuButton>
        </div>

        <div v-if="terminals.length" class="mt-4 flex flex-wrap items-center gap-1">
          <button
            v-for="term in terminals"
            :key="term.id"
            type="button"
            class="flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs"
            :class="
              term.id === activeTermId
                ? 'bg-canvas shadow-(--neu-inset) text-ink'
                : 'text-ink-2 hover:text-ink'
            "
            @click="activeTermId = term.id"
          >
            <span>{{ tabLabel(term) }}</span>
            <span v-if="term.alive === false" class="opacity-60">（已退出）</span>
            <span
              class="opacity-60 hover:opacity-100"
              aria-label="关闭终端"
              @click.stop="closeTerm(term)"
            >
              <AppIcon name="x" class="h-3 w-3" />
            </span>
          </button>
        </div>

        <div class="mt-4">
          <TerminalView
            v-if="activeTerm"
            :key="activeTerm.id"
            :client-id="client.id"
            :term-id="activeTerm.id"
            @mode="channelMode = $event"
          />
          <p v-else class="text-sm text-ink-2">
            {{ client.online ? '暂无终端，点「新建终端」开始。' : '' }}
          </p>
        </div>
      </div>
    </template>

    <div v-else class="neu-card p-4 text-sm text-ink-2">
      节点不存在或不属于当前账号。
    </div>
  </div>
  <p v-else class="text-sm text-ink-2">加载中…</p>
</template>
