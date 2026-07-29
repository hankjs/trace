<script lang="ts">
import { api, type StockSearchItem } from '../api'

/** 全市场清单所有 StockPicker 实例共享一次请求,失败时清空缓存允许重试 */
let stockListCache: Promise<StockSearchItem[]> | null = null
function loadAllStocks(): Promise<StockSearchItem[]> {
  if (!stockListCache) {
    stockListCache = api.stockList()
      .then((r) => r.items ?? [])
      .catch((e) => {
        stockListCache = null
        throw e
      })
  }
  return stockListCache
}
</script>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Check, ChevronDown, Search, X } from 'lucide-vue-next'

const props = withDefaults(defineProps<{
  /** 已选股票代码 */
  modelValue: string[]
  /** true 多选(checkbox) / false 单选(radio,选中即关闭) */
  multiple?: boolean
  placeholder?: string
  disabled?: boolean
}>(), {
  multiple: true,
  placeholder: '请选择股票',
  disabled: false,
})

const emit = defineEmits<{
  'update:modelValue': [codes: string[]]
}>()

const ROW_H = 36
const LIST_H = 320
const BUFFER = 6

const root = ref<HTMLElement | null>(null)
const listEl = ref<HTMLElement | null>(null)
const open = ref(false)
const stocks = ref<StockSearchItem[]>([])
const loading = ref(true)
const loadError = ref('')
const filter = ref('')
const scrollTop = ref(0)

const selected = computed(() => props.modelValue)
const nameByCode = computed(() => new Map(stocks.value.map((s) => [s.code, s.name])))

function stockName(code: string): string {
  return nameByCode.value.get(code) || '名称待同步'
}

const filtered = computed(() => {
  const f = filter.value.trim().toLowerCase()
  if (!f) return stocks.value
  return stocks.value.filter((s) => s.code.includes(f) || s.name.toLowerCase().includes(f))
})

/** 虚拟滚动窗口:固定行高,只渲染可视区上下各 BUFFER 行 */
const range = computed(() => {
  const total = filtered.value.length
  const start = Math.max(0, Math.floor(scrollTop.value / ROW_H) - BUFFER)
  const end = Math.min(total, Math.ceil((scrollTop.value + LIST_H) / ROW_H) + BUFFER)
  return { start, end, padTop: start * ROW_H, padBottom: Math.max(0, (total - end) * ROW_H) }
})
const visibleRows = computed(() => filtered.value.slice(range.value.start, range.value.end))

const summary = computed(() => {
  if (!selected.value.length) return ''
  if (!props.multiple) {
    const code = selected.value[0]
    return `${stockName(code)} · ${code}`
  }
  const names = selected.value.slice(0, 2).map((code) => stockName(code))
  return selected.value.length <= 2
    ? names.join('、')
    : `${names.join('、')} 等 ${selected.value.length} 只`
})

function isSelected(code: string): boolean {
  return selected.value.includes(code)
}

function toggle(code: string) {
  if (props.multiple) {
    emit('update:modelValue', isSelected(code)
      ? selected.value.filter((c) => c !== code)
      : [...selected.value, code])
  } else {
    emit('update:modelValue', [code])
    close()
  }
}

function remove(code: string) {
  emit('update:modelValue', selected.value.filter((c) => c !== code))
}

function clearAll() {
  emit('update:modelValue', [])
}

function toggleOpen() {
  if (props.disabled) return
  open.value ? close() : (open.value = true)
}

function close() {
  open.value = false
  filter.value = ''
}

function onDocMouseDown(e: MouseEvent) {
  if (root.value && !root.value.contains(e.target as Node)) close()
}

function onDocKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}

function onScroll() {
  scrollTop.value = listEl.value?.scrollTop ?? 0
}

watch(open, (v) => {
  if (v) {
    document.addEventListener('mousedown', onDocMouseDown)
    document.addEventListener('keydown', onDocKeydown)
  } else {
    document.removeEventListener('mousedown', onDocMouseDown)
    document.removeEventListener('keydown', onDocKeydown)
  }
})

watch(filter, () => {
  if (listEl.value) listEl.value.scrollTop = 0
  scrollTop.value = 0
})

onMounted(async () => {
  try {
    stocks.value = await loadAllStocks()
  } catch (e) {
    loadError.value = (e as Error).message
  } finally {
    loading.value = false
  }
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocMouseDown)
  document.removeEventListener('keydown', onDocKeydown)
})
</script>

