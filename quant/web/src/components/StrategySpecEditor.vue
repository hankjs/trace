<script setup lang="ts">
import type { Pool } from '../api'
import type { StrategySpecFormState } from '../strategySpecForm'

const props = withDefaults(defineProps<{
  pools: Pool[]
  disabled?: boolean
  idPrefix?: string
}>(), {
  disabled: false,
  idPrefix: 'strategy-spec',
})

const model = defineModel<StrategySpecFormState>({ required: true })

function id(name: string) {
  return `${props.idPrefix}-${name}`
}

const inputClass = 'h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm text-text-primary outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55'
const checkClass = 'h-4 w-4 rounded border-border text-accent focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:opacity-55'
</script>

<template>
  <div class="rounded-md border border-border bg-surface-raised">
    <section class="grid gap-4 p-4 md:grid-cols-2" aria-labelledby="spec-basic-heading">
      <div class="md:col-span-2">
        <h4 id="spec-basic-heading" class="text-sm font-semibold text-text-primary">基本信息与研究范围</h4>
      </div>
      <label :for="id('kind')" class="text-xs font-medium text-text-secondary">
        策略类型
        <select :id="id('kind')" v-model="model.kind" :disabled="disabled" :class="['mt-1', inputClass]">
          <option value="single">单标的目标仓位</option>
          <option value="portfolio">组合目标权重</option>
        </select>
      </label>
      <label :for="id('canonical-id')" class="text-xs font-medium text-text-secondary">
        规则标识
        <input :id="id('canonical-id')" v-model.trim="model.canonicalId" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label :for="id('source-book')" class="text-xs font-medium text-text-secondary">
        来源书籍（可选）
        <input :id="id('source-book')" v-model.trim="model.sourceBook" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label :for="id('source-candidate')" class="text-xs font-medium text-text-secondary">
        原始候选 ID（可选）
        <input :id="id('source-candidate')" v-model.trim="model.sourceCandidateId" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label :for="id('hypothesis')" class="text-xs font-medium text-text-secondary md:col-span-2">
        研究假设
        <textarea
          :id="id('hypothesis')"
          v-model.trim="model.hypothesis"
          rows="2"
          :disabled="disabled"
          class="mt-1 w-full resize-y rounded-md border border-border bg-surface-raised px-2.5 py-2 text-sm leading-5 text-text-primary outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55"
        />
      </label>
      <label :for="id('pool')" class="text-xs font-medium text-text-secondary">
        股票池
        <select :id="id('pool')" v-model="model.poolId" :disabled="disabled" :class="['mt-1', inputClass]">
          <option :value="null" disabled>请选择股票池</option>
          <option v-for="pool in pools" :key="pool.id" :value="pool.id">{{ pool.name }}</option>
        </select>
      </label>
      <label :for="id('listing-days')" class="text-xs font-medium text-text-secondary">
        最少上市天数
        <input :id="id('listing-days')" v-model.number="model.minListingDays" type="number" min="0" max="5000" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label :for="id('amount')" class="text-xs font-medium text-text-secondary">
        20 日平均成交额下限（元）
        <input :id="id('amount')" v-model.number="model.minAmountAvg20" type="number" min="0" step="1000000" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label class="flex items-center gap-2 text-sm text-text-secondary md:col-span-2">
        <input v-model="model.excludeSt" type="checkbox" :disabled="disabled" :class="checkClass" />
        排除 ST 股票
      </label>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-entry-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 id="spec-entry-heading" class="text-sm font-semibold text-text-primary">进场条件</h4>
        <span class="text-xs text-text-tertiary">所有条件同时满足，T 日收盘确认</span>
      </div>
      <div class="grid gap-3 md:grid-cols-3">
        <label :for="id('breakout-window')" class="text-xs font-medium text-text-secondary">
          突破前期最高价窗口
          <div class="mt-1 flex items-center gap-2">
            <input :id="id('breakout-window')" v-model.number="model.breakoutWindow" type="number" min="2" max="500" :disabled="disabled" :class="inputClass" />
            <span class="shrink-0 text-xs text-text-tertiary">日</span>
          </div>
        </label>
        <label :for="id('volume-window')" class="text-xs font-medium text-text-secondary">
          成交量均值窗口
          <div class="mt-1 flex items-center gap-2">
            <input :id="id('volume-window')" v-model.number="model.volumeWindow" type="number" min="2" max="500" :disabled="disabled" :class="inputClass" />
            <span class="shrink-0 text-xs text-text-tertiary">日</span>
          </div>
        </label>
        <label :for="id('volume-ratio')" class="text-xs font-medium text-text-secondary">
          当前成交量至少为均值
          <div class="mt-1 flex items-center gap-2">
            <input :id="id('volume-ratio')" v-model.number="model.volumeRatio" type="number" min="0.1" max="20" step="0.1" :disabled="disabled" :class="inputClass" />
            <span class="shrink-0 text-xs text-text-tertiary">倍</span>
          </div>
        </label>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-exit-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 id="spec-exit-heading" class="text-sm font-semibold text-text-primary">原生离场</h4>
        <span class="text-xs text-text-tertiary">风险覆盖不替代策略自身退出逻辑</span>
      </div>
      <label :for="id('exit-window')" class="block max-w-sm text-xs font-medium text-text-secondary">
        收盘跌破前期最低价窗口
        <div class="mt-1 flex items-center gap-2">
          <input :id="id('exit-window')" v-model.number="model.exitWindow" type="number" min="2" max="500" :disabled="disabled" :class="inputClass" />
          <span class="shrink-0 text-xs text-text-tertiary">日</span>
        </div>
      </label>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-overlay-heading">
      <h4 id="spec-overlay-heading" class="mb-3 text-sm font-semibold text-text-primary">风险覆盖</h4>
      <div class="divide-y divide-border-subtle rounded-md bg-surface-muted px-3">
        <div class="grid gap-3 py-3 md:grid-cols-[11rem_1fr_1fr_1fr] md:items-end">
          <label class="flex h-9 items-center gap-2 text-sm font-medium text-text-secondary">
            <input v-model="model.riskEnabled" type="checkbox" :disabled="disabled" :class="checkClass" />
            风险止损
          </label>
          <label :for="id('risk-type')" class="text-xs font-medium text-text-secondary">
            口径
            <select :id="id('risk-type')" v-model="model.riskType" :disabled="disabled || !model.riskEnabled" :class="['mt-1', inputClass]">
              <option value="fixed_pct">固定比例</option>
              <option value="atr_multiple">ATR 倍数</option>
            </select>
          </label>
          <label :for="id('risk-value')" class="text-xs font-medium text-text-secondary">
            {{ model.riskType === 'fixed_pct' ? '回撤比例' : 'ATR 倍数' }}
            <input :id="id('risk-value')" v-model.number="model.riskValue" type="number" min="0.001" max="20" step="0.01" :disabled="disabled || !model.riskEnabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('risk-atr')" class="text-xs font-medium text-text-secondary">
            ATR 周期
            <input :id="id('risk-atr')" v-model.number="model.riskAtrPeriod" type="number" min="2" max="250" :disabled="disabled || !model.riskEnabled || model.riskType !== 'atr_multiple'" :class="['mt-1', inputClass]" />
          </label>
        </div>
        <div class="grid gap-3 py-3 md:grid-cols-[11rem_1fr_1fr_1fr] md:items-end">
          <label class="flex h-9 items-center gap-2 text-sm font-medium text-text-secondary">
            <input v-model="model.takeProfitEnabled" type="checkbox" :disabled="disabled" :class="checkClass" />
            止盈覆盖
          </label>
          <label :for="id('profit-type')" class="text-xs font-medium text-text-secondary">
            口径
            <select :id="id('profit-type')" v-model="model.takeProfitType" :disabled="disabled || !model.takeProfitEnabled" :class="['mt-1', inputClass]">
              <option value="fixed_pct">固定比例</option>
              <option value="atr_multiple">ATR 倍数</option>
            </select>
          </label>
          <label :for="id('profit-value')" class="text-xs font-medium text-text-secondary">
            {{ model.takeProfitType === 'fixed_pct' ? '上涨比例' : 'ATR 倍数' }}
            <input :id="id('profit-value')" v-model.number="model.takeProfitValue" type="number" min="0.001" max="20" step="0.01" :disabled="disabled || !model.takeProfitEnabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('profit-atr')" class="text-xs font-medium text-text-secondary">
            ATR 周期
            <input :id="id('profit-atr')" v-model.number="model.takeProfitAtrPeriod" type="number" min="2" max="250" :disabled="disabled || !model.takeProfitEnabled || model.takeProfitType !== 'atr_multiple'" :class="['mt-1', inputClass]" />
          </label>
        </div>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-position-heading">
      <h4 id="spec-position-heading" class="mb-3 text-sm font-semibold text-text-primary">仓位与组合约束</h4>
      <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <label v-if="model.kind === 'single'" :for="id('target-weight')" class="text-xs font-medium text-text-secondary">
          满足条件时目标仓位
          <input :id="id('target-weight')" v-model.number="model.targetWeight" type="number" min="0.01" max="1" step="0.05" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label v-else :for="id('position-type')" class="text-xs font-medium text-text-secondary">
          权重方法
          <select :id="id('position-type')" v-model="model.positionType" :disabled="disabled" :class="['mt-1', inputClass]">
            <option value="equal_weight">等权</option>
            <option value="rank_weight">按排名加权</option>
          </select>
        </label>
        <label v-if="model.kind === 'portfolio'" :for="id('rebalance')" class="text-xs font-medium text-text-secondary">
          调仓周期
          <select :id="id('rebalance')" v-model="model.rebalance" :disabled="disabled" :class="['mt-1', inputClass]">
            <option value="fixed">固定持有周期</option>
            <option value="weekly">每周调仓</option>
          </select>
        </label>
        <label v-if="model.kind === 'portfolio' && model.rebalance === 'fixed'" :for="id('rebalance-days')" class="text-xs font-medium text-text-secondary">
          调仓间隔
          <div class="mt-1 flex items-center gap-2">
            <input :id="id('rebalance-days')" v-model.number="model.rebalanceIntervalDays" type="number" min="1" max="250" :disabled="disabled" :class="inputClass" />
            <span class="shrink-0 text-xs text-text-tertiary">日</span>
          </div>
        </label>
        <label :for="id('max-positions')" class="text-xs font-medium text-text-secondary">
          最大持仓数
          <input :id="id('max-positions')" v-model.number="model.maxPositions" type="number" min="1" max="200" :disabled="disabled || model.kind === 'single'" :class="['mt-1', inputClass]" />
        </label>
        <label :for="id('max-weight')" class="text-xs font-medium text-text-secondary">
          单股最大权重
          <input :id="id('max-weight')" v-model.number="model.maxWeight" type="number" min="0.01" max="1" step="0.01" :disabled="disabled || model.kind === 'single'" :class="['mt-1', inputClass]" />
        </label>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-execution-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 id="spec-execution-heading" class="text-sm font-semibold text-text-primary">成交规则</h4>
        <span class="text-xs text-text-tertiary">T 日收盘信号，T+1 开盘模拟成交</span>
      </div>
      <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-5">
        <dl class="contents text-xs">
          <div class="rounded-md bg-surface-muted px-3 py-2">
            <dt class="text-text-tertiary">涨停无法买入</dt>
            <dd class="mt-0.5 font-medium text-text-secondary">放弃本次入场</dd>
          </div>
          <div class="rounded-md bg-surface-muted px-3 py-2">
            <dt class="text-text-tertiary">跌停无法卖出</dt>
            <dd class="mt-0.5 font-medium text-text-secondary">后续交易日重试</dd>
          </div>
          <div class="rounded-md bg-surface-muted px-3 py-2">
            <dt class="text-text-tertiary">停牌</dt>
            <dd class="mt-0.5 font-medium text-text-secondary">拒绝入场，退出重试</dd>
          </div>
          <div class="rounded-md bg-surface-muted px-3 py-2">
            <dt class="text-text-tertiary">缺少日线</dt>
            <dd class="mt-0.5 font-medium text-text-secondary">拒绝入场，退出重试</dd>
          </div>
        </dl>
        <label :for="id('max-premium')" class="text-xs font-medium text-text-secondary">
          最大入场跳空比例
          <input :id="id('max-premium')" v-model.number="model.maxEntryPremium" type="number" min="0" max="1" step="0.01" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-validation-heading">
      <h4 id="spec-validation-heading" class="mb-3 text-sm font-semibold text-text-primary">验证约束</h4>
      <div class="grid gap-3 md:grid-cols-2">
        <label :for="id('baselines')" class="text-xs font-medium text-text-secondary">
          对照策略 ID
          <input :id="id('baselines')" v-model="model.baselineIds" placeholder="ma_cross, breakout" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label class="flex items-center gap-2 self-end pb-2 text-sm text-text-secondary">
          <input v-model="model.lockedOos" type="checkbox" :disabled="disabled" :class="checkClass" />
          使用锁定样本外区间
        </label>
      </div>
    </section>
  </div>
</template>
