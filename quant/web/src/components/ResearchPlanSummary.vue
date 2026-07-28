<script setup lang="ts">
import { computed } from 'vue'
import { CircleCheck, CircleX, Clock3, LogOut, TriangleAlert } from 'lucide-vue-next'
import type { ResearchExitRule, ResearchPlan, ResearchPlanSummary, SignalItem } from '../api'
import { fmtPct, fmtPrice } from '../format'
import { priceReferenceText, RESEARCH_PLAN_BOUNDARY, researchPlanStatusName } from '../researchPlans'

const props = defineProps<{
  plan?: ResearchPlanSummary | ResearchPlan | null
  signal?: SignalItem | null
}>()

const status = computed(() => props.plan?.status ?? props.signal?.plan_status ?? null)
const statusName = computed(() => props.plan?.status_name || props.signal?.plan_status_name || researchPlanStatusName(status.value))
const signalClose = computed(() => props.plan?.signal_close_price ?? props.signal?.signal_close_price ?? props.signal?.price ?? null)
const isHoldingSnapshot = computed(() =>
  props.plan?.signal_type === 'hold'
  || (props.plan && 'signal_side' in props.plan && props.plan.signal_side === 'hold')
)
const riskRules = computed(() => props.plan?.risk_rules ?? [])
const takeProfitRules = computed(() => props.plan?.take_profit_rules ?? [])
const nativeExitRules = computed(() => props.plan?.native_exit_rules ?? [])
const fullPlan = computed<ResearchPlan | null>(() => {
  const plan = props.plan
  return plan && 'params_snapshot' in plan ? plan as ResearchPlan : null
})

const statusIcon = computed(() => {
  if (status.value === 'active') return CircleCheck
  if (status.value === 'needs_review') return TriangleAlert
  if (status.value === 'exit_triggered') return LogOut
  if (status.value === 'invalidated') return CircleX
  return Clock3
})

function statusClass(): string {
  if (status.value === 'active') return 'bg-info-soft text-accent'
  if (status.value === 'needs_review' || status.value === 'expired') return 'bg-warning-soft text-warning'
  if (status.value === 'invalidated' || status.value === 'exit_triggered') return 'bg-danger-soft text-up'
  return 'bg-surface-muted text-text-secondary'
}

function ruleValue(rule: ResearchExitRule): string {
  if (rule.price_reference) return `${priceReferenceText(rule.price_reference)}（${rule.price_reference.data_date}）`
  if (rule.current_value != null) return `${rule.current_value}${rule.unit ?? ''}`
  return rule.dynamic ? '随收盘数据动态计算' : '按条件判断'
}

function parameterText(): string {
  const params = fullPlan.value?.params_snapshot ?? {}
  return Object.entries(params)
    .filter(([, value]) => typeof value !== 'object')
    .map(([key, value]) => `${key}=${String(value)}`)
    .join('，')
}

function costText(): string {
  const costs = props.plan?.evidence?.costs
  if (!costs) return ''
  const labels: Record<string, string> = {
    commission: '佣金',
    stamp_tax: '印花税',
    slippage: '滑点',
  }
  return Object.entries(labels)
    .filter(([key]) => typeof costs[key] === 'number')
    .map(([key, label]) => `${label} ${((costs[key] as number) * 100).toFixed(2)}%`)
    .join('，')
}
</script>

