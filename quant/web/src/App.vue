<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  Bell,
  BookOpen,
  BriefcaseBusiness,
  FlaskConical,
  Layers,
  LayoutDashboard,
  ListFilter,
  LogOut,
} from 'lucide-vue-next'
import { clearAuth, currentUsername, getToken } from './api'
import { loadCatalog } from './catalog'
import ResearchAssistant from './components/ResearchAssistant.vue'
import type { ResearchGuide } from './guides'

const route = useRoute()
const router = useRouter()
const isLoginPage = computed(() => route.name === 'login')
const username = computed(() => currentUsername())

function logout() {
  clearAuth()
  router.push('/login')
}

const nav = [
  { to: { name: 'dashboard' }, name: 'dashboard', label: '今日研究', icon: LayoutDashboard },
  { to: { name: 'selection' }, name: 'selection', label: '选股', icon: ListFilter },
  { to: { name: 'pools' }, name: 'pools', label: '股票池', icon: Layers },
  { to: { name: 'signals' }, name: 'signals', label: '信号提醒', icon: Bell },
  { to: { name: 'strategies' }, name: 'strategies', label: '策略研究', icon: FlaskConical },
  { to: { name: 'portfolio' }, name: 'portfolio', label: '我的持仓', icon: BriefcaseBusiness },
  { to: { name: 'catalog' }, name: 'catalog', label: '研究词典', icon: BookOpen },
]

const guides: Record<string, ResearchGuide> = {
  dashboard: {
    title: '今日研究提示',
    summary: '先确认数据日期，再按选股、信号、回测的顺序完成一次研究。系统只提供研究信息，最终决定由你完成。',
    concepts: [
      { term: '指标', explanation: '用于描述股票的价格、成交或财务特征。' },
      { term: '策略提示', explanation: '一组规则在日线数据上发生状态变化后的提醒。' },
    ],
    steps: ['确认行情和选股池已更新', '查看候选股票及入选原因', '阅读策略提示', '用历史回测验证', '自行决定并手工记录'],
    note: '盘中快照只用于显示和估值，策略按日线收盘数据研究。',
  },
  selection: {
    title: '选股提示',
    summary: '系统候选来自固定评分流程，组合筛选则由你逐条设置条件。两者相互独立，可以交叉验证。',
    concepts: [
      { term: '满足全部', explanation: '股票必须同时符合每个已启用条件，结果通常更少。' },
      { term: '满足任意', explanation: '股票符合其中任一条件即可入选，结果通常更多。' },
    ],
    steps: ['选择系统候选或组合筛选', '查看每条条件单独命中数', '检查每只股票的命中原因', '进入股票详情继续研究'],
    note: '财务指标应关注数据披露日期，历史回测不能使用当时尚未公布的数据。',
  },
  signals: {
    title: '信号提示',
    summary: '信号表示策略状态在某个交易日发生变化，不是自动买卖指令。先阅读原因，再查看股票和策略的历史表现。',
    concepts: [
      { term: '满足入场规则', explanation: '策略状态从未模拟持有变为模拟持有。' },
      { term: '满足退出规则', explanation: '策略状态从模拟持有变为未持有。' },
    ],
    note: '真实买卖和仓位决定均在外部交易软件中手工完成。',
  },
  strategies: {
    title: '策略研究提示',
    summary: '先理解策略规则和限制，再选择股票与时间区间回测。不同市场阶段的表现可能差异很大。',
    concepts: [
      { term: '算法模板', explanation: '系统内置的规则算法，如双均线趋势，决定有哪些参数可调。' },
      { term: '策略', explanation: '一个算法模板加一组参数和你起的名字。公共策略只读，调参请另存为自己的策略。' },
      { term: '回测', explanation: '按历史数据模拟策略表现，用于验证规则，不是收益承诺。' },
      { term: '最大回撤', explanation: '历史模拟净值从高点到低点的最大跌幅。' },
    ],
    steps: ['选择并理解策略', '设置股票与日期', '查看收益和最大回撤', '比较不同参数或策略'],
    note: '回测会受到样本、费用、滑点和数据质量影响。',
  },
  portfolio: {
    title: '持仓记录提示',
    summary: '这里用于记录你已在外部交易软件中完成的真实成交，并根据最新价格估算持仓。',
    concepts: [
      { term: '浮动盈亏', explanation: '按最新显示价格估算、尚未通过卖出确认的盈亏。' },
      { term: '已实现盈亏', explanation: '根据手工录入的卖出记录计算的盈亏。' },
    ],
    note: '记录不是订单，系统不会连接券商或执行交易。',
  },
  pools: {
    title: '股票池提示',
    summary: '股票池决定选股和回测的研究范围。预置池按成分变动历史逐日解析，自定义池只保存当前名单。',
    concepts: [
      { term: '动态解析', explanation: '按每个交易日当时的成分或上市状态确定池内股票，历史口径准确。' },
      { term: '幸存者偏差', explanation: '只用今天仍存在的股票回测过去，已退市或被剔除的股票缺失，结果偏乐观。' },
    ],
    steps: ['选择或新建股票池', '粘贴代码批量导入成员', '在选股和回测页选用该池'],
    note: '自定义池不含成员历史，做历史回测时优先使用预置池。',
  },
  catalog: {
    title: '词典阅读提示',
    summary: '中文名称用于日常阅读，英文 key 用于对照接口、参数和进阶资料。可按名称、说明或 key 搜索。',
    concepts: [
      { term: '口径', explanation: '指标具体使用哪些数据、时间范围和计算方式。' },
      { term: '限制', explanation: '该指标或策略在哪些情况下容易失效或被误读。' },
    ],
  },
  stock: {
    title: '股票详情提示',
    summary: '价格图用于观察历史走势和策略提示出现的位置。先核对数据日期，再结合公司基本面进行判断。',
    concepts: [
      { term: '前复权', explanation: '把历史价格按分红送股因素调整，便于连续比较走势。' },
      { term: '均线', explanation: '一段时间收盘价的平均值，用于观察趋势方向。' },
    ],
    note: '历史走势和提示都不能保证未来结果。',
  },
}

