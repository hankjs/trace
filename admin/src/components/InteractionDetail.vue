<script setup lang="ts">
/** 交互单展开详情：纯展示，不渲染 markdown */
import { RouterLink } from 'vue-router'
import type { AgentInteraction } from '../composables/api'

const props = defineProps<{ row: AgentInteraction }>()

function parseOptions(raw: string): string[] {
  try {
    const v = JSON.parse(raw)
    return Array.isArray(v) ? v.map(String) : []
  } catch {
    return []
  }
}

const optionsText = () => {
  const opts = parseOptions(props.row.options)
  return opts.length ? opts.join(' / ') : props.row.options
}
</script>

<template>
  <div class="px-3 py-3 text-xs text-text-secondary space-y-2">
    <div>
      <span class="text-text-tertiary">会话 </span>
      <RouterLink
        :to="`/sessions/${row.session_id}`"
        class="text-accent hover:underline font-mono"
      >{{ row.session_id }}</RouterLink>
    </div>
    <div v-if="row.goal">
      <div class="text-text-tertiary mb-0.5">目标</div>
      <pre class="whitespace-pre-wrap font-mono text-[11px] text-text-primary">{{ row.goal }}</pre>
    </div>
    <div v-if="row.analysis">
      <div class="text-text-tertiary mb-0.5">分析</div>
      <pre class="whitespace-pre-wrap font-mono text-[11px] text-text-primary max-h-64 overflow-y-auto">{{ row.analysis }}</pre>
    </div>
    <div>
      <span class="text-text-tertiary">选项 </span>
      <span class="font-mono">{{ optionsText() }}</span>
    </div>
    <div v-if="row.answer">
      <span class="text-text-tertiary">应答 </span>{{ row.answer }}
      <span v-if="row.answered_by" class="text-text-tertiary"> · by {{ row.answered_by.slice(0, 8) }}</span>
    </div>
    <div v-if="row.resume_ref">
      <div class="text-text-tertiary mb-0.5">resume_ref</div>
      <pre class="whitespace-pre-wrap font-mono text-[11px] break-all">{{ row.resume_ref }}</pre>
    </div>
    <div v-if="row.result">
      <div class="text-text-tertiary mb-0.5">结果</div>
      <pre class="whitespace-pre-wrap font-mono text-[11px]">{{ row.result }}</pre>
    </div>
    <div v-if="row.error" class="text-red-400">
      <div class="text-text-tertiary mb-0.5">错误</div>
      <pre class="whitespace-pre-wrap font-mono text-[11px]">{{ row.error }}</pre>
    </div>
    <div class="text-text-tertiary font-mono text-[10px]">id={{ row.id }}</div>
  </div>
</template>
