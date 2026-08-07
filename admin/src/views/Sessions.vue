<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { api, type Session, type PaginatedResponse } from '../composables/api'
import { backendLabel, backendTone } from '../utils/agentBackend'

const sessions = ref<Session[]>([])
const total = ref(0)
const page = ref(1)
const search = ref('')
const perPage = 20

async function load() {
  const res: PaginatedResponse<Session> = await api.sessions(page.value, perPage, search.value, '!explore')
  sessions.value = res.data
  total.value = res.total
}

onMounted(load)
watch([page, search], load)
</script>

<template>
  <div>
    <div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <h1 class="text-lg font-semibold text-text-primary">Sessions</h1>
      <input
        v-model="search"
        placeholder="Search..."
        class="w-full rounded-md border border-border bg-transparent px-3 py-2 text-[13px] placeholder:text-text-tertiary transition-colors focus:border-accent focus:outline-none sm:w-56 sm:py-1.5"
      />
    </div>

    <!-- 桌面表头 -->
    <div class="hidden text-[12px] font-medium text-text-tertiary md:grid md:grid-cols-[1fr_80px_100px_140px_100px_60px] md:gap-2 md:border-b md:border-border-subtle md:px-2 md:pb-2">
      <span>Title</span>
      <span>User</span>
      <span>Provider</span>
      <span>Model</span>
      <span class="text-right">Updated</span>
      <span></span>
    </div>

    <div class="divide-y divide-border-subtle">
      <div
        v-for="s in sessions"
        :key="s.id"
        class="-mx-2 rounded-md px-2 py-3 transition-colors duration-100 hover:bg-hover md:grid md:grid-cols-[1fr_80px_100px_140px_100px_60px] md:items-center md:gap-2 md:py-2.5"
      >
        <!-- 移动端卡片 -->
        <div class="md:contents">
          <div class="flex items-start justify-between gap-2 md:contents">
            <RouterLink
              :to="`/sessions/${s.id}`"
              class="min-w-0 flex-1 text-[13px] text-text-primary transition-colors hover:text-accent md:truncate"
            >{{ s.title || s.id.slice(0, 8) }}</RouterLink>
            <RouterLink
              :to="`/sessions/${s.id}/timeline`"
              class="shrink-0 text-[11px] text-accent transition-colors hover:text-accent-hover md:text-right"
              @click.stop
            >Timeline</RouterLink>
          </div>
          <div class="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] text-text-tertiary md:contents">
            <span class="md:truncate">{{ s.username || '-' }}</span>
            <span class="md:truncate" :class="backendTone(s.provider)" :title="s.provider || '未记录'">{{ backendLabel(s.provider) }}</span>
            <span class="md:truncate" :title="s.model">{{ s.model || '-' }}</span>
            <span class="md:text-right">{{ new Date(s.updated_at).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' }) }}</span>
          </div>
        </div>
      </div>
    </div>

    <div v-if="!sessions.length" class="py-12 text-center text-[13px] text-text-tertiary">No sessions found</div>

    <div class="mt-6 flex items-center justify-between text-[12px] text-text-tertiary">
      <span>{{ total }} total</span>
      <div class="flex items-center gap-1">
        <button
          type="button"
          class="min-h-9 rounded px-3 py-1.5 transition-colors hover:bg-hover disabled:opacity-30"
          :disabled="page <= 1"
          @click="page = Math.max(1, page - 1)"
        >←</button>
        <span class="px-2 tabular-nums">{{ page }}</span>
        <button
          type="button"
          class="min-h-9 rounded px-3 py-1.5 transition-colors hover:bg-hover disabled:opacity-30"
          :disabled="sessions.length < perPage"
          @click="page++"
        >→</button>
      </div>
    </div>
  </div>
</template>
