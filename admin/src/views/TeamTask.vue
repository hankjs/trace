<script setup lang="ts">
/**
 * 团队任务运行时配置：两阶段闸门、多角色流水线、角色顺序与闸门边界。
 *
 * 配置存在服务端 settings 表（DB 优先、config.toml 兜底），改完即时生效、
 * 不需要重启 hank-server。下一个飞书任务就会按新配置走。
 *
 * 不做轮询——这是配置页，没有需要追踪的运行态（Jobs 轮询是因为有执行中的任务）。
 * 选项由后端 role_options / gate_options 返回，不在前端硬编码——加角色时只改一处。
 */
import { computed, onMounted, ref } from 'vue'
import {
  api,
  type TeamTaskConfig,
  type TeamTaskOption,
} from '../composables/api'

const loading = ref(true)
const saving = ref(false)
const error = ref('')
const notice = ref('')

const source = ref<'db' | 'config_file'>('config_file')
const roleOptions = ref<TeamTaskOption[]>([])
const gateOptions = ref<TeamTaskOption[]>([])

/** 可编辑草稿 */
const form = ref<TeamTaskConfig>({
  task_gate_enabled: false,
  enabled: false,
  roles: [],
  gates: [],
  max_dev_rounds: 3,
  dashboard_base_url: null,
  updated_by: null,
})

/** 流水线依赖闸门：闸门关时流水线不可开 */
const pipelineDisabled = computed(() => !form.value.task_gate_enabled)

/** 至少勾一个角色才允许保存 */
const noRolesSelected = computed(() => form.value.roles.length === 0)

const canSave = computed(() => {
  if (saving.value) return false
  if (noRolesSelected.value) return false
  if (form.value.max_dev_rounds < 1 || form.value.max_dev_rounds > 10) return false
  return true
})

function applyResponse(res: {
  config: TeamTaskConfig
  source: 'db' | 'config_file'
  role_options: TeamTaskOption[]
  gate_options: TeamTaskOption[]
}) {
  form.value = {
    ...res.config,
    // 深拷贝数组，避免直接改响应引用
    roles: [...res.config.roles],
    gates: [...res.config.gates],
  }
  source.value = res.source
  roleOptions.value = res.role_options
  gateOptions.value = res.gate_options
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    const res = await api.getTeamTaskConfig()
    applyResponse(res)
  } catch (e: any) {
    error.value = e?.message || '加载失败'
  } finally {
    loading.value = false
  }
}

function setTaskGate(on: boolean) {
  form.value.task_gate_enabled = on
  // 关闸门时强制关流水线，避免提交后吃 400
  if (!on) {
    form.value.enabled = false
  }
}

function setPipeline(on: boolean) {
  if (!form.value.task_gate_enabled) return
  form.value.enabled = on
}

function isRoleEnabled(id: string): boolean {
  return form.value.roles.includes(id)
}

/** 勾选/取消角色：取消时从顺序里删；勾选时追加到末尾 */
function toggleRole(id: string) {
  if (isRoleEnabled(id)) {
    form.value.roles = form.value.roles.filter((r) => r !== id)
  } else {
    form.value.roles = [...form.value.roles, id]
  }
}

function roleLabel(id: string): string {
  return roleOptions.value.find((o) => o.id === id)?.label ?? id
}

function moveRole(index: number, delta: number) {
  const next = index + delta
  if (next < 0 || next >= form.value.roles.length) return
  const roles = [...form.value.roles]
  const tmp = roles[index]
  roles[index] = roles[next]
  roles[next] = tmp
  form.value.roles = roles
}

function isGateEnabled(id: string): boolean {
  return form.value.gates.includes(id)
}

function toggleGate(id: string) {
  if (isGateEnabled(id)) {
    form.value.gates = form.value.gates.filter((g) => g !== id)
  } else {
    form.value.gates = [...form.value.gates, id]
  }
}

