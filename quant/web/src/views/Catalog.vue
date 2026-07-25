<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { BookOpen, Search } from 'lucide-vue-next'
import type { CatalogEntry } from '../api'
import { categoryLabels, useCatalog } from '../catalog'
import PageHeader from '../components/PageHeader.vue'
import LoadingRows from '../components/LoadingRows.vue'
import WorkspaceTabs from '../components/WorkspaceTabs.vue'

type CatalogTab = 'factors' | 'strategies' | 'signals'

const { catalog, loading, usingFallback, load } = useCatalog()
const active = ref<CatalogTab>('factors')
const query = ref('')
const tabs = [
  { key: 'factors', label: '指标' },
  { key: 'strategies', label: '策略' },
  { key: 'signals', label: '信号' },
]

const entries = computed<CatalogEntry[]>(() => {
  if (active.value === 'strategies') return catalog.value.strategies
  if (active.value === 'signals') return catalog.value.signals

  const all = [...catalog.value.indicators, ...catalog.value.factors, ...catalog.value.backtest_metrics, ...catalog.value.filter_fields]
  return [...new Map(all.map((entry) => [entry.key, entry])).values()]
})

const filtered = computed(() => {
  const needle = query.value.trim().toLowerCase()
  if (!needle) return entries.value
  return entries.value.filter((entry) =>
    [entry.name, entry.key, entry.description, entry.category, entry.formula, entry.caliber, entry.caveat, entry.limits]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(needle))
  )
})

function changeTab(tab: string) {
  active.value = tab as CatalogTab
}

onMounted(load)
</script>

<template>
  <div class="space-y-5">
    <PageHeader
      title="研究词典"
      description="中文名称用于阅读，英文 key 用于对照系统参数。"
    >
      <template #actions>
        <span v-if="usingFallback" class="rounded-md bg-warning-soft px-2 py-1 text-xs text-warning">
          使用内置词典
        </span>
      </template>
    </PageHeader>

    <WorkspaceTabs :tabs="tabs" :active="active" @change="changeTab" />

    <label class="relative block max-w-md">
      <span class="sr-only">搜索名称、说明或英文 key</span>
      <Search :size="17" class="pointer-events-none absolute left-3 top-2.5 text-text-tertiary" />
      <input
        v-model="query"
        type="search"
        placeholder="搜索名称、说明或英文 key"
        class="w-full rounded-md border border-border py-2 pl-9 pr-3 text-sm"
      />
    </label>

    <LoadingRows v-if="loading" :rows="6" />

    <div v-else-if="filtered.length" class="rounded-md border border-border bg-surface-raised">
      <article
        v-for="entry in filtered"
        :key="entry.key"
        class="grid gap-3 border-b border-border-subtle px-4 py-4 last:border-0 md:grid-cols-[minmax(190px,0.32fr)_minmax(0,1fr)] md:px-5"
      >
        <div>
          <div class="flex flex-wrap items-center gap-2">
            <h2 class="text-sm font-semibold text-text-primary">{{ entry.name }}</h2>
            <span v-if="entry.available === false" class="rounded bg-surface-muted px-1.5 py-0.5 text-[11px] text-text-tertiary">数据待接入</span>
          </div>
          <code class="mt-1 block text-xs text-text-tertiary">{{ entry.key }}</code>
          <span v-if="entry.category" class="mt-2 inline-block rounded bg-active px-1.5 py-0.5 text-[11px] text-accent">
            {{ categoryLabels[entry.category] ?? entry.kind_name ?? entry.category }}
          </span>
        </div>
        <div>
          <p class="max-w-3xl text-sm leading-6 text-text-secondary">{{ entry.description }}</p>
          <dl v-if="entry.formula || entry.caliber || entry.direction || entry.caveat || entry.limits || entry.source" class="mt-3 grid gap-x-5 gap-y-2 text-xs sm:grid-cols-2">
            <div v-if="entry.formula">
              <dt class="text-text-tertiary">计算方式</dt>
              <dd class="mt-0.5 text-text-secondary">{{ entry.formula }}</dd>
            </div>
            <div v-if="entry.caliber">
              <dt class="text-text-tertiary">数据口径</dt>
              <dd class="mt-0.5 text-text-secondary">{{ entry.caliber }}</dd>
            </div>
            <div v-if="entry.direction">
              <dt class="text-text-tertiary">数值理解</dt>
              <dd class="mt-0.5 text-text-secondary">{{ entry.direction }}</dd>
            </div>
            <div v-if="entry.source">
              <dt class="text-text-tertiary">数据来源</dt>
              <dd class="mt-0.5 text-text-secondary">{{ entry.source }}</dd>
            </div>
            <div v-if="entry.caveat || entry.limits" class="sm:col-span-2">
              <dt class="text-text-tertiary">使用限制</dt>
              <dd class="mt-0.5 text-text-secondary">{{ entry.caveat ?? entry.limits }}</dd>
            </div>
          </dl>
        </div>
      </article>
    </div>

    <div v-else class="rounded-md border border-dashed border-border px-5 py-10 text-center">
      <BookOpen :size="24" class="mx-auto text-text-tertiary" />
      <p class="mt-3 text-sm text-text-secondary">没有找到匹配的词条</p>
      <button type="button" class="mt-2 text-sm text-accent hover:underline" @click="query = ''">清除搜索</button>
    </div>
  </div>
</template>
