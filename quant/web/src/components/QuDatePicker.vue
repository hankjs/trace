<script setup lang="ts">
/** 通用日期选择:替代原生 <input type="date">,样式与交互对齐设计系统(与 QuSelect 同套模式)。
 *
 * - v-model 为 'YYYY-MM-DD' 字符串,'' 表示未选(clearable 时可通过「清除」置空);
 * - 透传的 class 落在触发按钮上并整体替换默认外观(宽度/高度类按调用方保留);
 * - 弹出层为月份网格:左右箭头切换月份,今天高亮,选中日高亮,非当月日期淡化;
 * - 交互:点击/Enter/Space/ArrowDown 打开,点选某天选中并关闭,Escape 或点击外部关闭;
 * - 面板内按钮一律 mousedown.prevent,焦点留在触发按钮上,与 QuSelect 的关闭模型一致。
 */
import { computed, ref, useAttrs } from 'vue'
import { Calendar, ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { localDateISO } from '../format'

const props = withDefaults(defineProps<{
  placeholder?: string
  disabled?: boolean
  ariaLabel?: string
  /** 是否允许清空(筛选场景);false 时只能改成其他有效日期(等价原生 required 的约束效果) */
  clearable?: boolean
}>(), {
  placeholder: '选择日期',
  disabled: false,
  ariaLabel: undefined,
  clearable: true,
})

defineOptions({ inheritAttrs: false })

const model = defineModel<string>({ required: true })
const emit = defineEmits<{ change: [value: string] }>()

const attrs = useAttrs()
const open = ref(false)

const panelId = `qu-date-${Math.random().toString(36).slice(2)}`

// class 单独处理(整体替换默认外观),其余 attr(id、aria-describedby 等)透传到触发按钮
const buttonAttrs = computed(() => {
  const { class: _class, ...rest } = attrs
  return rest
})

const defaultTriggerClass = 'rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-sm text-text-primary focus:ring-2 focus:ring-accent focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55'

/** 严格解析 'YYYY-MM-DD',非法输入返回 null(按本地时区构造,避免 UTC 偏移) */
function parseISO(value: string): { year: number; month: number; day: number } | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return null
  const year = Number(match[1])
  const month = Number(match[2]) - 1
  const day = Number(match[3])
  const date = new Date(year, month, day)
  if (date.getFullYear() !== year || date.getMonth() !== month || date.getDate() !== day) return null
  return { year, month, day }
}

function toISO(year: number, month: number, day: number): string {
  return `${year}-${String(month + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`
}

const selected = computed(() => parseISO(model.value))
const todayISO = localDateISO()

// 面板当前展示的月份游标:打开时对齐到已选日期,否则对齐今天
const now = new Date()
const viewYear = ref(now.getFullYear())
const viewMonth = ref(now.getMonth())

const WEEKDAYS = ['一', '二', '三', '四', '五', '六', '日']

interface DayCell {
  iso: string
  day: number
  inMonth: boolean
  isToday: boolean
  isSelected: boolean
}

/** 周一开头的 6 行 x 7 列网格,前后补齐到整周 */
const cells = computed<DayCell[]>(() => {
  const first = new Date(viewYear.value, viewMonth.value, 1)
  const offset = (first.getDay() + 6) % 7
  const start = new Date(viewYear.value, viewMonth.value, 1 - offset)
  const result: DayCell[] = []
  for (let i = 0; i < 42; i++) {
    const date = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i)
    const iso = toISO(date.getFullYear(), date.getMonth(), date.getDate())
    result.push({
      iso,
      day: date.getDate(),
      inMonth: date.getMonth() === viewMonth.value,
      isToday: iso === todayISO,
      isSelected: selected.value !== null && iso === model.value,
    })
  }
  return result
})

const title = computed(() => `${viewYear.value} 年 ${viewMonth.value + 1} 月`)

function shiftMonth(step: 1 | -1) {
  const date = new Date(viewYear.value, viewMonth.value + step, 1)
  viewYear.value = date.getFullYear()
  viewMonth.value = date.getMonth()
}

