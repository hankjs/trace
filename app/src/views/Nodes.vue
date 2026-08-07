<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api, type ClientInfo } from '../api'
import NeuButton from '../components/ui/NeuButton.vue'

const router = useRouter()
const clients = ref<ClientInfo[]>([])
const error = ref('')
const loading = ref(true)
const showAdd = ref(false)

async function load() {
  loading.value = true
  error.value = ''
  try {
    clients.value = (await api.clients()).clients
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载失败'
  } finally {
    loading.value = false
  }
}

async function toggle(c: ClientInfo) {
  try {
    await api.setEnabled(c.id, !c.enabled)
    c.enabled = !c.enabled
  } catch (err) {
    error.value = err instanceof Error ? err.message : '更新失败'
  }
}

async function remove(c: ClientInfo) {
  const name = c.hostname || c.id.slice(0, 12)
  if (!confirm(`删除节点「${name}」？\n桌面端若仍开启远程，下次注册会重新出现。`)) return
  try {
    await api.deleteClient(c.id)
    clients.value = clients.value.filter((x) => x.id !== c.id)
  } catch (err) {
    error.value = err instanceof Error ? err.message : '删除失败'
  }
}

onMounted(load)
</script>

<template>
  <div>
    <div class="flex items-center justify-between gap-3">
      <div>
        <h1 class="text-xl font-medium text-ink">节点</h1>
        <p class="mt-1 text-sm text-ink-2">
          节点是本机上的 Trace 客户端。开启「允许远程终端」后会出现在这里。
        </p>
      </div>
      <div class="flex shrink-0 gap-2">
        <NeuButton :disabled="loading" @click="load">刷新</NeuButton>
        <NeuButton variant="primary" @click="showAdd = !showAdd">
          {{ showAdd ? '收起' : '添加' }}
        </NeuButton>
      </div>
    </div>

    <p v-if="error" class="mt-3 text-sm text-danger">{{ error }}</p>

    <!-- 添加节点：桌面端自注册，无配对码 -->
    <div v-if="showAdd || (!loading && !clients.length)" class="mt-5 neu-card p-4">
      <h2 class="text-sm font-medium text-ink">添加节点</h2>
      <p class="mt-1 text-xs text-ink-2">
        Trace 客户端出站注册，无需配对码、不用给电脑开端口。
      </p>
      <ol class="mt-3 space-y-3 text-sm text-ink-2">
        <li class="flex gap-2">
          <span class="shrink-0 font-medium text-ink-3">1.</span>
          <span>在本机打开 Trace 桌面客户端，用<strong class="font-medium text-ink">同一账号</strong>登录。</span>
        </li>
        <li class="flex gap-2">
          <span class="shrink-0 font-medium text-ink-3">2.</span>
          <span>设置 → 远程终端 → 打开「允许远程终端控制」。</span>
        </li>
        <li class="flex gap-2">
          <span class="shrink-0 font-medium text-ink-3">3.</span>
          <span>回到本页点「刷新」；绿点在线后即可进入操作终端。</span>
        </li>
      </ol>
      <div class="mt-4 flex justify-end">
        <NeuButton variant="primary" :disabled="loading" @click="load">
          刷新列表
        </NeuButton>
      </div>
    </div>

    <p v-if="loading" class="mt-6 text-sm text-ink-2">加载中…</p>

    <ul
      v-else-if="clients.length"
      class="mt-5 divide-y divide-(--shadow-lo) neu-card"
    >
      <li
        v-for="c in clients"
        :key="c.id"
        class="flex cursor-pointer items-center gap-3 px-4 py-3 hover:bg-raise"
        @click="router.push(`/nodes/${c.id}`)"
      >
        <span class="status-dot shrink-0" :class="c.online ? 'on' : 'off'" />
        <div class="min-w-0 flex-1">
          <div class="text-sm text-ink">
            {{ c.hostname || c.id.slice(0, 12) }}
            <span v-if="!c.enabled" class="text-xs text-ink-3">（已停用）</span>
            <span v-if="!c.accept_remote" class="text-xs text-warn"> · 未开远程</span>
          </div>
          <div class="mt-0.5 truncate text-xs text-ink-2">
            {{ c.work_dir || '未上报工作目录' }}
          </div>
        </div>
        <NeuButton variant="text" class="shrink-0" @click.stop="toggle(c)">
          {{ c.enabled ? '停用' : '启用' }}
        </NeuButton>
        <NeuButton variant="text-danger" class="shrink-0" @click.stop="remove(c)">
          删除
        </NeuButton>
      </li>
    </ul>
  </div>
</template>
