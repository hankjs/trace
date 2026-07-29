<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
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
    <WorkspaceTabs :tabs="tabs" :active="active" data-tour="selection-tabs" @change="changeTab" />
    <KeepAlive>
      <component :is="activeComponent" />
    </KeepAlive>
  </div>
</template>