<template>
  <article class="bg-surface-muted px-4 py-4 sm:px-5" aria-label="策略研究计划摘要">
    <div class="mb-4 flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
      <div>
        <h3 class="text-sm font-semibold">{{ plan?.type === 'portfolio_rebalance' ? '调仓研究计划' : '策略研究计划' }}</h3>
        <p class="mt-0.5 text-xs text-text-tertiary">日频研究参考</p>
      </div>
      <span class="inline-flex items-center gap-1.5 rounded px-2 py-1 text-xs font-medium" :class="statusClass()">
        <component :is="statusIcon" :size="14" aria-hidden="true" />
        {{ statusName }}
      </span>
    </div>

    <div class="grid gap-x-6 gap-y-5 lg:grid-cols-2">
      <section aria-labelledby="plan-data-heading">
        <h4 id="plan-data-heading" class="text-xs font-semibold text-text-primary">1. 数据与信号</h4>
        <dl class="mt-2 grid grid-cols-[7.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs leading-5">
          <dt class="text-text-tertiary">数据基准日</dt>
          <dd>{{ plan?.data_date ?? signal?.date ?? '待生成' }}</dd>
          <dt class="text-text-tertiary">信号日收盘价</dt>
          <dd v-if="isHoldingSnapshot">不适用 <span class="text-text-tertiary">（持仓快照未生成新信号）</span></dd>
          <dd v-else>{{ signalClose == null ? '未提供' : fmtPrice(signalClose) }} <span class="text-text-tertiary">（不是建议成交价）</span></dd>
          <dt class="text-text-tertiary">下一模拟成交日</dt>
          <dd>{{ plan?.next_simulated_trade_date ?? '按下一可交易日确定' }}</dd>
          <template v-if="plan?.status_reason">
            <dt class="text-text-tertiary">状态原因</dt>
            <dd class="font-medium">{{ plan.status_reason }}</dd>
          </template>
          <template v-if="fullPlan">
            <dt class="text-text-tertiary">策略版本</dt>
            <dd>{{ fullPlan.strategy_version }} · {{ fullPlan.adjustment }}</dd>
            <dt class="text-text-tertiary">参数快照</dt>
            <dd>{{ parameterText() || '覆盖层参数见风险与退出' }}</dd>
          </template>
        </dl>
      </section>

      <section aria-labelledby="plan-entry-heading">
        <h4 id="plan-entry-heading" class="text-xs font-semibold text-text-primary">2. 进场观察</h4>
        <div v-if="plan?.entry" class="mt-2 text-xs leading-5">
          <p>{{ plan.entry.summary }}</p>
          <p v-if="plan.entry.line" class="mt-1 text-text-secondary">
            进场观察线：<strong class="font-medium text-text-primary">{{ priceReferenceText(plan.entry.line) }}</strong>
            <span class="text-text-tertiary"> · {{ plan.entry.line.data_date }}</span>
          </p>
          <p v-if="plan.entry.range" class="mt-1 text-text-secondary">
            进场观察区间：<strong class="font-medium text-text-primary">{{ priceReferenceText(plan.entry.range) }}</strong>
            <span class="text-text-tertiary"> · {{ plan.entry.range.data_date }}</span>
          </p>
          <p v-if="plan.entry.review_condition" class="mt-1 text-text-tertiary">重评条件：{{ plan.entry.review_condition }}</p>
          <ul v-if="plan.entry.conditions?.length" class="mt-2 space-y-1">
            <li v-for="condition in plan.entry.conditions" :key="condition.id ?? condition.name">
              <span class="font-medium">{{ condition.name }}</span>：{{ condition.summary }}
            </li>
          </ul>
        </div>
        <p v-else class="mt-2 text-xs leading-5 text-text-tertiary">完整计划生成后显示客观观察线、区间或指标关系；不会为字段齐全推测价格。</p>
      </section>

      <section aria-labelledby="plan-exit-heading">
        <h4 id="plan-exit-heading" class="text-xs font-semibold text-text-primary">3. 风险与退出</h4>
        <div class="mt-2 space-y-2 text-xs leading-5">
          <div>
            <span class="font-medium">风险失效条件</span>
            <ul v-if="riskRules.length" class="mt-1 space-y-1 text-text-secondary">
              <li v-for="rule in riskRules" :key="rule.id ?? rule.name">{{ rule.name }}：{{ rule.summary }}；当前参考 {{ ruleValue(rule) }}</li>
            </ul>
            <p v-else class="mt-1 text-text-tertiary">完整计划生成后显示模板原生风险与已启用覆盖层。</p>
          </div>
          <div>
            <span class="font-medium">止盈条件</span>
            <ul v-if="takeProfitRules.length" class="mt-1 space-y-1 text-text-secondary">
              <li v-for="rule in takeProfitRules" :key="rule.id ?? rule.name">{{ rule.name }}：{{ rule.summary }}；当前参考 {{ ruleValue(rule) }}</li>
            </ul>
            <p v-else class="mt-1 text-text-tertiary">未设置止盈时，按风险规则或策略原生条件退出。</p>
          </div>
          <div>
            <span class="font-medium">策略退出条件</span>
            <ul v-if="nativeExitRules.length" class="mt-1 space-y-1 text-text-secondary">
              <li v-for="rule in nativeExitRules" :key="rule.id ?? rule.name">{{ rule.name }}：{{ rule.summary }}；当前参考 {{ ruleValue(rule) }}</li>
            </ul>
            <p v-else class="mt-1 text-text-tertiary">完整计划生成后显示模板自身的退出条件和当前距离。</p>
          </div>
        </div>
      </section>

      <section aria-labelledby="plan-evidence-heading">
        <h4 id="plan-evidence-heading" class="text-xs font-semibold text-text-primary">4. 历史回测对照</h4>
        <div v-if="plan?.evidence?.status === 'verified' && plan.evidence.exact_match" class="mt-2 text-xs leading-5">
          <p>已有同配置历史回测 #{{ plan.evidence.backtest_id }}（模拟记录，非科学证实）</p>
          <dl class="mt-1 grid grid-cols-2 gap-x-4 gap-y-1 text-text-secondary">
            <div>区间收益：{{ plan.evidence.metrics?.total_return == null ? '未提供' : fmtPct(plan.evidence.metrics.total_return) }}</div>
            <div>最大回撤：{{ plan.evidence.metrics?.max_drawdown == null ? '未提供' : fmtPct(plan.evidence.metrics.max_drawdown) }}</div>
            <div>胜率：{{ plan.evidence.metrics?.win_rate == null ? '未提供' : fmtPct(plan.evidence.metrics.win_rate) }}</div>
            <div>交易次数：{{ plan.evidence.metrics?.trade_count ?? '未提供' }}</div>
          </dl>
          <p v-if="costText()" class="mt-1 text-text-secondary">费用假设：{{ costText() }}</p>
          <p class="mt-1 text-text-tertiary">{{ plan.evidence.start }} 至 {{ plan.evidence.end }}，历史结果不代表未来表现。</p>
        </div>
        <div v-else class="mt-2 text-xs leading-5 text-text-tertiary">
          <p>尚无匹配回测：没有与当前策略版本、参数、覆盖层和费用完全一致的模拟记录。</p>
          <p v-if="costText()" class="mt-1 text-text-secondary">待对照费用口径：{{ costText() }}</p>
        </div>
      </section>
    </div>

    <section class="mt-5 border-t border-border pt-3" aria-labelledby="plan-boundary-heading">
      <h4 id="plan-boundary-heading" class="text-xs font-semibold text-text-primary">5. 产品边界</h4>
      <p class="mt-1 max-w-[75ch] text-xs leading-5 text-text-secondary">{{ RESEARCH_PLAN_BOUNDARY }}</p>
    </section>
  </article>
</template>
