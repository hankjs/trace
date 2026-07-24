<script setup lang="ts">
const nav = [
  { to: '/', label: '看板', exact: true },
  { to: '/picks', label: '选股池', exact: false },
  { to: '/screener', label: '筛选', exact: false },
  { to: '/signals', label: '信号', exact: false },
  { to: '/portfolio', label: '持仓', exact: false },
  { to: '/backtest', label: '回测', exact: false },
  { to: '/leaderboard', label: '排行', exact: false },
]
</script>

<template>
  <div class="min-h-screen">
    <header class="border-b border-border bg-surface-raised">
      <div class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
        <router-link to="/" class="text-lg font-semibold tracking-wide">
          quant <span class="text-sm font-normal text-text-tertiary">量化看板</span>
        </router-link>
        <nav class="flex gap-1 text-sm">
          <router-link
            v-for="item in nav"
            :key="item.to"
            :to="item.to"
            class="rounded-md px-3 py-1.5 text-text-secondary hover:bg-hover hover:text-text-primary"
            :active-class="item.exact ? '' : '!bg-active !text-text-primary font-medium'"
            :exact-active-class="'!bg-active !text-text-primary font-medium'"
          >
            {{ item.label }}
          </router-link>
        </nav>
      </div>
    </header>
    <main class="mx-auto max-w-6xl px-4 py-6">
      <router-view />
    </main>
  </div>
</template>
