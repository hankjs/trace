<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { Component } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  Activity,
  Bell,
  BookOpen,
  BriefcaseBusiness,
  ChevronDown,
  ChevronRight,
  Database,
  FlaskConical,
  Layers,
  LayoutDashboard,
  ListFilter,
  LogOut,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
  Settings,
  ShieldCheck,
  Star,
  Timer,
  X,
} from 'lucide-vue-next'
import { clearAuth, currentUsername, getToken, isAdmin } from './api'
import { loadCatalog } from './catalog'
import OnboardingGuide from './components/OnboardingGuide.vue'
import QuTour from './components/QuTour.vue'
import ResearchAssistant from './components/ResearchAssistant.vue'
import StockSearchInput from './components/StockSearchInput.vue'
import ThemeToggle from './components/ThemeToggle.vue'
import type { ResearchGuide } from './guides'

const route = useRoute()
const router = useRouter()
const isLoginPage = computed(() => route.name === 'login')
const username = computed(() => currentUsername())
const mobileNavOpen = ref(false)
const sidebarCollapsed = ref(localStorage.getItem('quant_sidebar_collapsed') === 'true')
const stockCode = ref('')
const stockSearch = ref<{ focus: () => void } | null>(null)

function logout() {
  clearAuth()
  router.push('/login')
}

function openStock() {
  if (!stockCode.value) return
  void router.push({ name: 'stock', params: { code: stockCode.value } })
  stockCode.value = ''
}

function toggleSidebar() {
  sidebarCollapsed.value = !sidebarCollapsed.value
  localStorage.setItem('quant_sidebar_collapsed', String(sidebarCollapsed.value))
}

function expandSidebarForGuide() {
  sidebarCollapsed.value = false
  localStorage.setItem('quant_sidebar_collapsed', 'false')
}

interface NavChild {
  to: { name: string }
  name: string
  label: string
}

interface NavItem extends NavChild {
  icon: Component
  children?: NavChild[]
}

const navGroups = computed(() => {
  const groups: { label: string; items: NavItem[] }[] = [
    {
      label: '行情研究',
      items: [
        { to: { name: 'dashboard' }, name: 'dashboard', label: '行情总览', icon: LayoutDashboard },
        { to: { name: 'watchlist' }, name: 'watchlist', label: '自选股', icon: Star },
        { to: { name: 'selection' }, name: 'selection', label: '选股中心', icon: ListFilter },
        { to: { name: 'signals' }, name: 'signals', label: '信号提醒', icon: Bell },
      ],
    },
    {
      label: '研究工具',
      items: [
        { to: { name: 'pools' }, name: 'pools', label: '股票池', icon: Layers },
        {
          to: { name: 'strategies-backtest' },
          name: 'strategies',
          label: '策略研究',
          icon: FlaskConical,
          children: [
            { to: { name: 'strategies-backtest' }, name: 'strategies-backtest', label: '回测验证' },
            { to: { name: 'strategies-experiments' }, name: 'strategies-experiments', label: '试验账本' },
            { to: { name: 'strategies-leaderboard' }, name: 'strategies-leaderboard', label: '策略比较' },
            { to: { name: 'strategies-manage' }, name: 'strategies-manage', label: '策略管理' },
          ],
        },
        { to: { name: 'portfolio' }, name: 'portfolio', label: '持仓记录', icon: BriefcaseBusiness },
        { to: { name: 'catalog' }, name: 'catalog', label: '研究词典', icon: BookOpen },
      ],
    },
  ]
  if (isAdmin()) {
    groups.push({
      label: '系统管理',
      items: [
        { to: { name: 'admin-jobs' }, name: 'admin-jobs', label: '定时任务', icon: Timer },
      ],
    })
  }
  return groups
})

