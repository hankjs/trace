<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { Search, Star } from 'lucide-vue-next'
import { api, normalizeStockCode, type StockSearchItem } from '../api'

withDefaults(defineProps<{
  label?: string
  placeholder?: string
  required?: boolean
  hideLabel?: boolean
  inputClass?: string
}>(), {
  label: '股票',
  placeholder: '输入中文名称或代码',
  required: false,
  hideLabel: false,
  inputClass: '',
})

const model = defineModel<string>({ required: true })
const query = ref(model.value)
const results = ref<StockSearchItem[]>([])
const open = ref(false)
const loading = ref(false)
const error = ref('')
const validationError = ref('')
const activeIndex = ref(-1)
const selected = ref<StockSearchItem | null>(null)
const input = ref<HTMLInputElement | null>(null)
let timer: ReturnType<typeof setTimeout> | null = null
let requestSequence = 0
let suppressSearch = false

const listId = `stock-search-${Math.random().toString(36).slice(2)}`
const activeId = computed(() => activeIndex.value >= 0 ? `${listId}-${activeIndex.value}` : undefined)

watch(model, (value) => {
  if (!value) query.value = ''
  else if (!query.value) query.value = value
})

watch(query, (value) => {
  if (suppressSearch) {
    suppressSearch = false
    return
  }
  if (timer) clearTimeout(timer)
  const needle = value.trim()
  if (!needle) {
    results.value = []
    open.value = false
    model.value = ''
    selected.value = null
    validationError.value = ''
    return
  }
  if (selected.value && value !== `${selected.value.name || '名称待同步'} ${selected.value.code}`) selected.value = null
  if (!selected.value) model.value = normalizeStockCode(needle) ?? ''
  validationError.value = ''
  timer = setTimeout(() => searchStocks(needle), 180)
})

async function searchStocks(needle: string) {
  const sequence = ++requestSequence
  loading.value = true
  error.value = ''
  try {
    const response = await api.stockSearch(needle)
    if (sequence !== requestSequence) return
    results.value = response.items ?? []
    activeIndex.value = results.value.length ? 0 : -1
    open.value = true
  } catch {
    if (sequence !== requestSequence) return
    results.value = []
    error.value = '股票搜索暂不可用，可直接输入完整代码'
    open.value = true
  } finally {
    if (sequence === requestSequence) loading.value = false
  }
}

function selectStock(stock: StockSearchItem) {
  requestSequence += 1
  loading.value = false
  selected.value = stock
  model.value = stock.code
  validationError.value = ''
  suppressSearch = true
  query.value = `${stock.name || '名称待同步'} ${stock.code}`
  open.value = false
  activeIndex.value = -1
}

function commitText() {
  const raw = query.value.trim()
  if (selected.value && raw === `${selected.value.name || '名称待同步'} ${selected.value.code}`) {
    model.value = selected.value.code
    open.value = false
    return
  }
  const exact = results.value.find((stock) => stock.code.toLowerCase() === raw.toLowerCase() || stock.name === raw)
  if (exact) selectStock(exact)
  else if (normalizeStockCode(raw)) {
    model.value = normalizeStockCode(raw) ?? ''
    validationError.value = ''
  } else if (raw) {
    model.value = ''
    validationError.value = '请选择搜索结果，或输入六位股票代码'
  }
  open.value = false
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    open.value = true
    if (results.value.length) activeIndex.value = (activeIndex.value + 1) % results.value.length
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    if (results.value.length) activeIndex.value = (activeIndex.value - 1 + results.value.length) % results.value.length
  } else if (event.key === 'Enter' && open.value) {
    event.preventDefault()
    const stock = results.value[activeIndex.value]
    if (stock) selectStock(stock)
    else commitText()
  } else if (event.key === 'Escape') {
    open.value = false
  }
}

function onBlur() {
  window.setTimeout(commitText, 120)
}

async function focusInput() {
  await nextTick()
  input.value?.focus()
}

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
  requestSequence += 1
})

defineExpose({ focus: focusInput })
</script>

<template>
  <label class="relative block text-sm">
    <span v-if="!hideLabel" class="mb-1 block text-xs text-text-tertiary">{{ label }}</span>
    <span class="relative block">
      <Search :size="15" class="pointer-events-none absolute left-2.5 top-2.5 text-text-tertiary" />
      <input
        ref="input"
        v-model="query"
        type="text"
        :required="required"
        :placeholder="placeholder"
        role="combobox"
        autocomplete="off"
        :aria-expanded="open"
        :aria-controls="listId"
        :aria-activedescendant="activeId"
        class="w-56 rounded-md border border-border py-1.5 pl-8 pr-2 text-sm"
        :class="inputClass"
        @focus="query.trim() && (open = true)"
        @blur="onBlur"
        @keydown="onKeydown"
      />
    </span>

    <div
      v-if="open"
      :id="listId"
      role="listbox"
      class="absolute left-0 top-full z-30 mt-1 max-h-64 w-72 overflow-y-auto rounded-md border border-border bg-surface-raised shadow-panel"
    >
      <p v-if="loading" class="px-3 py-2 text-xs text-text-tertiary">正在搜索</p>
      <button
        v-for="(stock, index) in results"
        :id="`${listId}-${index}`"
        :key="stock.code"
        type="button"
        role="option"
        :aria-selected="activeIndex === index"
        class="flex w-full items-center gap-2 border-b border-border-subtle px-3 py-2 text-left last:border-0"
        :class="activeIndex === index ? 'bg-active' : 'hover:bg-hover'"
        @mouseenter="activeIndex = index"
        @mousedown.prevent="selectStock(stock)"
      >
        <span class="min-w-0 flex-1">
          <span class="block truncate text-sm font-medium">{{ stock.name || '名称待同步' }}</span>
          <span class="block text-xs text-text-tertiary">{{ stock.code }}<template v-if="stock.industry"> · {{ stock.industry }}</template></span>
        </span>
        <Star v-if="stock.is_watch" :size="14" class="shrink-0 fill-current text-accent" aria-label="自选股" />
      </button>
      <p v-if="!loading && !results.length" class="px-3 py-2 text-xs text-text-tertiary">{{ error || '没有找到匹配股票' }}</p>
    </div>
    <span v-if="validationError" class="mt-1 block text-xs text-up">{{ validationError }}</span>
  </label>
</template>
