<script setup lang="ts">
import { computed } from 'vue'
import type { StrategyOverlayConfig } from '../api'
import { overlaySummary } from '../researchPlans'

const props = withDefaults(defineProps<{
  disabled?: boolean
  idPrefix?: string
}>(), {
  disabled: false,
  idPrefix: 'strategy-overlay',
})

const risk = defineModel<StrategyOverlayConfig>('risk', { required: true })
const takeProfit = defineModel<StrategyOverlayConfig>('takeProfit', { required: true })

const riskSummary = computed(() => overlaySummary(risk.value, 'risk'))
const takeProfitSummary = computed(() => overlaySummary(takeProfit.value, 'take_profit'))
</script>

<template>
  <div class="space-y-5">
    <section class="border-t border-border-subtle pt-4" :aria-labelledby="`${props.idPrefix}-risk-heading`">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h4 :id="`${props.idPrefix}-risk-heading`" class="text-sm font-semibold">统一风险覆盖层</h4>
          <p class="mt-0.5 text-xs text-text-tertiary">与模板原生风险并行，任一规则先触发就先形成退出状态。</p>
        </div>
        <label class="inline-flex min-h-9 items-center gap-2 text-sm">
          <input
            :id="`${props.idPrefix}-risk-enabled`"
            v-model="risk.enabled"
            type="checkbox"
            :disabled="disabled"
            class="h-4 w-4 rounded border-border disabled:opacity-50"
          />
          启用风险覆盖层
        </label>
      </div>
      <div v-if="risk.enabled" class="mt-3 flex flex-wrap gap-3">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">风险类型</span>
          <select v-model="risk.type" :disabled="disabled" class="h-9 rounded-md border border-border px-2 disabled:opacity-50">
            <option value="fixed_pct">固定百分比</option>
            <option value="atr_multiple">ATR 波动倍数</option>
          </select>
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">{{ risk.type === 'fixed_pct' ? '回落比例（小数）' : 'ATR 倍数' }}</span>
          <input v-model.number="risk.value" type="number" :min="risk.type === 'fixed_pct' ? 0.001 : 0.1" :max="risk.type === 'fixed_pct' ? 1 : 20" :step="risk.type === 'fixed_pct' ? 0.001 : 0.1" :disabled="disabled" class="h-9 w-32 rounded-md border border-border px-2 disabled:opacity-50" />
        </label>
        <label v-if="risk.type === 'atr_multiple'" class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">ATR 周期（交易日）</span>
          <input v-model.number="risk.atr_period" type="number" min="2" max="250" step="1" :disabled="disabled" class="h-9 w-32 rounded-md border border-border px-2 disabled:opacity-50" />
        </label>
      </div>
      <p class="mt-3 rounded-md bg-surface-muted px-3 py-2 text-xs leading-5 text-text-secondary">规则摘要：{{ riskSummary }}</p>
    </section>

    <section class="border-t border-border-subtle pt-4" :aria-labelledby="`${props.idPrefix}-take-profit-heading`">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h4 :id="`${props.idPrefix}-take-profit-heading`" class="text-sm font-semibold">止盈覆盖层</h4>
          <p class="mt-0.5 text-xs text-text-tertiary">默认关闭；启用后作为策略参数进入信号、回测和结果解释。</p>
        </div>
        <label class="inline-flex min-h-9 items-center gap-2 text-sm">
          <input
            :id="`${props.idPrefix}-take-profit-enabled`"
            v-model="takeProfit.enabled"
            type="checkbox"
            :disabled="disabled"
            class="h-4 w-4 rounded border-border disabled:opacity-50"
          />
          启用止盈覆盖层
        </label>
      </div>
      <div v-if="takeProfit.enabled" class="mt-3 flex flex-wrap gap-3">
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">止盈类型</span>
          <select v-model="takeProfit.type" :disabled="disabled" class="h-9 rounded-md border border-border px-2 disabled:opacity-50">
            <option value="fixed_pct">固定收益率</option>
            <option value="atr_multiple">ATR 波动倍数</option>
          </select>
        </label>
        <label class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">{{ takeProfit.type === 'fixed_pct' ? '上涨比例（小数）' : 'ATR 倍数' }}</span>
          <input v-model.number="takeProfit.value" type="number" :min="takeProfit.type === 'fixed_pct' ? 0.001 : 0.1" :max="takeProfit.type === 'fixed_pct' ? 1 : 50" :step="takeProfit.type === 'fixed_pct' ? 0.001 : 0.1" :disabled="disabled" class="h-9 w-32 rounded-md border border-border px-2 disabled:opacity-50" />
        </label>
        <label v-if="takeProfit.type === 'atr_multiple'" class="text-sm">
          <span class="mb-1 block text-xs text-text-tertiary">ATR 周期（交易日）</span>
          <input v-model.number="takeProfit.atr_period" type="number" min="2" max="250" step="1" :disabled="disabled" class="h-9 w-32 rounded-md border border-border px-2 disabled:opacity-50" />
        </label>
      </div>
      <p class="mt-3 rounded-md bg-surface-muted px-3 py-2 text-xs leading-5 text-text-secondary">规则摘要：{{ takeProfitSummary }}</p>
    </section>
  </div>
</template>
