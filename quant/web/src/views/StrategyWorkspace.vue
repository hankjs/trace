<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import WorkspaceTabs from '../components/WorkspaceTabs.vue'
import Backtest from './Backtest.vue'
import Experiments from './Experiments.vue'
import Leaderboard from './Leaderboard.vue'
import Strategies from './Strategies.vue'

const route = useRoute()
const router = useRouter()
const tabs = [
  { key: 'backtest', label: '回测验证' },
  { key: 'experiments', label: '试验账本' },
  { key: 'leaderboard', label: '策略比较' },
  { key: 'manage', label: '策略管理' },
]

const active = computed(() => {
  const tab = route.query.tab
  if (tab === 'leaderboard' || tab === 'manage' || tab === 'experiments') return tab
  return 'backtest'
})
const activeComponent = computed(() => {
  if (active.value === 'leaderboard') return Leaderboard
  if (active.value === 'manage') return Strategies
  if (active.value === 'experiments') return Experiments
  return Backtest
})

function changeTab(tab: string) {
  router.replace({ name: 'strategies', query: { ...route.query, tab } })
}
</script>

<template>
  <div :class="active === 'manage' ? 'space-y-5 lg:flex lg:h-full lg:min-h-0 lg:flex-col lg:gap-5 lg:space-y-0' : 'space-y-5'">
    <WorkspaceTabs :tabs="tabs" :active="active" @change="changeTab" />
    <div :class="active === 'manage' ? 'lg:min-h-0 lg:flex-1' : ''">
      <KeepAlive>
        <component :is="activeComponent" />
      </KeepAlive>
    </div>
  </div>
</template>
