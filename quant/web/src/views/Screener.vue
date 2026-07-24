<script setup lang="ts">
import { ref } from 'vue'
import { api, type ScreenerItem } from '../api'
import { fmtBigAmount, fmtPct, fmtPrice, pnlClass } from '../format'

const items = ref<ScreenerItem[]>([])
const total = ref(0)
const resultDate = ref('')
const searched = ref(false)
const loading = ref(false)
const error = ref('')

/** 表单输入:涨跌幅和成交额用人类单位(%、亿),提交时换算 */
const form = ref({
  chgMin: '',
  chgMax: '',
  volRatioMin: '',
  maBull: false,
  nearHighDays: '',
  highDistMax: '',
  amountMinYi: '',
})

function num(s: string): number | undefined {
  const v = Number(s)
  return s.trim() !== '' && !Number.isNaN(v) ? v : undefined
}

function chgPct(it: ScreenerItem): number | undefined {
  return it.pct_chg ?? it.chg_pct
}

async function search() {
  loading.value = true
  error.value = ''
  searched.value = true
  try {
    const chgMin = num(form.value.chgMin)
    const chgMax = num(form.value.chgMax)
    const highDistMax = num(form.value.highDistMax)
    const amountYi = num(form.value.amountMinYi)
    const r = await api.screener({
      pct_chg_min: chgMin !== undefined ? chgMin / 100 : undefined,
      pct_chg_max: chgMax !== undefined ? chgMax / 100 : undefined,
      vol_ratio_min: num(form.value.volRatioMin),
      ma_bull: form.value.maBull || undefined,
      high_window: num(form.value.nearHighDays),
      high_dist_max: highDistMax !== undefined ? highDistMax / 100 : undefined,
      amount_min: amountYi !== undefined ? amountYi * 1e8 : undefined,
    })
    items.value = r.items ?? []
    total.value = r.total ?? r.count ?? items.value.length
    resultDate.value = r.date ?? ''
  } catch (e) {
    items.value = []
    total.value = 0
    error.value = (e as Error).message
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="space-y-4">
    <h2 class="text-lg font-semibold">条件筛选</h2>

    <form class="flex flex-wrap items-end gap-3 rounded-lg border border-border bg-surface-raised p-4" @submit.prevent="search">
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">涨跌幅下限 %</span>
        <input v-model="form.chgMin" type="number" step="0.1" placeholder="-10" class="w-24 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">涨跌幅上限 %</span>
        <input v-model="form.chgMax" type="number" step="0.1" placeholder="10" class="w-24 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">量比下限</span>
        <input v-model="form.volRatioMin" type="number" step="0.1" min="0" placeholder="1.5" class="w-24 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">N 日内接近新高</span>
        <input v-model="form.nearHighDays" type="number" min="1" placeholder="60" class="w-28 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">距新高幅度上限 %</span>
        <input v-model="form.highDistMax" type="number" step="0.1" min="0" placeholder="5" class="w-28 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="text-sm">
        <span class="mb-1 block text-xs text-text-tertiary">20日日均成交额下限(亿)</span>
        <input v-model="form.amountMinYi" type="number" step="0.1" min="0" placeholder="1" class="w-36 rounded-md border border-border px-2 py-1.5" />
      </label>
      <label class="flex items-center gap-1.5 pb-1.5 text-sm text-text-secondary">
        <input v-model="form.maBull" type="checkbox" class="accent-accent" />
        均线多头
      </label>
      <button type="submit" :disabled="loading" class="rounded-md bg-accent px-4 py-1.5 text-sm text-white hover:bg-accent-hover disabled:opacity-50">
        {{ loading ? '筛选中…' : '筛选' }}
      </button>
    </form>

    <p v-if="error" class="rounded-md border border-up/30 bg-up/5 px-4 py-2 text-sm text-up">{{ error }}</p>

    <template v-if="searched && !loading && !error">
      <p class="text-sm text-text-secondary">
        共 <span class="font-medium text-text-primary">{{ total }}</span> 只
        <span v-if="resultDate" class="text-text-tertiary">(数据日期 {{ resultDate }})</span>
      </p>

      <div class="overflow-x-auto rounded-lg border border-border bg-surface-raised">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-xs text-text-tertiary">
              <th class="px-4 py-2 font-medium">代码</th>
              <th class="px-4 py-2 font-medium">名称</th>
              <th class="px-4 py-2 text-right font-medium">现价</th>
              <th class="px-4 py-2 text-right font-medium">涨跌幅</th>
              <th class="px-4 py-2 text-right font-medium">量比</th>
              <th class="px-4 py-2 text-right font-medium">20日动量</th>
              <th class="px-4 py-2 text-right font-medium">20日日均成交额</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="it in items"
              :key="it.code"
              class="border-b border-border-subtle last:border-0 hover:bg-hover"
            >
              <td class="px-4 py-2">
                <router-link :to="`/stock/${it.code}`" class="text-accent hover:underline">
                  {{ it.code }}
                </router-link>
              </td>
              <td class="px-4 py-2">{{ it.name }}</td>
              <td class="px-4 py-2 text-right">{{ fmtPrice(it.close) }}</td>
              <td class="px-4 py-2 text-right" :class="pnlClass(chgPct(it))">{{ fmtPct(chgPct(it)) }}</td>
              <td class="px-4 py-2 text-right">{{ it.vol_ratio5?.toFixed(2) ?? '--' }}</td>
              <td class="px-4 py-2 text-right" :class="pnlClass(it.mom20)">{{ fmtPct(it.mom20) }}</td>
              <td class="px-4 py-2 text-right">{{ fmtBigAmount(it.amount_avg20) }}</td>
            </tr>
          </tbody>
        </table>
        <p v-if="!items.length" class="px-4 py-6 text-center text-sm text-text-tertiary">无匹配结果,试试放宽条件</p>
      </div>
    </template>

    <p v-else-if="!searched" class="text-sm text-text-tertiary">设置条件后点击"筛选"查询</p>
  </div>
</template>