const routeTitles: Record<string, string> = {
  dashboard: '行情总览',
  watchlist: '自选股',
  selection: '选股中心',
  signals: '信号提醒',
  pools: '股票池',
  strategies: '策略研究',
  'strategies-backtest': '回测验证',
  'strategies-experiments': '试验账本',
  'strategies-leaderboard': '策略比较',
  'strategies-manage': '策略管理',
  portfolio: '持仓记录',
  catalog: '研究词典',
  stock: '个股研究',
  settings: '账户设置',
  'admin-jobs': '定时任务',
}

const routeDescriptions: Record<string, string> = {
  watchlist: '自选股票决定行情总览「自选行情」的展示范围，盘中快照也按自选名单采集。',
  selection: '查看每日量化候选，或用技术面、估值和财务条件独立组合筛选。',
  signals: '查看策略在日线数据上发生的状态变化，并阅读产生提示的原因。',
  pools: '选股与回测的研究范围。预置池按成分变动历史逐日解析，自定义池只保存当前名单。',
  strategies: '理解策略规则，用历史日线验证表现，再比较不同策略与参数。',
  'strategies-backtest': '按历史日线模拟策略表现，用于验证规则，不是收益承诺。',
  'strategies-experiments': '记录研究假设、回测证据与结论，跟踪策略从设计到验证的过程。',
  'strategies-leaderboard': '按统一口径比较不同策略与参数的历史模拟表现。',
  'strategies-manage': '新建、调参与维护自己的策略，公共策略只读。',
  portfolio: '记录已在外部交易软件中完成的成交，并查看持仓估值。',
  catalog: '中文名称用于阅读，英文 key 用于对照系统参数。',
  settings: '记录与实盘能力相关的偏好，系统不会代为下单。',
  'admin-jobs': '查看数据采集与研究流水线的调度状态，并可手动触发一次执行。',
}

const currentRouteTitle = computed(() => routeTitles[String(route.name)] ?? '研究工作台')
const currentRouteDescription = computed(() => routeDescriptions[String(route.name)] ?? '')
const currentRouteSection = computed(() => {
  const routeName = String(route.name)
  return navGroups.value.find((group) =>
    group.items.some((item) => item.name === routeName || item.children?.some((child) => child.name === routeName)),
  )?.label ?? '行情研究'
})

function isNavItemActive(item: NavItem) {
  const routeName = String(route.name)
  return item.name === routeName || (item.children?.some((child) => child.name === routeName) ?? false)
}

// 二级菜单默认收起,点击一级目录或子页面激活时展开
const expandedMenus = ref<Record<string, boolean>>({})

function toggleSubmenu(item: NavItem) {
  expandedMenus.value = { ...expandedMenus.value, [item.name]: !expandedMenus.value[item.name] }
}

function isSubmenuExpanded(item: NavItem) {
  if (!item.children) return false
  return Boolean(expandedMenus.value[item.name]) || isNavItemActive(item)
}

const today = new Intl.DateTimeFormat('zh-CN', {
  month: '2-digit',
  day: '2-digit',
  weekday: 'short',
}).format(new Date())

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
  watchlist: {
    title: '自选股提示',
    summary: '自选股是你日常盯盘的名单：行情总览的「自选行情」和盘中快照采集都以它为准。',
    concepts: [
      { term: '自选关系', explanation: '只属于当前账号的名单，不影响其他用户，股票资料本身全系统共享。' },
      { term: '盘中快照', explanation: '交易时段内定时采集的最新价，仅供显示和估值。' },
    ],
    steps: ['搜索并加入自选', '在行情总览查看最新价', '不需要时移出自选'],
    note: '自选只是观察名单，系统不会据此产生任何交易动作。',
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

const guide = computed(() => {
  const routeName = String(route.name)
  const direct = guides[routeName]
  if (direct) return direct
  // 策略研究的四个子页面共用同一份研究提示
  if (routeName.startsWith('strategies-')) return guides.strategies
  return guides.dashboard
})

function onGlobalKeydown(event: KeyboardEvent) {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault()
    void stockSearch.value?.focus()
  }
}

