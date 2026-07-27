<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ArrowRight, Bell, FlaskConical, ListFilter, RefreshCw } from 'lucide-vue-next'
import { api, type PickItem, type SignalItem, type SnapshotItem } from '../api'
import { loadCatalog, reasonText, signalName, templateName } from '../catalog'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import { fmtPct, fmtPrice, pnlClass } from '../format'

const snapshot = ref<SnapshotItem[]>([])
const signals = ref<SignalItem[]>([])
const picks = ref<PickItem[]>([])
const picksDate = ref('')
const loading = ref(true)
const error = ref('')

const latestSnapshotTime = computed(() => {
  const values = snapshot.value.map((item) => item.ts).filter((value): value is string => Boolean(value))
  values.sort()
  return values[values.length - 1] ?? ''
})

const researchDate = computed(() => picksDate.value || latestSnapshotTime.value.slice(0, 10) || '等待数据')
const stockMap = computed(() => Object.fromEntries(snapshot.value.map((item) => [item.code, item.name])))

function sourceLabel(item: SnapshotItem): string {
  if (item.source === 'snapshot') return '快照'
  if (item.source === 'close') return '收盘'
  return '缺失'
}

function displayName(signal: SignalItem): string {
  return signal.name || stockMap.value[signal.code] || '名称待同步'
}

function sideClass(side: SignalItem['side']): string {
  if (side === 'buy') return 'bg-up/10 text-up'
  if (side === 'sell') return 'bg-down/10 text-down'
  return 'bg-surface-muted text-text-secondary'
}

function sideLabel(signal: SignalItem): string {
  return signal.side_name || signalName(signal.side)
}

async function load() {
  loading.value = true
  error.value = ''
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
  if (failed.length === 3) error.value = (failed[0] as PromiseRejectedResult).reason?.message ?? '研究数据加载失败'
  else if (failed.length) error.value = '部分数据暂时不可用，已显示成功加载的内容。'
  loading.value = false
}

onMounted(load)
</script>