const guide = computed(() => guides[String(route.name)] ?? guides.dashboard)

onMounted(() => {
  if (getToken()) void loadCatalog()
})
</script>

<template>
  <div class="min-h-screen">
    <header v-if="!isLoginPage" class="sticky top-0 z-40 border-b border-border bg-surface-raised/95 backdrop-blur-sm">
      <div class="mx-auto flex max-w-[1600px] flex-wrap items-center gap-x-6 px-4 py-2.5 lg:flex-nowrap lg:px-6">
        <router-link to="/" class="flex shrink-0 items-baseline gap-2 py-1">
          <span class="text-lg font-semibold text-accent">quant</span>
          <span class="text-sm font-medium text-text-primary">量化研究决策工作台</span>
        </router-link>
        <nav class="order-3 mt-2 flex w-full gap-1 overflow-x-auto text-sm lg:order-none lg:mt-0 lg:w-auto" aria-label="主导航">
          <router-link
            v-for="item in nav"
            :key="item.name"
            :to="item.to"
            class="flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-text-secondary hover:bg-hover hover:text-text-primary"
            active-class="!bg-active !text-accent font-medium"
          >
            <component :is="item.icon" :size="15" />
            {{ item.label }}
          </router-link>
        </nav>
        <div class="ml-auto flex items-center gap-2 text-sm">
          <span class="hidden text-text-tertiary sm:inline">{{ username }}</span>
          <button
            class="icon-button"
            title="退出登录"
            @click="logout"
          >
            <LogOut :size="17" />
            <span class="sr-only">退出登录</span>
          </button>
        </div>
      </div>
    </header>
    <main
      class="mx-auto max-w-[1600px] px-4 py-5 lg:px-6 lg:py-6"
      :class="isLoginPage ? '' : 'flex items-start gap-6'"
    >
      <div class="min-w-0 flex-1">
        <router-view />
      </div>
      <ResearchAssistant v-if="!isLoginPage" :guide="guide" />
    </main>
  </div>
</template>
