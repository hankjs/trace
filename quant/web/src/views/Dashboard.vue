<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ArrowRight, BarChart3, Bell, CheckCircle2, ClipboardList, Filter, History } from 'lucide-vue-next'
import { api, type PickItem, type SignalItem, type SnapshotItem } from '../api'
import { loadCatalog, reasonText, signalName, templateName } from '../catalog'
import LoadingRows from '../components/LoadingRows.vue'
import PageHeader from '../components/PageHeader.vue'
import { fmtPct, fmtPrice, pnlClass } from '../format'

const snapshot = ref<SnapshotItem[]>([])
const signals = ref<SignalItem[]>([])
const picks = ref<PickItem[]>([])
const picksDate = ref('')
const loading = ref(true)
const error = ref('')

const workflow = [
  { title: '确认数据', detail: '核对行情与研究日期', icon: CheckCircle2, to: '/' },
  { title: '查看候选', detail: '理解入选指标', icon: Filter, to: '/selection?tab=picks' },
  { title: '阅读提示', detail: '确认策略状态变化', icon: Bell, to: '/signals' },
  { title: '历史验证', detail: '比较收益与风险', icon: History, to: '/strategies?tab=backtest' },
  { title: '手工记录', detail: '记录外部已完成成交', icon: ClipboardList, to: '/portfolio' },
]

const latestSnapshotTime = computed(() => {
  const values = snapshot.value.map((item) => item.ts).filter((value): value is string => Boolean(value))
  values.sort()
  return values[values.length - 1] ?? ''
})

const stockMap = computed(() => Object.fromEntries(snapshot.value.map((item) => [item.code, item.name])))

function sourceLabel(item: SnapshotItem): string {
  if (item.source === 'snapshot') return '盘中快照'
  if (item.source === 'close') return '最近收盘'
  return '暂无价格'
}

function displayName(signal: SignalItem): string {
  return signal.name || stockMap.value[signal.code] || '名称待同步'
}

