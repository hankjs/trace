<script setup lang="ts">
/** 完整策略规格表单:覆盖 StrategySpec 全部受控字段,表达式部分复用 SpecExpressionEditor。 */
import { computed } from 'vue'
import { Plus, X } from 'lucide-vue-next'
import type { Pool } from '../api'
import type { StrategyEvidenceStatus, StrategySpecFormState } from '../strategySpecForm'
import { parseRejectionRules } from '../strategySpecForm'
import { SUPPORTED_FIELDS, usedFields } from '../specExpression'
import SpecExpressionEditor from './SpecExpressionEditor.vue'

const props = withDefaults(defineProps<{
  pools: Pool[]
  disabled?: boolean
  idPrefix?: string
}>(), {
  disabled: false,
  idPrefix: 'strategy-spec',
})

const model = defineModel<StrategySpecFormState>({ required: true })

const EVIDENCE_STATUS_NAMES: Record<StrategyEvidenceStatus, string> = {
  unverified: '未验证',
  design_complete: '设计完成',
  backtested: '已回测',
  oos_passed: '样本外通过',
  rejected: '已否决',
}

function evidenceStatusName(status: StrategyEvidenceStatus) {
  return EVIDENCE_STATUS_NAMES[status] ?? status
}

/** 结构化否决规则 JSON 非法时给红字提示(构建按无规则处理) */
const rejectionRulesInvalid = computed(
  () => parseRejectionRules(model.value.rejectionRulesText) === null,
)
const rejectionRulesPlaceholder = '[{"metric":"annual_return","op":"lt","threshold":0,"segment":"oos"}]'

function id(name: string) {
  return `${props.idPrefix}-${name}`
}

const isPortfolio = computed(() => model.value.kind === 'portfolio')

// 表达式与启用覆盖层用到的字段必须出现在 data_requirements 且 required=true(后端硬约束)
const missingFields = computed(() => {
  const expressions = [model.value.entryCondition]
  if (isPortfolio.value) {
    expressions.push(model.value.scoreExpression)
    if (model.value.riskFilterEnabled) expressions.push(model.value.riskFilterExpression)
  } else {
    expressions.push(model.value.exitCondition)
    if (model.value.allowAdd) expressions.push(model.value.addCondition)
    if (model.value.allowReduce) expressions.push(model.value.reduceCondition)
  }
  const needed = usedFields(expressions)
  for (const overlay of [
    { enabled: model.value.riskEnabled, type: model.value.riskType },
    { enabled: model.value.takeProfitEnabled, type: model.value.takeProfitType },
  ]) {
    if (!overlay.enabled) continue
    needed.add('close')
    if (overlay.type === 'atr_multiple') {
      needed.add('high')
      needed.add('low')
    }
  }
  const declared = new Set(
    model.value.dataRequirements.filter((item) => item.required).map((item) => item.field),
  )
  return [...needed].filter((name) => !declared.has(name)).sort()
})

function declareMissingFields() {
  for (const name of missingFields.value) {
    model.value.dataRequirements.push({ field: name, availability: 'daily_close', required: true })
  }
}

function addDataRequirement() {
  model.value.dataRequirements.push({ field: 'close', availability: 'daily_close', required: true })
}

function addSource() {
  model.value.sources.push({ book: '', candidateId: '' })
}

function addParameterScan() {
  model.value.parameterScans.push({ path: '$.', values: '' })
}

const inputClass = 'h-9 w-full rounded-md border border-border bg-surface-raised px-2.5 text-sm text-text-primary outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55'
const checkClass = 'h-4 w-4 rounded border-border text-accent focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:opacity-55'
const smallInputClass = 'h-8 w-full rounded-md border border-border bg-surface-raised px-2 text-xs text-text-primary outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed disabled:opacity-55'
const removeButtonClass = 'inline-flex h-8 w-8 shrink-0 items-center justify-center self-end rounded-md border border-border text-text-tertiary hover:bg-hover disabled:opacity-40'
const addButtonClass = 'inline-flex h-7 items-center gap-1 rounded-md border border-dashed border-border px-2.5 text-xs text-text-secondary hover:bg-hover disabled:opacity-40'
</script>

