<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { api, type SignalItem, type SnapshotItem } from '../api'
import { fmtPct, fmtPrice, pnlClass } from '../format'

const snapshot = ref<SnapshotItem[]>([])
const signals = ref<SignalItem[]>([])
const loading = ref(true)
const error = ref('')

onMounted(async () => {
  try {
    const [snap, sig] = await Promise.all([
      api.snapshot(),
      api.signals({ limit: 20 }),
    ])
    snapshot.value = snap.items
    signals.value = sig.items
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
})

function sourceLabel(s: SnapshotItem): string {
  if (s.source === 'snapshot') return '盘中'
  if (s.source === 'close') return '收盘'
  return '无数据'
}
</script>

<template>
  <div class="space-y-6">
    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>
    <p v-if="loading" class="text-sm text-text-tertiary">加载中…</p>

    <template v-else>
      <section>
        <h2 class="mb-3 text-base font-semibold">自选股</h2>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          <router-link
            v-for="s in snapshot"
            :key="s.code"
            :to="`/stock/${s.code}`"
            class="rounded-lg border border-border bg-surface-raised p-4 transition hover:border-accent"
          >
            <div class="flex items-baseline justify-between">
              <span class="font-medium">{{ s.name || s.code }}</span>
              <span class="text-xs text-text-tertiary">{{ s.code }}</span>
            </div>
            <div class="mt-2 flex items-baseline justify-between">
              <span class="text-xl font-semibold" :class="pnlClass(s.pct_chg)">
                {{ fmtPrice(s.price) }}
              </span>
              <span class="text-sm" :class="pnlClass(s.pct_chg)">
                {{ s.pct_chg === null ? '--' : fmtPct(s.pct_chg / 100) }}
              </span>
            </div>
            <div class="mt-1 text-xs text-text-tertiary">
              {{ sourceLabel(s) }}<template v-if="s.ts"> · {{ s.ts }}</template>
            </div>
          </router-link>
        </div>
        <p v-if="!snapshot.length" class="text-sm text-text-tertiary">暂无自选股</p>
      </section>

      <section>
        <h2 class="mb-3 text-base font-semibold">最近信号</h2>
        <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs text-text-tertiary">
                <th class="px-4 py-2 font-medium">日期</th>
                <th class="px-4 py-2 font-medium">代码</th>
                <th class="px-4 py-2 font-medium">策略</th>
                <th class="px-4 py-2 font-medium">方向</th>
                <th class="px-4 py-2 text-right font-medium">价格</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="sig in signals"
                :key="sig.id"
                class="border-b border-border-subtle last:border-0 hover:bg-hover"
              >
                <td class="px-4 py-2">{{ sig.date }}</td>
                <td class="px-4 py-2">
                  <router-link :to="`/stock/${sig.code}`" class="text-accent hover:underline">
                    {{ sig.code }}
                  </router-link>
                </td>
                <td class="px-4 py-2">{{ sig.strategy }}</td>
                <td class="px-4 py-2">
                  <span :class="sig.side === 'buy' ? 'text-up' : 'text-down'" class="font-medium">
                    {{ sig.side === 'buy' ? '买入' : '卖出' }}
                  </span>
                </td>
                <td class="px-4 py-2 text-right">{{ fmtPrice(sig.price) }}</td>
              </tr>
            </tbody>
          </table>
          <p v-if="!signals.length" class="px-4 py-6 text-center text-sm text-text-tertiary">暂无信号</p>
        </div>
      </section>
    </template>
  </div>
</template>