async function save() {
  if (!canSave.value) return
  saving.value = true
  error.value = ''
  notice.value = ''
  try {
    // 整份草稿提交；后端 PATCH 语义支持 Partial，这里传全量更直观
    const res = await api.updateTeamTaskConfig({
      task_gate_enabled: form.value.task_gate_enabled,
      enabled: form.value.enabled,
      roles: form.value.roles,
      gates: form.value.gates,
      max_dev_rounds: form.value.max_dev_rounds,
      dashboard_base_url: form.value.dashboard_base_url?.trim()
        ? form.value.dashboard_base_url.trim()
        : null,
    })
    // 用响应刷新，后端若有归一前端能看到真实值
    applyResponse(res)
    notice.value = '已保存，即时生效（无需重启服务）'
  } catch (e: any) {
    // 直接展示后端 validate 原文，不另编文案（避免两套漂移）
    error.value = e?.message || '保存失败'
  } finally {
    saving.value = false
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-lg font-semibold text-text-primary">团队任务</h1>
    </div>

    <p v-if="error" class="mb-4 text-xs text-red-400">{{ error }}</p>
    <p v-else-if="notice" class="mb-4 text-xs text-green-500">{{ notice }}</p>

    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>

    <template v-else>
      <!-- 配置来源提示 -->
      <div
        v-if="source === 'config_file'"
        class="mb-5 rounded-lg border border-yellow-500/30 bg-yellow-500/5 px-4 py-3 text-xs leading-5 text-yellow-600"
      >
        当前使用配置文件中的默认值，尚未在此页面保存过。保存后将以此处配置为准。
      </div>
      <div
        v-else
        class="mb-5 rounded-lg border border-border-subtle px-4 py-3 text-xs leading-5 text-text-secondary"
      >
        当前使用数据库中的运行时配置
        <span v-if="form.updated_by" class="text-text-tertiary">
          · 最后修改人：{{ form.updated_by }}
        </span>
      </div>

      <!-- ① 总开关 -->
      <section class="mb-8">
        <h2 class="mb-3 text-sm font-medium text-text-primary">总开关</h2>
        <div class="space-y-4 rounded-lg border border-border-subtle p-4">
          <!-- 两阶段闸门 -->
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0 flex-1">
              <div class="text-[13px] font-medium text-text-primary">两阶段闸门</div>
              <p class="mt-1 text-xs leading-5 text-text-tertiary">
                开启后飞书代码任务先只读分析，产出目标 / 范围 / 疑似改动点 / 风险，
                等你点「开始修」才真正改代码。
              </p>
            </div>
            <button
              type="button"
              class="mt-0.5 flex shrink-0 items-center gap-2"
              @click="setTaskGate(!form.task_gate_enabled)"
            >
              <span
                class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors"
                :class="form.task_gate_enabled ? 'bg-green-500' : 'bg-border-subtle'"
              >
                <span
                  class="inline-block h-3 w-3 rounded-full bg-white transition-transform"
                  :class="form.task_gate_enabled ? 'translate-x-3.5' : 'translate-x-0.5'"
                />
              </span>
            </button>
          </div>

          <div class="border-t border-border-subtle" />

          <!-- 多角色流水线 -->
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0 flex-1">
              <div class="text-[13px] font-medium text-text-primary">多角色流水线</div>
              <p class="mt-1 text-xs leading-5 text-text-tertiary">
                开启后「开始修」之后按开发 → 评审 → 测试串行执行（顺序可在下方调整）。
              </p>
              <p
                v-if="pipelineDisabled"
                class="mt-1 text-xs text-yellow-600"
              >
                需要先开启两阶段闸门（分析轮是流水线入口）。
              </p>
            </div>
            <button
              type="button"
              class="mt-0.5 flex shrink-0 items-center gap-2 disabled:opacity-40"
              :disabled="pipelineDisabled"
              @click="setPipeline(!form.enabled)"
            >
              <span
                class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors"
                :class="form.enabled ? 'bg-green-500' : 'bg-border-subtle'"
              >
                <span
                  class="inline-block h-3 w-3 rounded-full bg-white transition-transform"
                  :class="form.enabled ? 'translate-x-3.5' : 'translate-x-0.5'"
                />
              </span>
            </button>
          </div>
        </div>
      </section>

      <!-- ② 角色顺序 -->
      <section class="mb-8">
        <h2 class="mb-1 text-sm font-medium text-text-primary">角色顺序</h2>
        <p class="mb-3 text-xs leading-5 text-text-tertiary">
          流水线按列表顺序流转。勾选启用，用上移 / 下移调整顺序。至少保留一个角色。
        </p>

        <!-- 当前顺序 -->
        <div
          v-if="form.roles.length > 0"
          class="mb-3 rounded-lg border border-border-subtle"
        >
          <div
            v-for="(id, idx) in form.roles"
            :key="id"
            class="flex items-center gap-3 border-b border-border-subtle px-3 py-2 last:border-b-0"
          >
            <span class="w-5 text-center font-mono text-[11px] text-text-tertiary">{{ idx + 1 }}</span>
            <span class="flex-1 text-[13px] text-text-primary">{{ roleLabel(id) }}</span>
            <span class="font-mono text-[10px] text-text-tertiary">{{ id }}</span>
            <button
              type="button"
              class="px-1.5 text-xs text-text-secondary hover:text-text-primary disabled:opacity-30"
              :disabled="idx === 0"
              title="上移"
              @click="moveRole(idx, -1)"
            >↑</button>
            <button
              type="button"
              class="px-1.5 text-xs text-text-secondary hover:text-text-primary disabled:opacity-30"
              :disabled="idx === form.roles.length - 1"
              title="下移"
              @click="moveRole(idx, 1)"
            >↓</button>
          </div>
        </div>
        <p v-else class="mb-3 text-xs text-red-400">尚未勾选任何角色，无法保存。</p>

        <!-- 勾选可用角色（选项由后端返回，不硬编码） -->
        <div class="flex flex-wrap gap-3">
          <label
            v-for="opt in roleOptions"
            :key="opt.id"
            class="flex cursor-pointer items-center gap-2 text-[13px] text-text-secondary"
          >
            <input
              type="checkbox"
              class="rounded border-border"
              :checked="isRoleEnabled(opt.id)"
              @change="toggleRole(opt.id)"
            >
            {{ opt.label }}
            <span class="font-mono text-[10px] text-text-tertiary">{{ opt.id }}</span>
          </label>
        </div>
      </section>

      <!-- ③ 闸门边界 -->
      <section class="mb-8">
        <h2 class="mb-1 text-sm font-medium text-text-primary">闸门边界</h2>
        <p class="mb-3 text-xs leading-5 text-text-tertiary">
          勾得越多，一个任务需要人工点的次数越多；全不勾即全自动流转（仍受「两阶段闸门」控制分析轮）。
        </p>
        <div class="space-y-2 rounded-lg border border-border-subtle p-4">
          <label
            v-for="opt in gateOptions"
            :key="opt.id"
            class="flex cursor-pointer items-start gap-2.5 text-[13px] text-text-secondary"
          >
            <input
              type="checkbox"
              class="mt-0.5 rounded border-border"
              :checked="isGateEnabled(opt.id)"
              @change="toggleGate(opt.id)"
            >
            <span>
              <span class="text-text-primary">{{ opt.label }}</span>
              <span class="ml-2 font-mono text-[10px] text-text-tertiary">{{ opt.id }}</span>
            </span>
          </label>
        </div>
      </section>

      <!-- ④ 参数 -->
      <section class="mb-8">
        <h2 class="mb-3 text-sm font-medium text-text-primary">参数</h2>
        <div class="space-y-4 rounded-lg border border-border-subtle p-4">
          <div>
            <label class="mb-1 block text-[13px] text-text-primary">最大返工轮次</label>
            <p class="mb-2 text-xs text-text-tertiary">
              评审打回后最多重新开发几轮，超出即失败（1–10）。
            </p>
            <input
              v-model.number="form.max_dev_rounds"
              type="number"
              min="1"
              max="10"
              class="w-24 rounded-md border border-border bg-transparent px-2.5 py-1.5 text-[13px] text-text-primary focus:border-accent focus:outline-none"
            >
            <p
              v-if="form.max_dev_rounds < 1 || form.max_dev_rounds > 10"
              class="mt-1 text-xs text-red-400"
            >
              请输入 1 到 10 之间的整数
            </p>
          </div>

          <div class="border-t border-border-subtle" />

          <div>
            <label class="mb-1 block text-[13px] text-text-primary">看板地址</label>
            <p class="mb-2 text-xs text-text-tertiary">
              飞书卡片深链用。留空则卡片不显示看板链接。
            </p>
            <input
              v-model="form.dashboard_base_url"
              type="text"
              placeholder="http://127.0.0.1:18789"
              class="w-full max-w-md rounded-md border border-border bg-transparent px-2.5 py-1.5 text-[13px] text-text-primary placeholder:text-text-tertiary focus:border-accent focus:outline-none"
            >
          </div>
        </div>
      </section>

      <!-- ⑤ 保存 -->
      <div class="flex items-center gap-3">
        <button
          type="button"
          class="rounded-md bg-text-primary px-3.5 py-1.5 text-[13px] text-surface-raised transition-opacity hover:opacity-80 disabled:opacity-40"
          :disabled="!canSave"
          @click="save"
        >
          {{ saving ? '保存中…' : '保存' }}
        </button>
        <span v-if="noRolesSelected" class="text-xs text-text-tertiary">
          请至少勾选一个角色
        </span>
      </div>
    </template>
  </div>
</template>
