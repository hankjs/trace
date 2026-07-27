<script setup lang="ts">
import { computed } from 'vue'
import type { PortfolioWeightChange, ResearchPlanSummary } from '../api'
import { fmtPct } from '../format'
import { portfolioChangeName, priceReferenceText, RESEARCH_PLAN_BOUNDARY, researchPlanStatusName } from '../researchPlans'

const props = defineProps<{ plan: ResearchPlanSummary }>()
const rebalance = computed(() => props.plan.rebalance)

function weightChange(previous: number, target: number): string {
  const delta = target - previous
  return `${delta >= 0 ? '+' : ''}${fmtPct(delta)}`
}

function scoreBreakdown(change: PortfolioWeightChange): string {
  return Object.values(change.score_details ?? {})
    .map((factor) => `${factor.name} ${factor.contribution.toFixed(4)}`)
    .join('；')
}
</script>

<template>
  <section v-if="rebalance" class="border-y border-border bg-surface-raised py-4" aria-labelledby="portfolio-plan-heading">
    <div class="flex flex-wrap items-start justify-between gap-3 px-4">
      <div>
        <h3 id="portfolio-plan-heading" class="text-base font-semibold">调仓研究计划</h3>
        <p class="mt-1 text-xs text-text-tertiary">
          {{ rebalance.pool_name }} · {{ rebalance.frequency }} · 计划日 {{ rebalance.plan_date }} · 下一模拟成交日 {{ rebalance.next_simulated_trade_date }}
        </p>
      </div>
      <div class="text-right text-xs">
        <span class="inline-flex rounded bg-surface-muted px-2 py-1 font-medium">计划状态：{{ plan.status_name || researchPlanStatusName(plan.status) }}</span>
        <p v-if="plan.status_reason" class="mt-1 max-w-md text-text-secondary">{{ plan.status_reason }}</p>
      </div>
    </div>

    <div class="mt-4 overflow-x-auto border-y border-border-subtle">
      <table class="min-w-[58rem] w-full text-sm">
        <thead class="bg-surface-muted text-left text-xs text-text-tertiary">
          <tr>
            <th class="px-4 py-2 font-medium">股票</th>
            <th class="px-4 py-2 font-medium">变化</th>
            <th class="px-4 py-2 text-right font-medium">原权重</th>
            <th class="px-4 py-2 text-right font-medium">调仓目标权重</th>
            <th class="px-4 py-2 text-right font-medium">权重变化</th>
            <th class="px-4 py-2 font-medium">纳入 / 调出原因</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="change in rebalance.changes" :key="change.code" class="border-t border-border-subtle">
            <td class="px-4 py-2">
              <span class="font-medium">{{ change.name || '名称待同步' }}</span>
              <span class="ml-2 text-xs text-text-tertiary">{{ change.code }}</span>
            </td>
            <td class="px-4 py-2"><span class="rounded bg-surface-muted px-1.5 py-0.5 text-xs font-medium">{{ portfolioChangeName(change.change_type, change.change_name) }}</span></td>
            <td class="px-4 py-2 text-right tabular-nums">{{ fmtPct(change.previous_weight) }}</td>
            <td class="px-4 py-2 text-right font-medium tabular-nums">{{ fmtPct(change.target_weight) }}</td>
            <td class="px-4 py-2 text-right tabular-nums">{{ weightChange(change.previous_weight, change.target_weight) }}</td>
            <td class="px-4 py-2 text-xs leading-5 text-text-secondary">
              {{ change.reasons.join('；') || '未提供结构化原因' }}
              <span v-if="scoreBreakdown(change)" class="mt-0.5 block text-text-tertiary">
                评分分解：{{ scoreBreakdown(change) }}
              </span>
              <span v-if="change.risk_reference && !change.risk_rules?.length" class="mt-0.5 block text-text-tertiary">
                {{ change.risk_reference.name }} {{ priceReferenceText(change.risk_reference) }} · {{ change.risk_reference.data_date }}
              </span>
              <span
                v-for="rule in change.risk_rules"
                :key="`${change.code}-${rule.source}`"
                class="mt-0.5 block text-text-tertiary"
              >
                {{ rule.name }}
                {{ rule.price_reference ? priceReferenceText(rule.price_reference) : rule.summary }}
                · {{ rule.data_date }}
              </span>
            </td>
          </tr>
        </tbody>
        <tfoot class="border-t border-border text-xs">
          <tr><td colspan="3" class="px-4 py-2 text-text-tertiary">现金权重</td><td class="px-4 py-2 text-right font-medium">{{ fmtPct(rebalance.cash_weight) }}</td><td colspan="2" /></tr>
        </tfoot>
      </table>
    </div>

    <div v-if="rebalance.risk_summary" class="px-4 pt-3 text-xs leading-5 text-text-secondary">
      <span class="font-medium text-text-primary">组合风险视图：</span>{{ rebalance.risk_summary }}
    </div>
    <p class="mx-4 mt-3 border-t border-border-subtle pt-3 text-xs leading-5 text-text-secondary">{{ RESEARCH_PLAN_BOUNDARY }}</p>
  </section>
</template>
