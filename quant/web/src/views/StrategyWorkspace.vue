<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import PageHeader from '../components/PageHeader.vue'
import WorkspaceTabs from '../components/WorkspaceTabs.vue'
import Backtest from './Backtest.vue'
import Leaderboard from './Leaderboard.vue'

const route = useRoute()
const router = useRouter()
const tabs = [
  { key: 'backtest', label: '回测验证' },
  { key: 'leaderboard', label: '策略比较' },
]

const active = computed(() => route.query.tab === 'leaderboard' ? 'leaderboard' : 'backtest')
const activeComponent = computed(() => active.value === 'leaderboard' ? Leaderboard : Backtest)

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