function openPanel() {
  if (props.disabled) return
  const base = selected.value
  viewYear.value = base?.year ?? now.getFullYear()
  viewMonth.value = base?.month ?? now.getMonth()
  open.value = true
}

function closePanel() {
  open.value = false
}

function toggleOpen() {
  if (open.value) closePanel()
  else openPanel()
}

function pick(cell: DayCell) {
  model.value = cell.iso
  emit('change', cell.iso)
  closePanel()
}

function pickToday() {
  model.value = todayISO
  emit('change', todayISO)
  closePanel()
}

function clear() {
  model.value = ''
  emit('change', '')
  closePanel()
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled) return
  if (event.key === 'Enter' || event.key === ' ' || event.key === 'ArrowDown') {
    event.preventDefault()
    if (!open.value) openPanel()
  } else if (event.key === 'Escape' && open.value) {
    event.preventDefault()
    event.stopPropagation()
    closePanel()
  }
}
</script>

<template>
  <div class="relative">
    <button
      v-bind="buttonAttrs"
      type="button"
      :disabled="disabled"
      :aria-label="ariaLabel"
      aria-haspopup="dialog"
      :aria-expanded="open"
      :aria-controls="panelId"
      class="flex items-center gap-2 text-left"
      :class="attrs.class ?? defaultTriggerClass"
      @click="toggleOpen"
      @keydown="onKeydown"
      @blur="closePanel"
    >
      <Calendar :size="15" class="shrink-0 text-text-tertiary" />
      <span class="min-w-0 flex-1 truncate" :class="selected ? '' : 'text-text-tertiary'">
        {{ selected ? model : placeholder }}
      </span>
    </button>

    <div
      v-if="open"
      :id="panelId"
      role="dialog"
      aria-label="选择日期"
      class="absolute left-0 top-full z-30 mt-1 w-64 rounded-md bg-surface-raised p-2 shadow-panel"
    >
      <div class="flex items-center justify-between px-1">
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-md text-text-tertiary hover:bg-hover hover:text-text-primary"
          aria-label="上个月"
          @mousedown.prevent
          @click="shiftMonth(-1)"
        >
          <ChevronLeft :size="15" />
        </button>
        <span class="text-sm font-medium">{{ title }}</span>
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-md text-text-tertiary hover:bg-hover hover:text-text-primary"
          aria-label="下个月"
          @mousedown.prevent
          @click="shiftMonth(1)"
        >
          <ChevronRight :size="15" />
        </button>
      </div>

      <div class="mt-1 grid grid-cols-7 gap-0.5 text-center text-[11px] text-text-tertiary">
        <span v-for="weekday in WEEKDAYS" :key="weekday" class="py-1">{{ weekday }}</span>
      </div>
      <div class="grid grid-cols-7 gap-0.5">
        <button
          v-for="cell in cells"
          :key="cell.iso"
          type="button"
          :data-date="cell.iso"
          :aria-pressed="cell.isSelected"
          :aria-current="cell.isToday ? 'date' : undefined"
          class="flex h-7 items-center justify-center rounded-md text-sm"
          :class="[
            cell.inMonth ? '' : 'text-text-tertiary opacity-50',
            cell.isSelected
              ? 'bg-accent text-on-accent'
              : cell.isToday
                ? 'text-accent hover:bg-hover'
                : 'hover:bg-hover',
          ]"
          @mousedown.prevent
          @click="pick(cell)"
        >
          {{ cell.day }}
        </button>
      </div>

      <div class="mt-1.5 flex items-center justify-between border-t border-border-subtle px-1 pt-1.5">
        <button
          type="button"
          class="rounded-md px-2 py-1 text-xs text-accent hover:bg-hover"
          @mousedown.prevent
          @click="pickToday"
        >
          今天
        </button>
        <button
          v-if="clearable && selected"
          type="button"
          class="rounded-md px-2 py-1 text-xs text-text-tertiary hover:bg-hover hover:text-text-primary"
          @mousedown.prevent
          @click="clear"
        >
          清除
        </button>
      </div>
    </div>
  </div>
</template>
