<script setup lang="ts">
/** 递归渲染因子预览的 reason_tree,默认折叠子节点。 */
import { ChevronRight } from 'lucide-vue-next'
import type { ReasonNode } from '../api'

const props = defineProps<{
  node: ReasonNode
  depth?: number
}>()

const depth = props.depth ?? 0
const hasChildren = Boolean(props.node.children?.length)

function detailText(): string {
  const parts: string[] = []
  if (props.node.field != null) parts.push(`字段 ${props.node.field}`)
  if (props.node.literal !== undefined && props.node.literal !== null) parts.push(`常量 ${String(props.node.literal)}`)
  if (props.node.window != null) parts.push(`窗口 ${props.node.window}`)
  if (props.node.shift != null) parts.push(`位移 ${props.node.shift}`)
  if (props.node.periods != null) parts.push(`前移 ${props.node.periods}`)
  if (props.node.n != null) parts.push(`N ${props.node.n}`)
  if (props.node.ascending != null) parts.push(props.node.ascending ? '升序' : '降序')
  if (props.node.value !== undefined && props.node.value !== null) parts.push(`结果 ${props.node.value}`)
  return parts.join(' · ') || (hasChildren ? '展开查看子节点' : '')
}

const indentClass = [
  'border-l border-border-subtle pl-3',
  depth > 0 ? 'ml-3 mt-1' : '',
].join(' ')
</script>

<template>
  <details v-if="hasChildren" class="group">
    <summary
      class="flex cursor-pointer list-none items-start gap-1.5 rounded px-1 py-0.5 hover:bg-hover"
      :class="depth > 0 ? 'text-xs' : 'text-sm'"
    >
      <ChevronRight
        :size="13"
        class="mt-0.5 shrink-0 text-text-tertiary transition-transform group-open:rotate-90"
      />
      <span class="font-medium text-text-primary">{{ node.op }}</span>
      <span v-if="detailText()" class="text-text-tertiary">· {{ detailText() }}</span>
    </summary>
    <div :class="indentClass">
      <ReasonTree
        v-for="(child, index) in node.children"
        :key="index"
        :node="child"
        :depth="depth + 1"
      />
    </div>
  </details>

  <div
    v-else
    class="flex items-start gap-1.5 rounded px-1 py-0.5"
    :class="depth > 0 ? 'text-xs' : 'text-sm'"
  >
    <span class="h-3.5 w-3.5 shrink-0" />
    <span class="font-medium text-text-primary">{{ node.op }}</span>
    <span v-if="detailText()" class="text-text-tertiary">· {{ detailText() }}</span>
  </div>
</template>
