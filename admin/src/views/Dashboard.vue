<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type MetricsOverview, type Session } from '../composables/api'

const overview = ref<MetricsOverview | null>(null)
const recentSessions = ref<Session[]>([])

onMounted(async () => {
  overview.value = await api.metricsOverview()
  const res = await api.sessions(1, 5)
  recentSessions.value = res.data
})

function formatTokens(n: number) {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M'
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'k'
  return String(n)
}
</script>

<template>
  <div>
    <h1 class="mb-6 text-lg font-semibold text-text-primary md:mb-8">Overview</h1>

    <div
      v-if="overview"
      class="mb-8 grid grid-cols-1 gap-6 sm:grid-cols-3 sm:gap-8 md:mb-12 md:gap-12"
    >
      <div class="border-b border-border-subtle pb-4 sm:border-b-0 sm:pb-0">
        <div class="mb-1 text-[13px] text-text-tertiary">Tokens</div>
        <div class="text-2xl font-semibold tabular-nums">{{ formatTokens(overview.total_input_tokens + overview.total_output_tokens) }}</div>
        <div class="mt-1 text-[12px] text-text-tertiary">{{ formatTokens(overview.total_input_tokens) }} in · {{ formatTokens(overview.total_output_tokens) }} out</div>
      </div>
      <div class="border-b border-border-subtle pb-4 sm:border-b-0 sm:pb-0">
        <div class="mb-1 text-[13px] text-text-tertiary">Avg latency</div>
        <div class="text-2xl font-semibold tabular-nums">{{ Math.round(overview.avg_latency_ms) }}<span class="text-sm font-normal text-text-tertiary">ms</span></div>
        <div class="mt-1 text-[12px] text-text-tertiary">{{ overview.total_llm_calls }} calls</div>
      </div>
      <div>
        <div class="mb-1 text-[13px] text-text-tertiary">Tool errors</div>
        <div class="text-2xl font-semibold tabular-nums">{{ overview.tool_total_count ? ((overview.tool_error_count / overview.tool_total_count) * 100).toFixed(1) : 0 }}<span class="text-sm font-normal text-text-tertiary">%</span></div>
        <div class="mt-1 text-[12px] text-text-tertiary">{{ overview.tool_error_count }} / {{ overview.tool_total_count }}</div>
      </div>
    </div>

    <div class="mb-3 text-[13px] font-medium text-text-tertiary">Recent sessions</div>
    <div class="divide-y divide-border-subtle">
      <RouterLink
        v-for="s in recentSessions"
        :key="s.id"
        :to="`/sessions/${s.id}`"
        class="-mx-2 flex min-h-12 cursor-pointer items-center justify-between rounded-md px-2 py-2.5 transition-colors duration-100 hover:bg-hover"
      >
        <div class="min-w-0">
          <div class="truncate text-[13px] text-text-primary">{{ s.title || s.id.slice(0, 8) }}</div>
          <div class="text-[12px] text-text-tertiary">{{ s.provider }} · {{ s.model }}</div>
        </div>
        <div class="ml-3 shrink-0 text-[12px] text-text-tertiary">{{ new Date(s.updated_at).toLocaleDateString() }}</div>
      </RouterLink>
      <div v-if="!recentSessions.length" class="py-8 text-center text-[13px] text-text-tertiary">No sessions yet</div>
    </div>
  </div>
</template>