<template>
  <div ref="root" class="relative">
    <button
      type="button"
      :disabled="disabled"
      class="flex h-9 w-full items-center justify-between gap-2 rounded-md border border-border bg-surface-raised px-2.5 text-sm transition-colors hover:bg-hover disabled:opacity-50"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggleOpen"
    >
      <span class="min-w-0 truncate" :class="summary ? 'text-text-primary' : 'text-text-tertiary'">
        {{ summary || placeholder }}
      </span>
      <ChevronDown
        :size="15"
        class="shrink-0 text-text-tertiary transition-transform"
        :class="open ? 'rotate-180' : ''"
      />
    </button>

    <div
      v-if="open"
      class="absolute left-0 top-full z-30 mt-1 w-full min-w-72 overflow-hidden rounded-lg border border-border bg-surface-raised shadow-panel"
      role="listbox"
      :aria-multiselectable="multiple"
    >
      <!-- 已选择内容 -->
      <div class="border-b border-border-subtle px-3 py-2">
        <div v-if="selected.length" class="flex max-h-16 flex-wrap items-center gap-1.5 overflow-y-auto">
          <span
            v-for="code in selected"
            :key="code"
            class="inline-flex h-6 items-center gap-1 rounded bg-active px-1.5 text-xs text-text-primary"
          >
            {{ stockName(code) }} · {{ code }}
            <button
              type="button"
              class="text-text-tertiary transition-colors hover:text-text-primary"
              :aria-label="`移除 ${stockName(code)}`"
              @click.stop="remove(code)"
            >
              <X :size="11" />
            </button>
          </span>
          <button
            v-if="multiple && selected.length > 1"
            type="button"
            class="rounded px-1.5 text-xs text-text-tertiary transition-colors hover:bg-hover hover:text-text-primary"
            @click="clearAll"
          >清空</button>
        </div>
        <p v-else class="text-xs text-text-tertiary">
          尚未选择{{ multiple ? '，可在下方列表多选' : '，可在下方列表选择一只' }}
        </p>
      </div>

      <!-- 过滤 -->
      <div class="flex items-center gap-2 border-b border-border-subtle px-3 py-1.5">
        <Search :size="14" class="shrink-0 text-text-tertiary" />
        <input
          v-model="filter"
          type="text"
          placeholder="输入名称或代码过滤"
          class="h-7 w-full bg-transparent text-sm outline-none"
        />
      </div>

      <!-- 虚拟表格 -->
      <div ref="listEl" class="overflow-y-auto" :style="{ maxHeight: `${LIST_H}px` }" @scroll="onScroll">
        <p v-if="loading" class="px-3 py-6 text-center text-xs text-text-tertiary">正在加载股票清单…</p>
        <p v-else-if="loadError" class="px-3 py-6 text-center text-xs text-up">{{ loadError }}</p>
        <p v-else-if="!filtered.length" class="px-3 py-6 text-center text-xs text-text-tertiary">
          没有匹配「{{ filter }}」的股票
        </p>
        <div v-else :style="{ paddingTop: `${range.padTop}px`, paddingBottom: `${range.padBottom}px` }">
          <button
            v-for="s in visibleRows"
            :key="s.code"
            type="button"
            role="option"
            :aria-selected="isSelected(s.code)"
            class="flex w-full items-center gap-2.5 px-3 text-left text-sm transition-colors"
            :class="isSelected(s.code) ? 'bg-active/60 text-text-primary' : 'text-text-secondary hover:bg-hover'"
            :style="{ height: `${ROW_H}px` }"
            @click="toggle(s.code)"
          >
            <span
              v-if="multiple"
              class="flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors"
              :class="isSelected(s.code) ? 'border-accent bg-accent text-on-accent' : 'border-border'"
            >
              <Check v-if="isSelected(s.code)" :size="11" />
            </span>
            <span
              v-else
              class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border transition-colors"
              :class="isSelected(s.code) ? 'border-accent' : 'border-border'"
            >
              <span v-if="isSelected(s.code)" class="h-2 w-2 rounded-full bg-accent" />
            </span>
            <span class="w-20 shrink-0 font-mono text-xs text-text-tertiary">{{ s.code }}</span>
            <span class="min-w-0 flex-1 truncate">{{ s.name }}</span>
            <span v-if="s.industry" class="shrink-0 text-xs text-text-tertiary">{{ s.industry }}</span>
          </button>
        </div>
      </div>

      <div class="border-t border-border-subtle px-3 py-1.5 text-[11px] text-text-tertiary">
        {{ filter ? `匹配 ${filtered.length} 只` : `共 ${stocks.length} 只` }}
        <template v-if="selected.length"> · 已选 {{ selected.length }} 只</template>
      </div>
    </div>
  </div>
</template>
