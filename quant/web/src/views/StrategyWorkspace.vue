<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import PageHeader from '../components/PageHeader.vue'
import WorkspaceTabs from '../components/WorkspaceTabs.vue'
import Backtest from './Backtest.vue'
import Leaderboard from './Leaderboard.vue'
import Strategies from './Strategies.vue'

const route = useRoute()
const router = useRouter()
const tabs = [
  { key: 'backtest', label: '回测验证' },
  { key: 'leaderboard', label: '策略比较' },
  { key: 'manage', label: '策略管理' },
]

const active = computed(() => {
  const tab = route.query.tab
  return tab === 'leaderboard' || tab === 'manage' ? tab : 'backtest'
})
const activeComponent = computed(() => {
  if (active.value === 'leaderboard') return Leaderboard
  if (active.value === 'manage') return Strategies
  return Backtest
})

function changeTab(tab: string) {
  router.replace({ name: 'strategies', query: { ...route.query, tab } })
}
</script>

<template>
  <div class="space-y-5">
    <PageHeader
      title="策略研究"
      description="理解策略规则，用历史日线验证表现，再比较不同策略与参数。"
    />
    <WorkspaceTabs :tabs="tabs" :active="active" @change="changeTab" />
    <KeepAlive>
      <component :is="activeComponent" />
    </KeepAlive>
  </div>
</template>
