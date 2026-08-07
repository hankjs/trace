<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type Session, type PaginatedResponse } from '../composables/api'

const sessions = ref<Session[]>([])
const loading = ref(true)
const page = ref(1)
const total = ref(0)
const perPage = 20

async function load() {
  loading.value = true
  try {
    const res: PaginatedResponse<Session> = await api.sessions(page.value, perPage, '', 'explore')
    sessions.value = res.data
    total.value = res.total
  } finally {
    loading.value = false
  }
}

function prevPage() { if (page.value > 1) { page.value--; load() } }
function nextPage() { if (page.value * perPage < total.value) { page.value++; load() } }

onMounted(load)
</script>

<template>
  <div>
    <h1 class="text-lg font-semibold text-text-primary mb-6">探索追踪</h1>

    <div v-if="loading" class="py-12 text-center text-[13px] text-text-tertiary">Loading...</div>

    <template v-else>
      <div class="space-y-1">
        <RouterLink
          v-for="s in sessions"
          :key="s.id"
          :to="`/explore/${s.id}`"
          class="group flex min-h-12 flex-col gap-1 rounded-lg border border-transparent px-3 py-2.5 transition-all hover:border-border-subtle hover:bg-surface-raised/50 sm:flex-row sm:items-center sm:gap-3"
        >
          <span class="w-auto shrink-0 font-mono text-[12px] text-text-tertiary sm:w-16">{{ s.id.slice(0, 8) }}</span>
          <span class="min-w-0 flex-1 truncate text-[13px] text-text-primary">{{ s.title || '(untitled)' }}</span>
          <div class="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-text-tertiary">
            <span>{{ s.model }}</span>
            <span class="tabular-nums">{{ new Date(s.created_at).toLocaleDateString() }}</span>
            <span class="text-accent sm:opacity-0 sm:transition-opacity sm:group-hover:opacity-100">Explore →</span>
          </div>
        </RouterLink>
      </div>

      <div v-if="!sessions.length" class="py-12 text-center text-[13px] text-text-tertiary">暂无会话</div>

      <div v-if="total > perPage" class="flex items-center justify-center gap-4 mt-6 text-[12px]">
        <button @click="prevPage" :disabled="page === 1" class="text-text-tertiary hover:text-text-secondary disabled:opacity-30 transition-colors">← 上一页</button>
        <span class="text-text-tertiary tabular-nums">{{ page }} / {{ Math.ceil(total / perPage) }}</span>
        <button @click="nextPage" :disabled="page * perPage >= total" class="text-text-tertiary hover:text-text-secondary disabled:opacity-30 transition-colors">下一页 →</button>
      </div>
    </template>
  </div>
</template>