onMounted(async () => {
  await loadCatalog()
  const [snapshotResult, signalResult, picksResult] = await Promise.allSettled([
    api.snapshot(),
    api.signals({ limit: 12 }),
    api.picks(),
  ])

  if (snapshotResult.status === 'fulfilled') snapshot.value = snapshotResult.value.items
  if (signalResult.status === 'fulfilled') signals.value = signalResult.value.items
  if (picksResult.status === 'fulfilled') {
    picks.value = picksResult.value.items
    picksDate.value = picksResult.value.date ?? ''
  }

  const failed = [snapshotResult, signalResult, picksResult].filter((result) => result.status === 'rejected')
  if (failed.length === 3) error.value = (failed[0] as PromiseRejectedResult).reason?.message ?? '今日研究数据加载失败'
  else if (failed.length) error.value = '部分数据暂时不可用，已显示成功加载的内容。'
  loading.value = false
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader title="今日研究" description="从数据状态开始，完成一次可追溯的日频研究。">
      <template #actions>
        <router-link to="/catalog" class="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover">
          <BarChart3 :size="15" /> 查看研究词典
        </router-link>
      </template>
    </PageHeader>

    <p v-if="error" role="status" class="rounded-md border border-warning/25 bg-warning-soft px-4 py-2 text-sm text-warning">{{ error }}</p>
    <LoadingRows v-if="loading" :rows="6" />

    <template v-else>
      <section class="overflow-hidden rounded-md border border-border bg-surface-raised" aria-labelledby="workflow-heading">
        <h2 id="workflow-heading" class="border-b border-border px-4 py-3 text-sm font-semibold">今日研究流程</h2>
        <div class="grid sm:grid-cols-2 lg:grid-cols-5">
          <router-link
            v-for="(step, index) in workflow"
            :key="step.title"
            :to="step.to"
            class="group flex min-h-24 items-start gap-3 border-b border-border-subtle px-4 py-4 last:border-0 hover:bg-hover sm:border-r lg:border-b-0"
          >
            <span class="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-active text-accent">
              <component :is="step.icon" :size="16" />
            </span>
            <span class="min-w-0">
              <span class="block text-xs text-text-tertiary">步骤 {{ index + 1 }}</span>
              <span class="mt-0.5 flex items-center gap-1 text-sm font-medium">
                {{ step.title }} <ArrowRight :size="13" class="opacity-0 transition-opacity group-hover:opacity-100" />
              </span>
              <span class="mt-1 block text-xs leading-5 text-text-tertiary">{{ step.detail }}</span>
            </span>
          </router-link>
        </div>
      </section>

      <section aria-labelledby="status-heading">
        <div class="mb-3 flex items-baseline justify-between gap-3">
          <h2 id="status-heading" class="text-base font-semibold">数据状态</h2>
          <span class="text-xs text-text-tertiary">策略按日线收盘数据计算</span>
        </div>
        <dl class="grid overflow-hidden rounded-md border border-border bg-surface-raised sm:grid-cols-3">
          <div class="border-b border-border-subtle px-4 py-3 sm:border-b-0 sm:border-r">
            <dt class="text-xs text-text-tertiary">关注股票</dt>
            <dd class="mt-1 text-sm font-semibold">{{ snapshot.length }} 只</dd>
            <p class="mt-0.5 text-xs text-text-tertiary">{{ latestSnapshotTime || '等待行情数据' }}</p>
          </div>
          <div class="border-b border-border-subtle px-4 py-3 sm:border-b-0 sm:border-r">
            <dt class="text-xs text-text-tertiary">系统候选</dt>
            <dd class="mt-1 text-sm font-semibold">{{ picks.length }} 只</dd>
            <p class="mt-0.5 text-xs text-text-tertiary">{{ picksDate ? `数据日期 ${picksDate}` : '尚未生成' }}</p>
          </div>
          <div class="px-4 py-3">
            <dt class="text-xs text-text-tertiary">最近提示</dt>
            <dd class="mt-1 text-sm font-semibold">{{ signals.length }} 条</dd>
            <p class="mt-0.5 text-xs text-text-tertiary">仅展示最近加载的记录</p>
          </div>
        </dl>
      </section>

      <section aria-labelledby="watch-heading">
        <div class="mb-3 flex items-baseline justify-between gap-3">
          <h2 id="watch-heading" class="text-base font-semibold">关注股票</h2>
          <span class="text-xs text-text-tertiary">盘中价格仅供显示</span>
        </div>
        <div v-if="snapshot.length" class="overflow-x-auto rounded-md border border-border bg-surface-raised">
          <table class="w-full min-w-[560px] text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs text-text-tertiary">
                <th class="px-4 py-2.5 font-medium">股票</th>
                <th class="px-4 py-2.5 font-medium">数据来源</th>
                <th class="px-4 py-2.5 text-right font-medium">价格</th>
                <th class="px-4 py-2.5 text-right font-medium">涨跌幅</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="item in snapshot" :key="item.code" class="border-b border-border-subtle last:border-0 hover:bg-hover">
                <td class="px-4 py-3">
                  <router-link :to="`/stock/${item.code}`" class="font-medium hover:text-accent">{{ item.name || '名称待同步' }}</router-link>
                  <span class="ml-2 text-xs text-text-tertiary">{{ item.code }}</span>
                </td>
                <td class="px-4 py-3 text-text-secondary">{{ sourceLabel(item) }}</td>
                <td class="px-4 py-3 text-right font-medium tabular-nums" :class="pnlClass(item.pct_chg)">{{ fmtPrice(item.price) }}</td>
                <td class="px-4 py-3 text-right tabular-nums" :class="pnlClass(item.pct_chg)">{{ item.pct_chg === null ? '--' : fmtPct(item.pct_chg / 100) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div v-else class="rounded-md border border-dashed border-border px-5 py-8 text-center text-sm text-text-tertiary">暂无关注股票</div>
      </section>

      <section aria-labelledby="signal-heading">
        <div class="mb-3 flex items-baseline justify-between gap-3">
          <h2 id="signal-heading" class="text-base font-semibold">最近策略提示</h2>
          <router-link to="/signals" class="text-xs text-accent hover:underline">查看全部</router-link>
        </div>
        <div v-if="signals.length" class="divide-y divide-border-subtle rounded-md border border-border bg-surface-raised">
          <article v-for="signal in signals.slice(0, 6)" :key="signal.id" class="grid gap-2 px-4 py-3 sm:grid-cols-[minmax(150px,0.8fr)_minmax(150px,0.8fr)_minmax(240px,1.6fr)_auto] sm:items-center">
            <div>
              <router-link :to="`/stock/${signal.code}`" class="text-sm font-medium hover:text-accent">{{ displayName(signal) }}</router-link>
              <div class="text-xs text-text-tertiary">{{ signal.code }} · {{ signal.date }}</div>
            </div>
            <div>
              <div class="flex items-center gap-1.5 text-sm">
                <span>{{ signal.strategy_name || `策略 ${signal.strategy_id}` }}</span>
                <span v-if="signal.is_system === false" class="rounded bg-active px-1.5 py-0.5 text-[11px] text-accent">自定义</span>
              </div>
              <div class="text-[11px] text-text-tertiary">{{ signal.template ? templateName(signal.template) : '' }}</div>
            </div>
            <p class="text-xs leading-5 text-text-secondary">{{ reasonText(signal.reason, signal.reason_text) }}</p>
            <span class="w-fit rounded-md bg-active px-2 py-1 text-xs font-medium text-accent">{{ signal.side_name || signalName(signal.side) }}</span>
          </article>
        </div>
        <div v-else class="rounded-md border border-dashed border-border px-5 py-8 text-center text-sm text-text-tertiary">暂无策略提示</div>
      </section>
    </template>
  </div>
</template>
