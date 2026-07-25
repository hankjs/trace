<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import PageHeader from '../components/PageHeader.vue'
import WorkspaceTabs from '../components/WorkspaceTabs.vue'
import Picks from './Picks.vue'
import Screener from './Screener.vue'

const route = useRoute()
const router = useRouter()
const tabs = [
  { key: 'picks', label: '系统候选' },
  { key: 'screener', label: '组合筛选' },
]

const active = computed(() => route.query.tab === 'screener' ? 'screener' : 'picks')
const activeComponent = computed(() => active.value === 'screener' ? Screener : Picks)

function changeTab(tab: string) {
  router.replace({ name: 'selection', query: { ...route.query, tab } })
}
</script>

<template>
  <div class="space-y-5">
    <PageHeader
      title="选股"
      description="查看每日量化候选，或用技术面、估值和财务条件独立组合筛选。"
    />
    <WorkspaceTabs :tabs="tabs" :active="active" @change="changeTab" />
    <KeepAlive>
      <component :is="activeComponent" />
    </KeepAlive>
  </div>
</template>
