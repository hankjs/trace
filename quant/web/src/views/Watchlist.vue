<script setup lang="ts">
/**
 * 自选股管理:列表 + 搜索添加 + 删除。
 *
 * 自选关系按用户隔离(quant_watchlist),行情总览的「自选行情」展示
 * 和盘中快照的采集范围都以自选名单为准;股票资料本身全系统共享。
 */
import { onMounted, ref } from 'vue'
import { Star, Trash2 } from 'lucide-vue-next'
import { api, type WatchItem } from '../api'
import PageHeader from '../components/PageHeader.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import StockSearchInput from '../components/StockSearchInput.vue'
import { useAsyncAction } from '../useAsyncAction'

const items = ref<WatchItem[]>([])
const loading = ref(true)
const { busy, error, notice, fail, run } = useAsyncAction()
const stockCode = ref('')
const removingCode = ref('')

async function load() {
  loading.value = true
  try {
    const response = await api.watchlist()
    items.value = response.items ?? []
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void load()
})

async function add() {
  const code = stockCode.value
  if (!code) {
    fail('请先搜索并选择一只股票')
    return
  }
  await run(async () => {
    const item = await api.addWatch(code)
    stockCode.value = ''
    await load()
    return item
  }, { success: (item) => `已把 ${item.name || item.code} 加入自选。` })
}

async function remove(item: WatchItem) {
  removingCode.value = item.code
  try {
    await run(async () => {
      await api.removeWatch(item.code)
      items.value = items.value.filter((w) => w.code !== item.code)
    }, { success: `已把 ${item.name || item.code} 移出自选。` })
  } finally {
    removingCode.value = ''
  }
}
</script>

<template>
  <div class="space-y-4">
    <PageHeader
      eyebrow="行情研究"
      title="自选股"
      description="自选股票决定行情总览「自选行情」的展示范围，盘中快照也按自选名单采集。股票资料全系统共享，自选关系仅自己可见。"
    />

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-else-if="notice">{{ notice }}</InlineFeedback>

    <section class="terminal-panel" aria-labelledby="watchlist-add-heading">
      <div class="terminal-panel-header">
        <h2 id="watchlist-add-heading" class="text-sm font-semibold">添加自选</h2>
        <span class="text-[11px] text-text-tertiary">按名称或代码搜索</span>
      </div>
      <form class="flex flex-wrap items-end gap-2 px-3 py-3" @submit.prevent="add">
        <StockSearchInput
          v-model="stockCode"
          label="股票"
          placeholder="输入中文名称或代码"
          input-class="!w-64"
        />
        <button
          type="submit"
          :disabled="busy || !stockCode"
          class="inline-flex items-center justify-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
        >
          <Star :size="15" />
          加入自选
        </button>
      </form>
    </section>

    <section class="terminal-panel" aria-labelledby="watchlist-heading">
      <div class="terminal-panel-header">
        <div class="flex items-baseline gap-2">
          <h2 id="watchlist-heading" class="text-sm font-semibold">我的自选</h2>
          <span class="text-[11px] text-text-tertiary">{{ items.length }} 只</span>
        </div>
        <router-link to="/" class="text-[11px] text-accent hover:underline">查看自选行情</router-link>
      </div>

      <LoadingRows v-if="loading" :rows="4" />

      <div v-else-if="items.length" class="overflow-auto">
        <table class="terminal-table min-w-[480px]">
          <thead>
            <tr class="text-left">
              <th>名称 / 代码</th>
              <th>行业</th>
              <th class="w-20 text-right">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in items" :key="item.code">
              <td>
                <router-link :to="`/stock/${item.code}`" class="font-medium hover:text-accent">{{ item.name || '名称待同步' }}</router-link>
                <span class="ml-2 text-[11px] text-text-tertiary">{{ item.code }}</span>
              </td>
              <td class="text-xs text-text-tertiary">{{ item.industry || '--' }}</td>
              <td class="text-right">
                <button
                  type="button"
                  class="icon-button"
                  :disabled="busy && removingCode === item.code"
                  :title="`移出自选 ${item.name || item.code}`"
                  @click="remove(item)"
                >
                  <Trash2 :size="15" />
                  <span class="sr-only">移出自选</span>
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-else class="px-4 py-12 text-center">
        <p class="text-sm text-text-secondary">还没有自选股票</p>
        <p class="mt-1 text-xs text-text-tertiary">用上方搜索框添加后，行情总览会展示这些股票的最新行情。</p>
      </div>
    </section>
  </div>
</template>