<template>
  <div class="rounded-md border border-border bg-surface-raised">
    <section class="grid gap-4 p-4 md:grid-cols-2" aria-labelledby="spec-basic-heading">
      <div class="md:col-span-2">
        <h4 :id="id('basic-heading')" class="text-sm font-semibold text-text-primary">基本信息与研究范围</h4>
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
      <div class="text-xs font-medium text-text-secondary">
        证据状态
        <div class="mt-1 flex h-9 items-center rounded-md border border-border bg-surface-muted px-2.5 text-sm text-text-primary">
          {{ evidenceStatusName(model.evidenceStatus) }}
        </div>
        <p class="mt-1 text-[11px] font-normal leading-4 text-text-tertiary">
          由回测与否决判定自动推进，仅「标记设计完成 / 否决复位」可在策略管理页手动操作。
        </p>
      </div>
      <label :for="id('pool')" class="text-xs font-medium text-text-secondary">
        股票池
        <select :id="id('pool')" v-model="model.poolId" :disabled="disabled" :class="['mt-1', inputClass]">
          <option :value="null" disabled>请选择股票池</option>
          <option v-for="pool in pools" :key="pool.id" :value="pool.id">{{ pool.name }}</option>
        </select>
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

      <div class="md:col-span-2">
        <span class="text-xs font-medium text-text-secondary">研究来源(1–20 条)</span>
        <div v-for="(source, index) in model.sources" :key="index" class="mt-1.5 flex items-end gap-2">
          <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
            书籍
            <input v-model.trim="source.book" :disabled="disabled" :class="['mt-0.5', smallInputClass]" placeholder="股市趋势技术分析" />
          </label>
          <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
            候选 ID
            <input v-model.trim="source.candidateId" :disabled="disabled" :class="['mt-0.5', smallInputClass]" placeholder="TREND-08" />
          </label>
          <button
            type="button"
            :disabled="disabled || model.sources.length <= 1"
            :class="removeButtonClass"
            :aria-label="`删除来源 ${index + 1}`"
            @click="model.sources.splice(index, 1)"
          >
            <X :size="13" />
          </button>
        </div>
        <button v-if="model.sources.length < 20" type="button" :disabled="disabled" :class="['mt-2', addButtonClass]" @click="addSource">
          <Plus :size="12" />
          添加来源
        </button>
      </div>

      <label :for="id('listing-days')" class="text-xs font-medium text-text-secondary">
        最少上市天数
        <input :id="id('listing-days')" v-model.number="model.minListingDays" type="number" min="0" max="3650" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label :for="id('amount')" class="text-xs font-medium text-text-secondary">
        20 日平均成交额下限(元)
        <input :id="id('amount')" v-model.number="model.minAmountAvg20" type="number" min="0" step="1000000" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <label class="flex items-center gap-2 text-sm text-text-secondary md:col-span-2">
        <input v-model="model.excludeSt" type="checkbox" :disabled="disabled" :class="checkClass" />
        排除 ST 股票
      </label>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-data-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 :id="id('data-heading')" class="text-sm font-semibold text-text-primary">数据需求</h4>
        <span class="text-xs text-text-tertiary">表达式与启用覆盖层用到的字段必须在此声明为必需</span>
      </div>
      <div v-if="missingFields.length" class="mb-3 flex flex-wrap items-center gap-2 rounded-md bg-surface-muted px-3 py-2 text-xs text-warning">
        <span>缺少字段声明:{{ missingFields.join(', ') }}</span>
        <button v-if="!disabled" type="button" class="rounded border border-border px-2 py-0.5 text-text-secondary hover:bg-hover" @click="declareMissingFields">
          一键补充声明
        </button>
      </div>
      <div v-for="(item, index) in model.dataRequirements" :key="index" class="mb-1.5 flex items-end gap-2">
        <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
          字段
          <select v-model="item.field" :disabled="disabled" :class="['mt-0.5', smallInputClass]">
            <option v-for="fieldOption in SUPPORTED_FIELDS" :key="fieldOption.name" :value="fieldOption.name">
              {{ fieldOption.label }} ({{ fieldOption.name }})
            </option>
          </select>
        </label>
        <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
          可用性
          <select v-model="item.availability" :disabled="disabled" :class="['mt-0.5', smallInputClass]">
            <option value="daily_close">日收盘</option>
            <option value="daily_open">日开盘</option>
            <option value="point_in_time">时点数据</option>
          </select>
        </label>
        <label class="flex h-8 shrink-0 items-center gap-1.5 text-xs text-text-secondary">
          <input v-model="item.required" type="checkbox" :disabled="disabled" :class="checkClass" />
          必需
        </label>
        <button
          type="button"
          :disabled="disabled || model.dataRequirements.length <= 1"
          :class="removeButtonClass"
          :aria-label="`删除数据需求 ${index + 1}`"
          @click="model.dataRequirements.splice(index, 1)"
        >
          <X :size="13" />
        </button>
      </div>
      <button v-if="model.dataRequirements.length < 100" type="button" :disabled="disabled" :class="['mt-1', addButtonClass]" @click="addDataRequirement">
        <Plus :size="12" />
        添加字段
      </button>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-entry-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 :id="id('entry-heading')" class="text-sm font-semibold text-text-primary">进场条件</h4>
        <span class="text-xs text-text-tertiary">布尔表达式,T 日收盘确认</span>
      </div>
      <label :for="id('entry-reason')" class="mb-2 block max-w-sm text-xs font-medium text-text-secondary">
        原因码(snake_case)
        <input :id="id('entry-reason')" v-model.trim="model.entryReasonCode" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <SpecExpressionEditor
        v-model="model.entryCondition"
        expected-type="bool"
        :cross-sectional="isPortfolio"
        :disabled="disabled"
      />
    </section>

    <section v-if="!isPortfolio" class="border-t border-border-subtle p-4" aria-labelledby="spec-exit-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 :id="id('exit-heading')" class="text-sm font-semibold text-text-primary">原生离场</h4>
        <span class="text-xs text-text-tertiary">单标的策略必须包含原生离场,风险覆盖不替代它</span>
      </div>
      <label :for="id('exit-reason')" class="mb-2 block max-w-sm text-xs font-medium text-text-secondary">
        原因码(snake_case)
        <input :id="id('exit-reason')" v-model.trim="model.exitReasonCode" :disabled="disabled" :class="['mt-1', inputClass]" />
      </label>
      <SpecExpressionEditor
        v-model="model.exitCondition"
        expected-type="bool"
        :disabled="disabled"
      />
    </section>

    <section v-if="isPortfolio" class="border-t border-border-subtle p-4" aria-labelledby="spec-portfolio-heading">
      <h4 :id="id('portfolio-heading')" class="mb-3 text-sm font-semibold text-text-primary">组合构建</h4>
      <div class="space-y-3">
        <div>
          <div class="mb-1 text-xs font-medium text-text-secondary">评分表达式(数值,横截面排序依据)</div>
          <SpecExpressionEditor
            v-model="model.scoreExpression"
            expected-type="number"
            cross-sectional
            :disabled="disabled"
          />
        </div>
        <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
          <label :for="id('selection-n')" class="text-xs font-medium text-text-secondary">
            入选数量 Top N
            <input :id="id('selection-n')" v-model.number="model.selectionN" type="number" min="1" max="500" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('weighting')" class="text-xs font-medium text-text-secondary">
            权重方法
            <select :id="id('weighting')" v-model="model.weightingType" :disabled="disabled" :class="['mt-1', inputClass]">
              <option value="equal">等权</option>
              <option value="rank">按排名加权</option>
            </select>
          </label>
          <label :for="id('rebalance')" class="text-xs font-medium text-text-secondary">
            调仓周期
            <select :id="id('rebalance')" v-model="model.rebalance" :disabled="disabled" :class="['mt-1', inputClass]">
              <option value="fixed">固定持有周期</option>
              <option value="weekly">每周调仓</option>
              <option value="monthly">每月调仓</option>
            </select>
          </label>
          <label v-if="model.rebalance === 'fixed'" :for="id('rebalance-days')" class="text-xs font-medium text-text-secondary">
            调仓间隔
            <div class="mt-1 flex items-center gap-2">
              <input :id="id('rebalance-days')" v-model.number="model.rebalanceIntervalDays" type="number" min="1" max="250" :disabled="disabled" :class="inputClass" />
              <span class="shrink-0 text-xs text-text-tertiary">日</span>
            </div>
          </label>
        </div>
        <div>
          <label class="flex items-center gap-2 text-sm text-text-secondary">
            <input v-model="model.riskFilterEnabled" type="checkbox" :disabled="disabled" :class="checkClass" />
            启用风险过滤(布尔表达式,为真时剔除候选)
          </label>
          <SpecExpressionEditor
            v-if="model.riskFilterEnabled"
            v-model="model.riskFilterExpression"
            expected-type="bool"
            cross-sectional
            :disabled="disabled"
            class="mt-2"
          />
        </div>
        <div class="grid gap-3 md:grid-cols-3">
          <label :for="id('max-positions')" class="text-xs font-medium text-text-secondary">
            最大持仓数
            <input :id="id('max-positions')" v-model.number="model.maxPositions" type="number" min="1" max="500" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('max-weight')" class="text-xs font-medium text-text-secondary">
            单股最大权重
            <input :id="id('max-weight')" v-model.number="model.maxWeight" type="number" min="0.01" max="1" step="0.01" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('max-total-weight')" class="text-xs font-medium text-text-secondary">
            组合总权重上限
            <input :id="id('max-total-weight')" v-model.number="model.maxTotalWeight" type="number" min="0.01" max="1" step="0.01" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
        </div>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-position-heading">
      <h4 :id="id('position-heading')" class="mb-3 text-sm font-semibold text-text-primary">仓位与持有规则</h4>
      <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <label v-if="!isPortfolio" :for="id('position-type')" class="text-xs font-medium text-text-secondary">
          仓位类型
          <select :id="id('position-type')" v-model="model.positionType" :disabled="disabled" :class="['mt-1', inputClass]">
            <option value="binary">二元(满足条件即到目标仓位)</option>
            <option value="fixed">固定目标仓位</option>
          </select>
        </label>
        <label v-if="!isPortfolio" :for="id('target-weight')" class="text-xs font-medium text-text-secondary">
          目标仓位
          <input :id="id('target-weight')" v-model.number="model.targetWeight" type="number" min="0.01" max="1" step="0.05" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label :for="id('cooldown-days')" class="text-xs font-medium text-text-secondary">
          退出后冷却天数
          <div class="mt-1 flex items-center gap-2">
            <input :id="id('cooldown-days')" v-model.number="model.cooldownDays" type="number" min="0" max="250" :disabled="disabled" :class="inputClass" />
            <span class="shrink-0 text-xs text-text-tertiary">日</span>
          </div>
        </label>
        <label v-if="!isPortfolio && (model.allowAdd || model.allowReduce)" :for="id('position-step')" class="text-xs font-medium text-text-secondary">
          档位大小(0-1,占总资金比例)
          <input :id="id('position-step')" v-model.number="model.positionStep" type="number" min="0.01" max="0.99" step="0.05" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label v-if="!isPortfolio && model.allowAdd" :for="id('max-position')" class="text-xs font-medium text-text-secondary">
          加仓后总仓位上限
          <input :id="id('max-position')" v-model.number="model.maxPosition" type="number" min="0.01" max="1" step="0.05" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
      </div>
      <div v-if="isPortfolio" class="mt-3 rounded-md bg-surface-muted px-3 py-2 text-xs text-text-tertiary">
        组合策略暂不支持加减仓;加仓/减仓规则仅适用于单标的策略。
      </div>
      <template v-else>
        <div class="mt-3 flex flex-wrap gap-6">
          <label class="flex items-center gap-2 text-sm text-text-secondary">
            <input v-model="model.allowAdd" type="checkbox" :disabled="disabled" :class="checkClass" />
            允许加仓(持仓期间条件触发时按档位上调目标仓位)
          </label>
          <label class="flex items-center gap-2 text-sm text-text-secondary">
            <input v-model="model.allowReduce" type="checkbox" :disabled="disabled" :class="checkClass" />
            允许减仓(持仓期间条件触发时按档位下调目标仓位,减到 0 等同离场)
          </label>
        </div>
        <div v-if="model.allowAdd" class="mt-3 rounded-md bg-surface-muted p-3">
          <div class="mb-2 text-xs font-medium text-text-secondary">加仓规则(持仓期间、非进场当日触发)</div>
          <label :for="id('add-reason')" class="mb-2 block max-w-sm text-xs font-medium text-text-secondary">
            原因码(snake_case)
            <input :id="id('add-reason')" v-model.trim="model.addReasonCode" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
          <SpecExpressionEditor
            v-model="model.addCondition"
            expected-type="bool"
            :disabled="disabled"
          />
        </div>
        <div v-if="model.allowReduce" class="mt-3 rounded-md bg-surface-muted p-3">
          <div class="mb-2 text-xs font-medium text-text-secondary">减仓规则(优先级低于离场与风险覆盖)</div>
          <label :for="id('reduce-reason')" class="mb-2 block max-w-sm text-xs font-medium text-text-secondary">
            原因码(snake_case)
            <input :id="id('reduce-reason')" v-model.trim="model.reduceReasonCode" :disabled="disabled" :class="['mt-1', inputClass]" />
          </label>
          <SpecExpressionEditor
            v-model="model.reduceCondition"
            expected-type="bool"
            :disabled="disabled"
          />
        </div>
      </template>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-overlay-heading">
      <h4 :id="id('overlay-heading')" class="mb-3 text-sm font-semibold text-text-primary">风险覆盖</h4>
      <div class="divide-y divide-border-subtle rounded-md bg-surface-muted px-3">
        <div class="grid gap-3 py-3 md:grid-cols-[11rem_1fr_1fr_1fr_6rem] md:items-end">
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
            {{ model.riskType === 'fixed_pct' ? '回撤比例(≤1)' : 'ATR 倍数(≤50)' }}
            <input :id="id('risk-value')" v-model.number="model.riskValue" type="number" min="0.001" :max="model.riskType === 'fixed_pct' ? 1 : 50" step="0.01" :disabled="disabled || !model.riskEnabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('risk-atr')" class="text-xs font-medium text-text-secondary">
            ATR 周期
            <input :id="id('risk-atr')" v-model.number="model.riskAtrPeriod" type="number" min="2" max="250" :disabled="disabled || !model.riskEnabled || model.riskType !== 'atr_multiple'" :class="['mt-1', inputClass]" />
          </label>
          <label class="flex h-9 items-center gap-1.5 text-xs text-text-secondary">
            <input v-model="model.riskTrailing" type="checkbox" :disabled="disabled || !model.riskEnabled" :class="checkClass" />
            跟踪
          </label>
        </div>
        <div class="grid gap-3 py-3 md:grid-cols-[11rem_1fr_1fr_1fr_6rem] md:items-end">
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
            {{ model.takeProfitType === 'fixed_pct' ? '上涨比例(≤1)' : 'ATR 倍数(≤50)' }}
            <input :id="id('profit-value')" v-model.number="model.takeProfitValue" type="number" min="0.001" :max="model.takeProfitType === 'fixed_pct' ? 1 : 50" step="0.01" :disabled="disabled || !model.takeProfitEnabled" :class="['mt-1', inputClass]" />
          </label>
          <label :for="id('profit-atr')" class="text-xs font-medium text-text-secondary">
            ATR 周期
            <input :id="id('profit-atr')" v-model.number="model.takeProfitAtrPeriod" type="number" min="2" max="250" :disabled="disabled || !model.takeProfitEnabled || model.takeProfitType !== 'atr_multiple'" :class="['mt-1', inputClass]" />
          </label>
          <label class="flex h-9 items-center gap-1.5 text-xs text-text-secondary">
            <input v-model="model.takeProfitTrailing" type="checkbox" :disabled="disabled || !model.takeProfitEnabled" :class="checkClass" />
            跟踪
          </label>
        </div>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-execution-heading">
      <div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 :id="id('execution-heading')" class="text-sm font-semibold text-text-primary">成交规则</h4>
        <span class="text-xs text-text-tertiary">T 日收盘信号,T+1 开盘模拟成交</span>
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
            <dd class="mt-0.5 font-medium text-text-secondary">拒绝入场,退出重试</dd>
          </div>
          <div class="rounded-md bg-surface-muted px-3 py-2">
            <dt class="text-text-tertiary">缺少日线</dt>
            <dd class="mt-0.5 font-medium text-text-secondary">拒绝入场,退出重试</dd>
          </div>
        </dl>
        <label :for="id('max-premium')" class="text-xs font-medium text-text-secondary">
          最大入场跳空比例
          <input :id="id('max-premium')" v-model.number="model.maxEntryPremium" type="number" min="0" max="1" step="0.01" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
      </div>
    </section>

    <section class="border-t border-border-subtle p-4" aria-labelledby="spec-validation-heading">
      <h4 :id="id('validation-heading')" class="mb-3 text-sm font-semibold text-text-primary">验证约束</h4>
      <div class="grid gap-3 md:grid-cols-2">
        <label :for="id('baselines')" class="text-xs font-medium text-text-secondary">
          对照策略 ID
          <input :id="id('baselines')" v-model="model.baselineIds" placeholder="buy_and_hold, equal_weight" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label :for="id('rejection-criteria')" class="text-xs font-medium text-text-secondary">
          否决条件(逗号分隔)
          <input :id="id('rejection-criteria')" v-model="model.rejectionCriteria" placeholder="no_net_oos_increment, unstable_parameters" :disabled="disabled" :class="['mt-1', inputClass]" />
        </label>
        <label class="flex items-center gap-2 text-sm text-text-secondary">
          <input v-model="model.lockedOos" type="checkbox" :disabled="disabled" :class="checkClass" />
          使用锁定样本外区间
        </label>
        <label :for="id('rejection-rules')" class="text-xs font-medium text-text-secondary md:col-span-2">
          结构化否决规则(JSON 数组,可选;metric/op/threshold/segment)
          <textarea
            :id="id('rejection-rules')"
            v-model="model.rejectionRulesText"
            rows="3"
            :disabled="disabled"
            :placeholder="rejectionRulesPlaceholder"
            class="mt-1 w-full resize-y rounded-md border px-2.5 py-2 font-mono text-xs leading-5 text-text-primary outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55"
            :class="rejectionRulesInvalid ? 'border-up' : 'border-border'"
          />
          <span v-if="rejectionRulesInvalid" class="mt-1 block text-[11px] text-up">
            JSON 非法,保存时将按无结构化规则处理
          </span>
        </label>
      </div>
      <div class="mt-3">
        <span class="text-xs font-medium text-text-secondary">参数扫描(可选,最多 20 项)</span>
        <div v-for="(scan, index) in model.parameterScans" :key="index" class="mt-1.5 flex items-end gap-2">
          <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
            参数路径($.a.b 形式)
            <input v-model.trim="scan.path" :disabled="disabled" :class="['mt-0.5', smallInputClass]" placeholder="$.entry.condition.args[0].right.window" />
          </label>
          <label class="min-w-0 flex-1 text-[11px] text-text-tertiary">
            候选值(逗号分隔数字)
            <input v-model="scan.values" :disabled="disabled" :class="['mt-0.5', smallInputClass]" placeholder="10, 20, 30" />
          </label>
          <button
            type="button"
            :disabled="disabled"
            :class="removeButtonClass"
            :aria-label="`删除参数扫描 ${index + 1}`"
            @click="model.parameterScans.splice(index, 1)"
          >
            <X :size="13" />
          </button>
        </div>
        <button v-if="model.parameterScans.length < 20" type="button" :disabled="disabled" :class="['mt-2', addButtonClass]" @click="addParameterScan">
          <Plus :size="12" />
          添加参数扫描
        </button>
      </div>
    </section>
  </div>
</template>
