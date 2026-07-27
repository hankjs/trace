<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { echarts } from '../echarts'
import type { EChartsCoreOption } from 'echarts/core'
import { useTheme } from '../theme'

const props = defineProps<{
  option: EChartsCoreOption
  height?: string
}>()

const el = ref<HTMLDivElement | null>(null)
const { isDark } = useTheme()
let chart: echarts.ECharts | null = null
let ro: ResizeObserver | null = null

function color(token: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(token).trim()
}

function chartTheme() {
  const primary = color('--color-text-primary')
  const secondary = color('--color-text-secondary')
  const tertiary = color('--color-text-tertiary')
  const border = color('--color-border')
  const borderSubtle = color('--color-border-subtle')
  const raised = color('--color-surface-raised')
  const muted = color('--color-surface-muted')
  const active = color('--color-active')

  return {
    darkMode: isDark.value,
    backgroundColor: 'transparent',
    textStyle: { color: secondary },
    title: {
      textStyle: { color: primary },
      subtextStyle: { color: tertiary },
    },
    legend: {
      textStyle: { color: secondary },
      pageTextStyle: { color: tertiary },
    },
    categoryAxis: {
      axisLine: { lineStyle: { color: border } },
      axisTick: { lineStyle: { color: border } },
      axisLabel: { color: tertiary },
      splitLine: { lineStyle: { color: borderSubtle } },
    },
    valueAxis: {
      axisLine: { lineStyle: { color: border } },
      axisTick: { lineStyle: { color: border } },
      axisLabel: { color: tertiary },
      splitLine: { lineStyle: { color: borderSubtle } },
    },
    timeAxis: {
      axisLine: { lineStyle: { color: border } },
      axisTick: { lineStyle: { color: border } },
      axisLabel: { color: tertiary },
      splitLine: { lineStyle: { color: borderSubtle } },
    },
    tooltip: {
      backgroundColor: raised,
      borderColor: border,
      textStyle: { color: primary },
    },
    dataZoom: {
      backgroundColor: muted,
      borderColor: border,
      dataBackground: {
        lineStyle: { color: tertiary },
        areaStyle: { color: borderSubtle },
      },
      selectedDataBackground: {
        lineStyle: { color: secondary },
        areaStyle: { color: active },
      },
      textStyle: { color: tertiary },
    },
  }
}

function initializeChart() {
  if (!el.value) return
  chart?.dispose()
  chart = echarts.init(el.value, chartTheme())
  chart.setOption(props.option)
}

onMounted(() => {
  initializeChart()
  ro = new ResizeObserver(() => chart?.resize())
  if (el.value) ro.observe(el.value)
})

watch(
  () => props.option,
  (opt) => {
    chart?.setOption(opt, { notMerge: true })
  },
  { deep: true }
)

watch(isDark, async () => {
  await nextTick()
  initializeChart()
})

onBeforeUnmount(() => {
  ro?.disconnect()
  chart?.dispose()
  chart = null
})
</script>

<template>
  <div ref="el" :style="{ width: '100%', height: height ?? '480px' }" />
</template>
