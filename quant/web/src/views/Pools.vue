<script setup lang="ts">
/**
 * 股票池组管理:列表 + 新建 + 成员增删(支持粘贴批量导入) + 删除。
 *
 * 预置池(kind='index'/'all')全局共享且只读,只能「另存为自定义池」。
 * 自定义池(kind='static')只存代码不存日期,故无成员历史 —— 页面需明示该取舍。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { AlertTriangle, ClipboardPaste, Lock, Plus, Trash2 } from 'lucide-vue-next'
import { api, isPresetPool, normalizeStockCode, type Pool, type PoolMember } from '../api'
import PageHeader from '../components/PageHeader.vue'
import InlineFeedback from '../components/InlineFeedback.vue'
import LoadingRows from '../components/LoadingRows.vue'
import { usePools } from '../pools'
import { useAsyncAction } from '../useAsyncAction'

const { pools, loading: poolsLoading, error: loadError, load: loadPools, invalidate } = usePools()

onMounted(() => {
  // 直接打开本页时共享缓存可能为空,必须主动拉取;失败原因已由 loadPools 写入 loadError
  void loadPools().catch(() => {})
})

const selectedId = ref<number | null>(null)
const members = ref<PoolMember[]>([])
const { busy, error, notice, clear, fail, run: runAction } = useAsyncAction()
const memberAction = useAsyncAction()
const membersLoading = memberAction.busy

const newPoolName = ref('')
const pasteText = ref('')
const minListDays = ref(60)

const selected = computed<Pool | null>(() => pools.value.find((pool) => pool.id === selectedId.value) ?? null)
const readonlyPool = computed(() => isPresetPool(selected.value))

/** 粘贴框里能识别出的合法代码,去重后预览 */
const parsedCodes = computed(() => {
  const codes = pasteText.value
    .split(/[\s,,;；、]+/)
    .map((raw) => normalizeStockCode(raw))
    .filter((code): code is string => !!code)
  return [...new Set(codes)]
})

const invalidCount = computed(() => {
  const tokens = pasteText.value.split(/[\s,,;；、]+/).filter(Boolean)
  return tokens.length - tokens.filter((raw) => normalizeStockCode(raw)).length
})

async function refreshPools(selectId?: number) {
  invalidate()
  const items = await loadPools(true)
  if (selectId !== undefined) selectedId.value = selectId
  else if (!items.some((pool) => pool.id === selectedId.value)) {
    selectedId.value = items[0]?.id ?? null
  }
}

async function loadMembers(id: number | null) {
  if (id === null) {
    members.value = []
    return
  }
  await memberAction.run(async () => {
    const response = await api.poolMembers(id)
    members.value = response.items ?? []
  })
  if (memberAction.error.value) {
    members.value = []
    fail(memberAction.error.value)
  }
}

watch(selectedId, (id) => {
  clear()
  pasteText.value = ''
  void loadMembers(id)
})

watch(pools, (items) => {
  if (selectedId.value === null && items.length) selectedId.value = items[0].id
}, { immediate: true })

async function createPool() {
  const name = newPoolName.value.trim()
  if (!name) {
    fail('请填写股票池名称')
    return
  }
  await runAction(async () => {
    const pool = await api.createPool({ name, min_list_days: minListDays.value })
    newPoolName.value = ''
    await refreshPools(pool.id)
    return pool
  }, { success: (pool) => `已创建「${pool.name}」，可在右侧粘贴代码导入成员。` })
}

/** 预置池另存为自定义池:用当前成分做初始成员快照 */
async function saveAsCustom() {
  const source = selected.value
  if (!source) return
  await runAction(async () => {
    const snapshot = await api.poolMembers(source.id)
    const pool = await api.createPool({
      name: `${source.name} 副本`,
      min_list_days: source.min_list_days,
      codes: (snapshot.items ?? []).map((member) => member.code),
    })
    await refreshPools(pool.id)
    return pool
  }, { success: (pool) => `已按当前成分另存为「${pool.name}」。该副本为静态名单，不含成员变动历史。` })
}

async function importCodes() {
  const pool = selected.value
  if (!pool || readonlyPool.value) return
  if (!parsedCodes.value.length) {
    fail('没有识别到合法的股票代码')
    return
  }
  await runAction(async () => {
    const result = await api.addPoolMembers(pool.id, parsedCodes.value)
    pasteText.value = ''
    await Promise.all([loadMembers(pool.id), refreshPools(pool.id)])
    return result
  }, {
    success: (result) => {
      const skipped = result.skipped?.length ? `，${result.skipped.length} 个代码未入库被忽略` : ''
      return `已导入 ${result.added} 只股票${skipped}。`
    },
  })
}

async function removeMember(code: string) {
  const pool = selected.value
  if (!pool || readonlyPool.value) return
  await runAction(async () => {
    await api.removePoolMember(pool.id, code)
    await Promise.all([loadMembers(pool.id), refreshPools(pool.id)])
  })
}