watch(() => route.fullPath, () => {
  mobileNavOpen.value = false
})

watch(mobileNavOpen, (open) => {
  document.body.style.overflow = open ? 'hidden' : ''
})

onMounted(() => {
  if (getToken()) void loadCatalog()
  document.addEventListener('keydown', onGlobalKeydown)
})

onBeforeUnmount(() => {
  document.body.style.overflow = ''
  document.removeEventListener('keydown', onGlobalKeydown)
})
</script>

<template>
  <div v-if="isLoginPage" class="min-h-screen">
    <router-view />
  </div>

  <div v-else class="app-shell flex h-screen min-h-[480px] overflow-hidden bg-surface">
    <aside
      class="hidden h-full shrink-0 flex-col border-r border-border bg-workbench transition-[width] duration-200 lg:flex"
      :class="sidebarCollapsed ? 'w-[52px]' : 'w-[208px]'"
    >
      <router-link to="/" class="flex h-11 shrink-0 items-center gap-2.5 border-b border-border px-3">
        <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded bg-accent text-on-accent">
          <Activity :size="17" />
        </span>
        <span v-if="!sidebarCollapsed" class="min-w-0">
          <span class="block text-sm font-semibold leading-4">Trace Quant</span>
          <span class="block truncate text-[10px] leading-4 text-text-tertiary">日频研究工作台</span>
        </span>
      </router-link>

      <nav class="min-h-0 flex-1 overflow-y-auto px-2 py-2.5" aria-label="主导航">
        <section v-for="group in navGroups" :key="group.label" class="mb-3.5 last:mb-0">
          <h2 v-if="!sidebarCollapsed" class="mb-1 px-2 text-[10px] font-medium text-text-tertiary">{{ group.label }}</h2>
          <div class="space-y-0.5">
            <div v-for="item in group.items" :key="item.name">
              <button
                v-if="item.children"
                type="button"
                class="group flex h-8 w-full items-center gap-2.5 rounded px-2 text-[13px] text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
                :class="{ '!bg-active !text-text-primary font-medium': isNavItemActive(item) }"
                :title="sidebarCollapsed ? item.label : undefined"
                :aria-expanded="isSubmenuExpanded(item)"
                @click="toggleSubmenu(item)"
              >
                <component
                  :is="item.icon"
                  :size="15"
                  class="shrink-0"
                  :class="{ 'text-accent': isNavItemActive(item) }"
                />
                <template v-if="!sidebarCollapsed">
                  <span class="min-w-0 flex-1 truncate text-left">{{ item.label }}</span>
                  <ChevronDown v-if="isSubmenuExpanded(item)" :size="13" class="shrink-0 text-text-tertiary" />
                  <ChevronRight v-else :size="13" class="shrink-0 text-text-tertiary" />
                </template>
              </button>
              <router-link
                v-else
                :to="item.to"
                class="group flex h-8 items-center gap-2.5 rounded px-2 text-[13px] text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
                :class="{ '!bg-active !text-text-primary font-medium': isNavItemActive(item) }"
                :title="sidebarCollapsed ? item.label : undefined"
              >
                <component
                  :is="item.icon"
                  :size="15"
                  class="shrink-0"
                  :class="{ 'text-accent': isNavItemActive(item) }"
                />
                <span v-if="!sidebarCollapsed" class="truncate">{{ item.label }}</span>
              </router-link>
              <div v-if="item.children && !sidebarCollapsed && isSubmenuExpanded(item)" class="mt-0.5 space-y-0.5">
                <router-link
                  v-for="child in item.children"
                  :key="child.name"
                  :to="child.to"
                  class="flex h-7 items-center rounded pl-[35px] pr-2 text-xs text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
                  active-class="!bg-active !text-text-primary font-medium"
                >
                  <span class="truncate">{{ child.label }}</span>
                </router-link>
              </div>
            </div>
          </div>
        </section>
      </nav>

      <ResearchAssistant
        variant="desktop"
        :guide="guide"
        :sidebar-collapsed="sidebarCollapsed"
        @expand-sidebar="expandSidebarForGuide"
      />

      <div class="shrink-0 border-t border-border p-2">
        <div v-if="!sidebarCollapsed" class="mb-2 flex items-start gap-2 rounded bg-surface-muted px-2 py-2 text-[10px] leading-4 text-text-tertiary">
          <ShieldCheck :size="14" class="mt-0.5 shrink-0 text-accent" />
          <span>仅提供日频研究<br />交易均在外部手工完成</span>
        </div>
        <button
          type="button"
          class="icon-button w-full"
          :title="sidebarCollapsed ? '展开导航' : '收起导航'"
          @click="toggleSidebar"
        >
          <PanelLeftOpen v-if="sidebarCollapsed" :size="17" />
          <PanelLeftClose v-else :size="17" />
          <span class="sr-only">{{ sidebarCollapsed ? '展开导航' : '收起导航' }}</span>
        </button>
      </div>
    </aside>

    <div class="flex min-w-0 flex-1 flex-col">
      <header class="z-30 flex h-11 shrink-0 items-center gap-2 border-b border-border bg-workbench px-2.5 lg:px-3">
        <button type="button" class="icon-button lg:!hidden" title="打开导航" @click="mobileNavOpen = true">
          <Menu :size="19" />
          <span class="sr-only">打开导航</span>
        </button>

        <div class="hidden min-w-0 items-center gap-1.5 text-xs sm:flex">
          <span class="shrink-0 text-text-tertiary">{{ currentRouteSection }}</span>
          <span class="text-border">/</span>
          <strong class="shrink-0 font-medium text-text-primary">{{ currentRouteTitle }}</strong>
          <span v-if="currentRouteDescription" class="hidden min-w-0 truncate text-text-tertiary xl:inline">
            · {{ currentRouteDescription }}
          </span>
        </div>

        <form class="ml-auto flex w-full min-w-0 max-w-md items-center" role="search" @submit.prevent="openStock">
          <StockSearchInput
            ref="stockSearch"
            v-model="stockCode"
            hide-label
            placeholder="搜索股票名称或代码"
            input-class="!h-8 !w-full !rounded-r-none !border-r-0 !bg-surface-raised !py-1"
            class="min-w-0 flex-1"
          />
          <button
            type="submit"
            class="flex h-8 w-9 shrink-0 items-center justify-center rounded-r border border-border bg-surface-muted text-text-secondary transition-colors hover:bg-hover hover:text-text-primary"
            title="打开个股研究"
          >
            <Search :size="15" />
            <span class="sr-only">打开个股研究</span>
          </button>
        </form>

        <div class="hidden items-center gap-2.5 text-xs text-text-tertiary 2xl:flex">
          <span>{{ today }}</span>
          <span class="h-3 w-px bg-border" />
          <span class="inline-flex items-center gap-1.5"><span class="h-1.5 w-1.5 rounded-full bg-down" />日频研究模式</span>
        </div>

        <ThemeToggle />
        <span class="hidden max-w-24 truncate text-xs text-text-tertiary md:block">{{ username }}</span>
        <router-link
          :to="{ name: 'settings' }"
          class="icon-button shrink-0"
          title="账户设置"
          active-class="!bg-active !text-text-primary"
        >
          <Settings :size="17" />
          <span class="sr-only">账户设置</span>
        </router-link>
        <button class="icon-button shrink-0" title="退出登录" @click="logout">
          <LogOut :size="17" />
          <span class="sr-only">退出登录</span>
        </button>
      </header>

      <div class="flex min-h-0 min-w-0 flex-1">
        <main class="min-h-0 min-w-0 flex-1 overflow-hidden bg-surface">
          <div class="h-full min-h-0 overflow-y-auto p-3 lg:p-4">
            <router-view />
          </div>
        </main>
      </div>

      <footer class="flex h-6 shrink-0 items-center justify-between gap-3 border-t border-border bg-workbench px-3 text-[10px] text-text-tertiary">
        <div class="flex min-w-0 items-center gap-3">
          <span class="inline-flex items-center gap-1.5 text-text-secondary">
            <Database :size="11" /> 日频数据
          </span>
          <span class="hidden truncate sm:inline">当前工作区：{{ currentRouteTitle }}</span>
        </div>
        <span class="shrink-0">研究与模拟 · 外部手工决策</span>
      </footer>
    </div>

    <ResearchAssistant variant="mobile" :guide="guide" />
    <OnboardingGuide />
    <QuTour />

    <Transition name="nav-drawer">
      <div v-if="mobileNavOpen" class="fixed inset-0 z-50 lg:hidden" @keydown.esc="mobileNavOpen = false">
        <button class="absolute inset-0 bg-overlay" aria-label="关闭导航" @click="mobileNavOpen = false" />
        <aside class="absolute inset-y-0 left-0 flex w-[min(84vw,290px)] flex-col bg-workbench shadow-panel">
          <div class="flex h-11 items-center justify-between border-b border-border px-3">
            <router-link to="/" class="flex items-center gap-2.5">
              <span class="flex h-7 w-7 items-center justify-center rounded bg-accent text-on-accent"><Activity :size="17" /></span>
              <span>
                <span class="block text-sm font-semibold leading-4">Trace Quant</span>
                <span class="block text-[10px] leading-4 text-text-tertiary">日频研究工作台</span>
              </span>
            </router-link>
            <button type="button" class="icon-button" title="关闭导航" @click="mobileNavOpen = false">
              <X :size="18" />
              <span class="sr-only">关闭导航</span>
            </button>
          </div>
          <nav class="flex-1 overflow-y-auto px-3 py-4" aria-label="移动端主导航">
            <section v-for="group in navGroups" :key="group.label" class="mb-5">
              <h2 class="mb-1 px-2 text-xs font-medium text-text-tertiary">{{ group.label }}</h2>
              <template v-for="item in group.items" :key="item.name">
                <button
                  v-if="item.children"
                  type="button"
                  class="flex h-10 w-full items-center gap-3 rounded px-2 text-sm text-text-secondary hover:bg-hover hover:text-text-primary"
                  :class="{ '!bg-active !text-accent font-medium': isNavItemActive(item) }"
                  :aria-expanded="isSubmenuExpanded(item)"
                  @click="toggleSubmenu(item)"
                >
                  <component :is="item.icon" :size="17" />
                  <span class="min-w-0 flex-1 truncate text-left">{{ item.label }}</span>
                  <ChevronDown v-if="isSubmenuExpanded(item)" :size="15" class="shrink-0 text-text-tertiary" />
                  <ChevronRight v-else :size="15" class="shrink-0 text-text-tertiary" />
                </button>
                <router-link
                  v-else
                  :to="item.to"
                  class="flex h-10 items-center gap-3 rounded px-2 text-sm text-text-secondary hover:bg-hover hover:text-text-primary"
                  :class="{ '!bg-active !text-accent font-medium': isNavItemActive(item) }"
                >
                  <component :is="item.icon" :size="17" />
                  {{ item.label }}
                </router-link>
                <template v-if="item.children && isSubmenuExpanded(item)">
                  <router-link
                    v-for="child in item.children"
                    :key="child.name"
                    :to="child.to"
                    class="flex h-9 items-center rounded pl-11 pr-2 text-[13px] text-text-secondary hover:bg-hover hover:text-text-primary"
                    active-class="!bg-active !text-accent font-medium"
                  >
                    {{ child.label }}
                  </router-link>
                </template>
              </template>
            </section>
          </nav>
          <div class="border-t border-border p-3 text-xs leading-5 text-text-tertiary">
            仅提供日频研究与模拟回测，实际交易由用户在外部应用中手工完成。
          </div>
        </aside>
      </div>
    </Transition>
  </div>
</template>
