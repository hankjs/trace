<script setup lang="ts">
/**
 * 统一股票池选择器。取代各页面自行维护的 universe 下拉。
 *
 * v-model 绑 pool_id(number | null)。null 表示「用后端默认池」,
 * 列表加载完成后会自动落到全部A股。
 */
import { computed, onMounted, watch } from 'vue'
import { AlertTriangle, Settings2 } from 'lucide-vue-next'
import type { Pool } from '../api'
import { hasSurvivorshipBias, usePools } from '../pools'

const props = withDefaults(defineProps<{
  label?: string
  /** 是否展示「管理股票池」入口 */
  manageLink?: boolean
  /** 是否在选中静态池时展示幸存者偏差提示 */
  showBiasHint?: boolean
  disabled?: boolean
}>(), {
  label: '股票池',
  manageLink: true,
  showBiasHint: true,
  disabled: false,
})

const model = defineModel<number | null>({ required: true })
const { pools, loading, error, load, defaultPoolId } = usePools()

const selected = computed<Pool | null>(() =>
  pools.value.find((pool) => pool.id === model.value) ?? null
)

/** 选中静态池时结果含幸存者偏差,由父页面决定是否展示 */
const biased = computed(() => props.showBiasHint && hasSurvivorshipBias(selected.value))

const emit = defineEmits<{ change: [poolId: number | null] }>()

function optionLabel(pool: Pool): string {
  if (pool.kind === 'static') return `${pool.name}（自定义）`
  return pool.name
}

// 池列表就绪后:未选或所选池已不存在(被删除)则落到默认池(全部A股)
watch(pools, (items) => {
  if (!items.length) return
  const stillExists = items.some((pool) => pool.id === model.value)
  if (!stillExists) {
    model.value = defaultPoolId(items as Pool[])
    emit('change', model.value)
  }
}, { immediate: true })

function onSelect(value: number) {
  model.value = value
  emit('change', value)
}

onMounted(() => {
  void load().catch(() => { /* 错误已进 error,页面按需展示 */ })
})
</script>

<template>
  <div class="text-sm">
    <div class="flex items-end gap-2">
      <label class="block">
        <span class="mb-1 block text-xs text-text-tertiary">{{ label }}</span>
        <select
          :value="model ?? ''"
          :disabled="disabled || loading || !pools.length"
          class="min-w-52 rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm disabled:opacity-50"
          :aria-describedby="biased ? 'pool-bias-hint' : undefined"
          @change="onSelect(Number(($event.target as HTMLSelectElement).value))"
        >
          <option v-if="loading" value="">加载中…</option>
          <option v-else-if="!pools.length" value="">暂无可用股票池</option>
          <option v-for="pool in pools" :key="pool.id" :value="pool.id">{{ optionLabel(pool) }}</option>
        </select>
      </label>
      <router-link
        v-if="manageLink"
        :to="{ name: 'pools' }"
        class="mb-0.5 inline-flex items-center gap-1 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-hover hover:text-text-primary"
      >
        <Settings2 :size="14" />
        管理股票池
      </router-link>
    </div>

    <p v-if="error" class="mt-1 text-xs text-up">股票池加载失败：{{ error }}</p>

    <p
      v-else-if="biased"
      id="pool-bias-hint"
      class="mt-1.5 flex items-start gap-1.5 text-xs leading-5 text-text-tertiary"
    >
      <AlertTriangle :size="14" class="mt-0.5 shrink-0 text-warning" />
      <span>
        自定义池只保存当前成员名单、不含成员变动历史，用于历史区间时相当于「用今天的名单跑过去」，
        结果存在<strong class="font-medium text-text-secondary">幸存者偏差</strong>。
        预置池按成分变动历史逐日解析，不受此影响。
      </span>
    </p>
  </div>
</template>
