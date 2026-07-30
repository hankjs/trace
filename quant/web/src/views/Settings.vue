<script setup lang="ts">
/**
 * 账户偏好:与实盘权限相关的开关。
 * 不改动行情入库;默认关闭北交所,避免研究池混入自己买不了的票时误判。
 */
import { onMounted, ref } from 'vue'
import { ListChecks, Settings as SettingsIcon } from 'lucide-vue-next'
import { api, type UserSettings } from '../api'
import InlineFeedback from '../components/InlineFeedback.vue'
import { useAsyncAction } from '../useAsyncAction'
import { useOnboarding } from '../onboarding'

const loading = ref(true)
const canTradeBse = ref(false)
const updatedAt = ref<string | null>(null)
const { busy, error, notice, fail, run } = useAsyncAction()
const { dismissed, hideForever, showGuide } = useOnboarding()

function onToggleGuide(event: Event) {
  const input = event.target as HTMLInputElement
  // 仅保存在本浏览器(localStorage),不涉及服务端设置
  if (input.checked) showGuide()
  else hideForever()
}

async function load() {
  loading.value = true
  try {
    const settings: UserSettings = await api.getSettings()
    canTradeBse.value = settings.can_trade_bse
    updatedAt.value = settings.updated_at
  } catch (caught) {
    fail(caught instanceof Error ? caught.message : String(caught))
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void load()
})

async function onToggleBse(event: Event) {
  const input = event.target as HTMLInputElement
  const next = input.checked
  const previous = canTradeBse.value
  canTradeBse.value = next
  const result = await run(async () => {
    const settings = await api.patchSettings({ can_trade_bse: next })
    canTradeBse.value = settings.can_trade_bse
    updatedAt.value = settings.updated_at
    return settings
  }, {
    success: next
      ? '已标记：本账户可交易北交所。'
      : '已标记：本账户不可交易北交所。',
  })
  if (result === undefined) {
    canTradeBse.value = previous
  }
}
</script>

<template>
  <div class="space-y-4">
    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-else-if="notice">{{ notice }}</InlineFeedback>

    <section class="rounded border border-border bg-surface-raised">
      <div class="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <SettingsIcon :size="15" class="text-accent" />
        <h2 class="text-sm font-medium text-text-primary">交易权限标记</h2>
      </div>

      <div v-if="loading" class="px-3 py-6 text-xs text-text-tertiary">加载中…</div>

      <label
        v-else
        class="flex cursor-pointer items-start gap-3 px-3 py-3.5 transition-colors hover:bg-hover"
      >
        <input
          class="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
          type="checkbox"
          :checked="canTradeBse"
          :disabled="busy"
          @change="onToggleBse"
        />
        <span class="min-w-0">
          <span class="block text-sm font-medium text-text-primary">可交易北交所</span>
          <span class="mt-1 block text-xs leading-5 text-text-secondary">
            打开表示你已在券商开通北交所权限（个人通常需 50 万日均资产 + 2 年经验等）。
            关闭时表示当前账户只做沪深研究/记账意图；这只是账户标记，不会删除或停采库里的北交所行情。
          </span>
          <span v-if="updatedAt" class="mt-1.5 block text-[10px] text-text-tertiary">
            最近更新：{{ updatedAt }}
          </span>
        </span>
      </label>
    </section>

    <section class="rounded border border-border bg-surface-raised">
      <div class="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <ListChecks :size="15" class="text-accent" />
        <h2 class="text-sm font-medium text-text-primary">界面</h2>
      </div>

      <label class="flex cursor-pointer items-start gap-3 px-3 py-3.5 transition-colors hover:bg-hover">
        <input
          class="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
          type="checkbox"
          :checked="!dismissed"
          @change="onToggleGuide"
        />
        <span class="min-w-0">
          <span class="block text-sm font-medium text-text-primary">显示「新手上路」引导入口</span>
          <span class="mt-1 block text-xs leading-5 text-text-secondary">
            页面右下角的新手任务浮动按钮。在任务面板里点了「永远隐藏」后，从这里重新打开。
            该选择只保存在当前浏览器，进度不会受影响。
          </span>
        </span>
      </label>
    </section>
  </div>
</template>