<template>
  <div class="space-y-3">
    <header class="flex min-h-10 flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
      <div class="flex min-w-0 items-baseline gap-3">
        <h1 class="text-base font-semibold">行情总览</h1>
        <span class="hidden text-xs text-text-tertiary sm:inline">研究基准 {{ researchDate }}</span>
      </div>
      <div class="flex items-center gap-1.5 overflow-x-auto">
        <router-link to="/selection?tab=picks" class="workspace-command">
          <ListFilter :size="14" /> 系统选股
        </router-link>
        <router-link to="/signals" class="workspace-command">
          <Bell :size="14" /> 全部提醒
        </router-link>
        <router-link to="/strategies?tab=backtest" class="workspace-command">
          <FlaskConical :size="14" /> 策略回测
        </router-link>
        <button type="button" class="icon-button !h-8 !w-8" title="刷新盘面" :disabled="loading" @click="load">
          <RefreshCw :size="15" :class="loading ? 'animate-spin' : ''" />
          <span class="sr-only">刷新盘面</span>
        </button>
      </div>
    </header>

    <InlineFeedback v-if="error" tone="warning">{{ error }}</InlineFeedback>
    <LoadingRows v-if="loading" :rows="8" />

    <template v-else>
      <dl class="grid grid-cols-2 overflow-hidden border border-border bg-surface-raised xl:grid-cols-4">
        <div class="flex min-h-14 items-center justify-between gap-3 border-b border-r border-border-subtle px-3 py-2 xl:border-b-0">
          <dt class="text-xs text-text-tertiary">数据基准</dt>
          <dd class="text-sm font-semibold">{{ researchDate }}</dd>
        </div>
        <div class="flex min-h-14 items-center justify-between gap-3 border-b border-border-subtle px-3 py-2 xl:border-b-0 xl:border-r">
          <dt class="text-xs text-text-tertiary">自选行情</dt>
          <dd class="text-sm font-semibold">{{ snapshot.length }} <span class="font-normal text-text-tertiary">只</span></dd>
        </div>
        <div class="flex min-h-14 items-center justify-between gap-3 border-r border-border-subtle px-3 py-2">
          <dt class="text-xs text-text-tertiary">系统候选</dt>
          <dd class="text-sm font-semibold">{{ picks.length }} <span class="font-normal text-text-tertiary">只</span></dd>
        </div>
        <div class="flex min-h-14 items-center justify-between gap-3 px-3 py-2">
          <dt class="text-xs text-text-tertiary">近期提示</dt>
          <dd class="text-sm font-semibold">{{ signals.length }} <span class="font-normal text-text-tertiary">条</span></dd>
        </div>
      </dl>

      <div class="grid items-start gap-3 xl:grid-cols-[minmax(420px,1.25fr)_minmax(330px,0.9fr)] 2xl:grid-cols-[minmax(390px,1.08fr)_minmax(320px,0.88fr)_minmax(340px,1fr)]">
        <section class="terminal-panel xl:row-span-2 2xl:row-span-1" aria-labelledby="watch-heading">
          <div class="terminal-panel-header">
            <div class="flex items-baseline gap-2">
              <h2 id="watch-heading" class="text-sm font-semibold">自选行情</h2>
              <span class="text-[11px] text-text-tertiary">{{ latestSnapshotTime || '尚无行情时间' }}</span>
            </div>
            <span class="text-[11px] text-text-tertiary">盘中价格仅供显示</span>
            <router-link to="/watchlist" class="text-[11px] text-accent hover:underline">管理自选</router-link>
          </div>
          <div v-if="snapshot.length" class="max-h-[calc(100vh-214px)] overflow-auto">
            <table class="terminal-table min-w-[510px]">
              <thead>
                <tr class="text-left">
                  <th>名称 / 代码</th>
                  <th>来源</th>
                  <th class="text-right">最新价</th>
                  <th class="text-right">涨跌幅</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="item in snapshot" :key="item.code">
                  <td>
                    <router-link :to="`/stock/${item.code}`" class="font-medium hover:text-accent">{{ item.name || '名称待同步' }}</router-link>
                    <span class="ml-2 text-[11px] text-text-tertiary">{{ item.code }}</span>
                  </td>
                  <td class="text-xs text-text-tertiary">{{ sourceLabel(item) }}</td>
                  <td class="text-right font-medium" :class="pnlClass(item.pct_chg)">{{ fmtPrice(item.price) }}</td>
                  <td class="text-right font-medium" :class="pnlClass(item.pct_chg)">
                    {{ item.pct_chg === null ? '--' : fmtPct(item.pct_chg / 100) }}
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-else class="px-4 py-12 text-center">
            <p class="text-sm text-text-secondary">暂无自选行情</p>
            <router-link to="/watchlist" class="mt-2 inline-flex items-center gap-1 text-xs text-accent hover:underline">
              去添加自选股 <ArrowRight :size="13" />
            </router-link>
          </div>
        </section>

        <section class="terminal-panel" aria-labelledby="picks-heading">
          <div class="terminal-panel-header">
            <div class="flex items-baseline gap-2">
              <h2 id="picks-heading" class="text-sm font-semibold">系统候选</h2>
              <span class="text-[11px] text-text-tertiary">{{ picksDate || '尚未生成' }}</span>
            </div>
            <router-link to="/selection?tab=picks" class="text-xs text-accent hover:underline">完整列表</router-link>
          </div>
          <div v-if="picks.length" class="max-h-[330px] overflow-auto 2xl:max-h-[calc(100vh-214px)]">
            <table class="terminal-table min-w-[380px]">
              <thead>
                <tr class="text-left">
                  <th class="w-10">排名</th>
                  <th>股票</th>
                  <th class="text-right">评分</th>
                  <th class="text-right">20日动量</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="pick in picks" :key="pick.code">
                  <td class="text-xs font-medium text-text-tertiary">{{ pick.rank }}</td>
                  <td>
                    <router-link :to="`/stock/${pick.code}`" class="font-medium hover:text-accent">{{ pick.name || '名称待同步' }}</router-link>
                    <div class="mt-0.5 flex items-center gap-1.5 text-[11px] text-text-tertiary">
                      <span>{{ pick.code }}</span>
                      <span v-if="pick.change === 'new'" class="bg-up/10 px-1 text-up">新进</span>
                    </div>
                  </td>
                  <td class="text-right font-medium">{{ pick.score?.toFixed(3) ?? '--' }}</td>
                  <td class="text-right" :class="pnlClass(pick.factors?.mom20)">{{ fmtPct(pick.factors?.mom20) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-else class="px-4 py-10 text-center text-sm text-text-tertiary">该研究日暂无系统候选</div>
        </section>

        <section class="terminal-panel" aria-labelledby="signal-heading">
          <div class="terminal-panel-header">
            <div class="flex items-baseline gap-2">
              <h2 id="signal-heading" class="text-sm font-semibold">策略提示</h2>
              <span class="text-[11px] text-text-tertiary">状态变化，不是交易指令</span>
            </div>
            <router-link to="/signals" class="text-xs text-accent hover:underline">全部提示</router-link>
          </div>
          <div v-if="signals.length" class="max-h-[360px] divide-y divide-border-subtle overflow-auto 2xl:max-h-[calc(100vh-214px)]">
            <article v-for="signal in signals" :key="signal.id" class="px-3 py-2.5 hover:bg-hover">
              <div class="flex items-start gap-2">
                <div class="min-w-0 flex-1">
                  <div class="flex flex-wrap items-baseline gap-x-2">
                    <router-link :to="`/stock/${signal.code}`" class="text-sm font-medium hover:text-accent">{{ displayName(signal) }}</router-link>
                    <span class="text-[11px] text-text-tertiary">{{ signal.code }} · {{ signal.date }}</span>
                  </div>
                  <div class="mt-1 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
                    <span>{{ signal.strategy_name || `策略 ${signal.strategy_id}` }}</span>
                    <span v-if="signal.template" class="text-[11px] text-text-tertiary">{{ templateName(signal.template) }}</span>
                  </div>
                </div>
                <span class="shrink-0 rounded px-1.5 py-0.5 text-[11px] font-medium" :class="sideClass(signal.side)">
                  {{ sideLabel(signal) }}
                </span>
              </div>
              <p class="mt-1.5 line-clamp-2 text-[11px] leading-4 text-text-tertiary" :title="reasonText(signal.reason, signal.reason_text)">
                {{ reasonText(signal.reason, signal.reason_text) }}
              </p>
            </article>
          </div>
          <div v-else class="px-4 py-10 text-center text-sm text-text-tertiary">暂无策略状态变化</div>
        </section>
      </div>
    </template>
  </div>
</template>
