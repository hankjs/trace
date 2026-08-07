<script setup lang="ts">
import ExploreTimelineCard from './ExploreTimelineCard.vue'
import type { TimelineItem } from './types'

defineProps<{ items: TimelineItem[] }>()
</script>

<template>
  <div class="relative">
    <!-- 桌面双栏中轴 -->
    <div class="absolute bottom-0 left-1/2 top-0 hidden w-px -translate-x-px bg-border md:block"></div>

    <!-- 桌面列头 -->
    <div class="mb-3 hidden grid-cols-2 gap-4 text-[11px] font-medium uppercase tracking-wide text-text-tertiary md:grid">
      <div class="pr-6 text-right">User</div>
      <div class="pl-6">ExploreAgent</div>
    </div>

    <!-- 移动端：单列时间线；桌面：双栏 -->
    <div
      v-for="item in items"
      :key="item.id"
      class="relative mb-3 md:mb-1.5 md:grid md:grid-cols-2 md:gap-4"
    >
      <div class="absolute left-1/2 top-2 z-10 hidden -translate-x-1/2 md:block">
        <div
          class="h-2.5 w-2.5 rounded-full border-2 border-surface-base"
          :class="{
            'bg-blue-500': item.color === 'blue',
            'bg-indigo-500': item.color === 'indigo',
            'bg-gray-400': item.color === 'gray',
            'bg-green-500': item.color === 'green',
            'bg-purple-500': item.color === 'purple',
            'bg-amber-500': item.color === 'amber',
            'bg-emerald-500': item.color === 'emerald',
            'bg-sky-500': item.color === 'sky',
          }"
        ></div>
      </div>

      <!-- 移动：整行展示，带侧别标签 -->
      <div class="md:hidden">
        <div class="mb-1 flex items-center gap-2 text-[10px] font-medium uppercase tracking-wide text-text-tertiary">
          <span
            class="inline-block h-2 w-2 rounded-full"
            :class="{
              'bg-blue-500': item.color === 'blue',
              'bg-indigo-500': item.color === 'indigo',
              'bg-gray-400': item.color === 'gray',
              'bg-green-500': item.color === 'green',
              'bg-purple-500': item.color === 'purple',
              'bg-amber-500': item.color === 'amber',
              'bg-emerald-500': item.color === 'emerald',
              'bg-sky-500': item.color === 'sky',
            }"
          ></span>
          {{ item.side === 'user' ? 'User' : 'ExploreAgent' }}
        </div>
        <ExploreTimelineCard :item="item" :side="item.side" />
      </div>

      <!-- 桌面左列 (user) -->
      <div class="hidden justify-end pr-5 md:flex">
        <ExploreTimelineCard v-if="item.side === 'user'" :item="item" side="user" />
      </div>

      <!-- 桌面右列 (agent) -->
      <div class="hidden pl-5 md:block">
        <ExploreTimelineCard v-if="item.side === 'agent'" :item="item" side="agent" />
      </div>
    </div>
  </div>
</template>