async function saveSettings() {
  const pool = selected.value
  if (!pool || readonlyPool.value) return
  await runAction(async () => {
    await api.updatePool(pool.id, { min_list_days: pool.min_list_days })
    await refreshPools(pool.id)
  }, { success: '已保存股票池设置。' })
}

async function deletePool() {
  const pool = selected.value
  if (!pool || readonlyPool.value) return
  if (!window.confirm(`确认删除股票池「${pool.name}」？该操作不可撤销。`)) return
  await runAction(async () => {
    await api.deletePool(pool.id)
    selectedId.value = null
    await refreshPools()
  }, { success: `已删除「${pool.name}」。` })
}
</script>

<template>
  <div class="space-y-5">
    <PageHeader
      title="股票池组"
      description="选股与回测的研究范围。预置池按指数成分变动历史逐日解析，自定义池只保存当前名单。"
    />

    <InlineFeedback v-if="error" tone="error">{{ error }}</InlineFeedback>
    <InlineFeedback v-if="notice">{{ notice }}</InlineFeedback>

    <div class="grid gap-5 lg:grid-cols-[18rem_1fr]">
      <!-- 池列表 + 新建 -->
      <section class="space-y-3" aria-labelledby="pool-list-heading">
        <h2 id="pool-list-heading" class="text-sm font-semibold">全部股票池</h2>
        <LoadingRows v-if="poolsLoading" :rows="3" />
        <ul v-else class="space-y-1.5">
          <li v-for="pool in pools" :key="pool.id">
            <button
              type="button"
              class="w-full rounded-md border px-3 py-2 text-left text-sm"
              :class="pool.id === selectedId
                ? 'border-accent bg-active text-text-primary'
                : 'border-border bg-surface-raised text-text-secondary hover:bg-hover'"
              @click="selectedId = pool.id"
            >
              <span class="flex items-center gap-1.5">
                <span class="min-w-0 flex-1 truncate font-medium">{{ pool.name }}</span>
                <Lock v-if="isPresetPool(pool)" :size="13" class="shrink-0 text-text-tertiary" aria-label="预置只读" />
              </span>
              <span class="mt-0.5 block text-xs text-text-tertiary">
                {{ pool.kind === 'static' ? '自定义静态名单' : pool.kind === 'all' ? '全部A股（动态解析）' : '指数成分（动态解析）' }}
                <template v-if="pool.kind === 'static' && pool.member_count != null"> · {{ pool.member_count }} 只</template>
              </span>
            </button>
          </li>
          <li v-if="loadError">
            <InlineFeedback tone="error">股票池加载失败：{{ loadError }}</InlineFeedback>
          </li>
          <li v-else-if="!pools.length" class="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-text-tertiary">
            暂无股票池
          </li>
        </ul>

        <form class="space-y-2 rounded-md border border-border bg-surface-raised p-3" @submit.prevent="createPool">
          <span class="block text-xs font-medium text-text-secondary">新建自定义池</span>
          <input
            v-model="newPoolName"
            placeholder="名称，如 我的观察池"
            class="w-full rounded-md border border-border px-2 py-1.5 text-sm"
          />
          <label class="block text-xs text-text-tertiary">
            新股上市满（交易日）
            <input v-model.number="minListDays" type="number" min="0" max="750" class="mt-1 w-full rounded-md border border-border px-2 py-1.5 text-sm" />
          </label>
          <button
            type="submit"
            :disabled="busy"
            class="inline-flex w-full items-center justify-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
          >
            <Plus :size="15" />
            新建
          </button>
        </form>
      </section>

      <!-- 选中池详情 -->
      <section v-if="selected" class="space-y-4" aria-labelledby="pool-detail-heading">
        <div class="flex flex-wrap items-end justify-between gap-3 border-b border-border-subtle pb-3">
          <div>
            <h2 id="pool-detail-heading" class="text-base font-semibold">{{ selected.name }}</h2>
            <p class="mt-0.5 text-xs text-text-tertiary">
              <template v-if="selected.kind === 'all'">按上市/退市日期与 ST 状态逐日解析全部A股，新股需上市满 {{ selected.min_list_days }} 个交易日。</template>
              <template v-else-if="selected.kind === 'index'">按指数成分生效与剔除日期逐日解析。</template>
              <template v-else>自定义静态名单，共 {{ members.length }} 只股票。</template>
            </p>
          </div>
          <div class="flex items-center gap-2">
            <button
              type="button"
              :disabled="busy"
              class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="saveAsCustom"
            >
              另存为自定义池
            </button>
            <button
              v-if="!readonlyPool"
              type="button"
              :disabled="busy"
              class="inline-flex items-center gap-1.5 rounded-md border border-up/40 px-3 py-1.5 text-sm text-up hover:bg-up/5 disabled:opacity-50"
              @click="deletePool"
            >
              <Trash2 :size="14" />
              删除池
            </button>
          </div>
        </div>

        <!-- 幸存者偏差说明:仅自定义静态池,预置池是 point-in-time 正确的 -->
        <p
          v-if="!readonlyPool"
          class="flex items-start gap-2 rounded-md border border-border bg-warning-soft px-4 py-3 text-sm leading-6 text-text-secondary"
        >
          <AlertTriangle :size="16" class="mt-0.5 shrink-0 text-warning" />
          <span>
            <strong class="font-medium text-text-primary">该池无成员历史。</strong>
            自定义池只保存当前股票名单、不记录每只股票何时加入或移出，因此用它回测历史区间等于「用今天的名单去跑过去」，
            已退市或曾被剔除的股票不会出现，结果偏乐观 —— 即<strong class="font-medium text-text-primary">幸存者偏差</strong>。
            需要严格的历史口径时请改用预置池（全部A股 / 指数成分），它们按逐日成分解析。
          </span>
        </p>

        <div v-if="readonlyPool" class="flex items-start gap-2 rounded-md border border-border bg-info-soft px-4 py-3 text-sm leading-6 text-text-secondary">
          <Lock :size="16" class="mt-0.5 shrink-0 text-text-tertiary" />
          <span>预置池由系统维护、全部用户共享，不可编辑成员。如需自行增删，请先「另存为自定义池」。</span>
        </div>

        <template v-if="!readonlyPool">
          <div class="flex flex-wrap items-end gap-3 rounded-md border border-border bg-surface-raised p-4">
            <label class="text-sm">
              <span class="mb-1 block text-xs text-text-tertiary">新股上市满（交易日）</span>
              <input v-model.number="selected.min_list_days" type="number" min="0" max="750" class="w-32 rounded-md border border-border px-2 py-1.5 text-sm" />
            </label>
            <button
              type="button"
              :disabled="busy"
              class="rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary hover:bg-hover disabled:opacity-50"
              @click="saveSettings"
            >
              保存设置
            </button>
          </div>

          <!-- 粘贴批量导入 -->
          <div class="space-y-2 rounded-md border border-border bg-surface-raised p-4">
            <div class="flex items-center gap-1.5">
              <ClipboardPaste :size="15" class="text-text-tertiary" />
              <h3 class="text-sm font-semibold">批量导入成员</h3>
            </div>
            <p class="text-xs text-text-tertiary">
              粘贴股票代码，支持换行、空格、逗号、分号、顿号分隔。六位代码会自动补交易所前缀。
            </p>
            <textarea
              v-model="pasteText"
              rows="4"
              placeholder="600519 000001&#10;sh.601318, sz.300750"
              class="w-full rounded-md border border-border px-2.5 py-2 font-mono text-sm"
            ></textarea>
            <div class="flex flex-wrap items-center gap-3">
              <button
                type="button"
                :disabled="busy || !parsedCodes.length"
                class="rounded-md bg-accent px-4 py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-50"
                @click="importCodes"
              >
                导入 {{ parsedCodes.length }} 只
              </button>
              <span v-if="invalidCount > 0" class="text-xs text-up">{{ invalidCount }} 项无法识别为股票代码，将被跳过。</span>
            </div>
          </div>
        </template>

        <!-- 成员列表 -->
        <div>
          <h3 class="mb-2 text-sm font-semibold">
            成员
            <span class="ml-1 font-normal text-text-tertiary">（{{ members.length }}）</span>
          </h3>
          <LoadingRows v-if="membersLoading" :rows="3" />
          <template v-else-if="members.length">
            <div class="flex flex-wrap gap-2">
              <span
                v-for="member in members"
                :key="member.code"
                class="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface-raised px-2.5 py-1 text-xs"
              >
                <router-link :to="`/stock/${member.code}`" class="hover:text-accent">
                  {{ member.name || member.code }}
                  <span class="ml-1 text-text-tertiary">{{ member.code }}</span>
                </router-link>
                <button
                  v-if="!readonlyPool"
                  type="button"
                  :disabled="busy"
                  class="text-text-tertiary hover:text-up disabled:opacity-40"
                  :aria-label="`从股票池移除 ${member.name || member.code}`"
                  @click="removeMember(member.code)"
                >
                  <Trash2 :size="12" />
                </button>
              </span>
            </div>
          </template>
          <p v-else class="rounded-md border border-dashed border-border px-4 py-8 text-center text-sm text-text-tertiary">
            <template v-if="readonlyPool">该池成员按交易日动态解析，不保存固定名单。</template>
            <template v-else>还没有成员，先在上方粘贴代码导入。</template>
          </p>
        </div>
      </section>

      <section v-else class="rounded-md border border-dashed border-border px-5 py-12 text-center text-sm text-text-tertiary">
        选择左侧股票池查看详情，或新建一个自定义池。
      </section>
    </div>
  </div>
</template>
