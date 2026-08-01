<script setup lang="ts">
import { ref, onMounted } from 'vue'
import {
  api,
  type AgentCliBackend,
  type AgentCliProfile,
  type AgentCliTestResult,
} from '../composables/api'

/** 编辑中的配置草稿。api_key 留空表示不修改已存凭据。 */
interface Draft {
  name: string
  auth_kind: string
  api_key: string
  base_url: string
  model: string
  extra_env: Record<string, string>
}

const backends = ref<AgentCliBackend[]>([])
const loading = ref(true)
/** 正在编辑的配置 id；'new:<backend>' 表示新建 */
const editing = ref<string | null>(null)
const draft = ref<Draft | null>(null)
const busy = ref<string | null>(null)
const results = ref<Record<string, AgentCliTestResult>>({})
const errors = ref<Record<string, string>>({})
const notice = ref<Record<string, string>>({})

const BACKEND_TITLES: Record<string, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
}

const SOURCE_LABELS: Record<string, string> = {
  db: '本页启用的配置',
  env: '服务器 agent-cli.env',
  provider: '复用供应商记录',
}

function sourceLabel(source: string | null): string {
  return source ? SOURCE_LABELS[source] || source : '无可用凭据'
}

function sourceTone(source: string | null): string {
  if (source === 'db') return 'text-accent'
  if (!source) return 'text-red-500'
  return 'text-text-secondary'
}

function emptyDraft(backend: AgentCliBackend): Draft {
  const extra: Record<string, string> = {}
  for (const key of backend.extra_env_keys) extra[key] = ''
  return {
    name: '',
    auth_kind: backend.auth_kind_options[0] || '',
    api_key: '',
    base_url: '',
    model: '',
    extra_env: extra,
  }
}

function toDraft(profile: AgentCliProfile, backend: AgentCliBackend): Draft {
  // 按白名单补全空键，让每个可配项都有输入框。
  const extra: Record<string, string> = {}
  for (const key of backend.extra_env_keys) {
    extra[key] = profile.extra_env[key] ?? ''
  }
  return {
    name: profile.name,
    auth_kind: profile.auth_kind || backend.auth_kind_options[0] || '',
    api_key: '',
    base_url: profile.base_url,
    model: profile.model,
    extra_env: extra,
  }
}

async function load() {
  loading.value = true
  try {
    backends.value = await api.listAgentCliConfigs()
  } catch (e) {
    errors.value = { _load: e instanceof Error ? e.message : '加载失败' }
  }
  loading.value = false
}

function startCreate(backend: AgentCliBackend) {
  editing.value = `new:${backend.backend}`
  draft.value = emptyDraft(backend)
  delete errors.value._form
}

function startEdit(profile: AgentCliProfile, backend: AgentCliBackend) {
  editing.value = profile.id
  draft.value = toDraft(profile, backend)
  delete errors.value._form
}

function cancelEdit() {
  editing.value = null
  draft.value = null
  delete errors.value._form
}

/** 只提交非空的附加变量，空值表示不设置该变量。 */
function normalizedExtra(current: Draft): Record<string, string> {
  const extra: Record<string, string> = {}
  for (const [key, value] of Object.entries(current.extra_env)) {
    if (value.trim()) extra[key] = value.trim()
  }
  return extra
}

async function save(backend: AgentCliBackend) {
  const current = draft.value
  if (!current || !editing.value) return
  busy.value = 'save'
  delete errors.value._form
  try {
    const payload = {
      name: current.name,
      auth_kind: current.auth_kind,
      api_key: current.api_key,
      base_url: current.base_url,
      model: current.model,
      extra_env: normalizedExtra(current),
    }
    if (editing.value.startsWith('new:')) {
      await api.createAgentCliProfile(backend.backend, payload)
      notice.value[backend.backend] = '已新建，点「启用」切换过去'
    } else {
      await api.updateAgentCliProfile(editing.value, payload)
      notice.value[backend.backend] = '已保存'
    }
    cancelEdit()
    await load()
  } catch (e) {
    errors.value._form = e instanceof Error ? e.message : '保存失败'
  }
  busy.value = null
}

async function activate(profile: AgentCliProfile) {
  busy.value = profile.id
  delete errors.value[profile.backend]
  try {
    await api.activateAgentCliProfile(profile.id)
    notice.value[profile.backend] = `已切换到「${profile.name}」，下一轮任务生效`
    await load()
  } catch (e) {
    errors.value[profile.backend] = e instanceof Error ? e.message : '切换失败'
  }
  busy.value = null
}

async function deactivate(backend: string) {
  if (!confirm('停用后回退到服务器上的 agent-cli.env，确定？')) return
  busy.value = `off:${backend}`
  delete errors.value[backend]
  try {
    await api.deactivateAgentCliProfiles(backend)
    notice.value[backend] = '已停用，回退到服务器环境文件'
    await load()
  } catch (e) {
    errors.value[backend] = e instanceof Error ? e.message : '停用失败'
  }
  busy.value = null
}

async function test(profile: AgentCliProfile) {
  busy.value = `test:${profile.id}`
  delete results.value[profile.id]
  delete errors.value[profile.backend]
  try {
    results.value[profile.id] = await api.testAgentCliProfile(profile.id)
  } catch (e) {
    errors.value[profile.backend] = e instanceof Error ? e.message : '测试失败'
  }
  busy.value = null
}

/** 停用只是不再使用，凭据仍留在库里；轮换掉泄露的 key 需要真正删除。 */
async function remove(profile: AgentCliProfile) {
  const extra = profile.is_active ? '这是当前启用的配置，删除后回退到服务器环境文件。' : ''
  if (!confirm(`删除配置「${profile.name}」及其中的凭据？${extra}`)) return
  busy.value = profile.id
  delete errors.value[profile.backend]
  try {
    await api.deleteAgentCliProfile(profile.id)
    notice.value[profile.backend] = '已删除'
    await load()
  } catch (e) {
    errors.value[profile.backend] = e instanceof Error ? e.message : '删除失败'
  }
  busy.value = null
}

/** 从测试结果里点选模型：直接落库保存，省去再进编辑态一次。 */
async function pickModel(profile: AgentCliProfile, model: string) {
  busy.value = profile.id
  delete errors.value[profile.backend]
  try {
    await api.updateAgentCliProfile(profile.id, {
      name: profile.name,
      auth_kind: profile.auth_kind,
      api_key: '',
      base_url: profile.base_url,
      model,
      extra_env: profile.extra_env,
    })
    notice.value[profile.backend] = `已设为 ${model}`
    delete results.value[profile.id]
    await load()
  } catch (e) {
    errors.value[profile.backend] = e instanceof Error ? e.message : '设置失败'
  }
  busy.value = null
}

onMounted(load)
</script>

<template>
  <div>
    <div class="mb-6">
      <h1 class="text-lg font-semibold text-text-primary">Agent CLI 凭据</h1>
      <p class="mt-1 text-xs text-text-tertiary">
        飞书任务里 Codex / Claude Code 使用的第三方 API 配置。每个后端可存多份，同时启用一份，
        切换后下一轮任务即生效，无需重启服务。全部停用则回退到服务器上的 agent-cli.env。
      </p>
    </div>

    <div v-if="errors._load" class="mb-4 text-sm text-red-500">{{ errors._load }}</div>
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>

    <div v-else class="space-y-6">
      <div
        v-for="backend in backends"
        :key="backend.backend"
        class="border border-border-subtle rounded-lg overflow-hidden"
      >
        <div class="flex items-center justify-between px-4 py-3 border-b border-border-subtle">
          <div>
            <h2 class="text-sm font-medium text-text-primary">
              {{ BACKEND_TITLES[backend.backend] || backend.backend }}
            </h2>
            <p class="mt-0.5 text-[11px]" :class="sourceTone(backend.effective_source)">
              当前生效：{{ sourceLabel(backend.effective_source) }}
            </p>
          </div>
          <div class="flex items-center gap-2">
            <button
              v-if="backend.effective_source === 'db'"
              @click="deactivate(backend.backend)"
              :disabled="busy === `off:${backend.backend}`"
              class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-text-secondary hover:bg-hover disabled:opacity-40"
              title="全部停用，回退到服务器上的 agent-cli.env"
            >
              停用
            </button>
            <button
              @click="startCreate(backend)"
              class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90"
            >
              添加配置
            </button>
          </div>
        </div>

        <div v-if="notice[backend.backend]" class="px-4 pt-2 text-xs text-accent">
          {{ notice[backend.backend] }}
        </div>
        <div v-if="errors[backend.backend]" class="px-4 pt-2 text-xs text-red-500">
          {{ errors[backend.backend] }}
        </div>

        <p
          v-if="!backend.profiles.length && editing !== `new:${backend.backend}`"
          class="px-4 py-3 text-xs text-text-tertiary"
        >
          还没有配置，当前使用服务器上的 agent-cli.env。
        </p>

        <ul v-else class="divide-y divide-border-subtle">
          <li v-for="profile in backend.profiles" :key="profile.id" class="px-4 py-3">
            <div v-if="editing !== profile.id" class="flex items-center justify-between gap-3">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span
                    class="text-sm truncate"
                    :class="profile.is_active ? 'text-accent font-medium' : 'text-text-primary'"
                  >{{ profile.name }}</span>
                  <span
                    v-if="profile.is_active"
                    class="px-1.5 py-0.5 text-[10px] rounded bg-accent/10 text-accent"
                  >启用中</span>
                  <span v-if="!profile.api_key_set" class="text-[10px] text-red-500">缺凭据</span>
                </div>
                <p class="mt-0.5 text-[11px] text-text-tertiary truncate">
                  {{ profile.base_url || '官方端点' }}
                  <span v-if="profile.model"> · {{ profile.model }}</span>
                  <span class="font-mono"> · {{ profile.auth_kind }}</span>
                </p>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <button
                  v-if="!profile.is_active"
                  @click="activate(profile)"
                  :disabled="busy === profile.id"
                  class="px-2 py-1 text-xs bg-accent text-white rounded hover:opacity-90 disabled:opacity-40"
                >
                  启用
                </button>
                <button
                  @click="test(profile)"
                  :disabled="busy === `test:${profile.id}` || !profile.api_key_set"
                  class="px-2 py-1 text-xs border border-border-subtle rounded text-text-secondary hover:bg-hover disabled:opacity-40"
                  title="向该端点发一次最小请求"
                >
                  {{ busy === `test:${profile.id}` ? '测试中' : '测试' }}
                </button>
                <button
                  @click="startEdit(profile, backend)"
                  class="px-2 py-1 text-xs border border-border-subtle rounded text-text-secondary hover:bg-hover"
                >
                  编辑
                </button>
                <button
                  @click="remove(profile)"
                  :disabled="busy === profile.id"
                  class="px-2 py-1 text-xs border border-border-subtle rounded text-red-500 hover:bg-hover disabled:opacity-40"
                >
                  删除
                </button>
              </div>
            </div>

            <div
              v-if="results[profile.id]"
              class="mt-2 p-2 text-xs rounded border"
              :class="results[profile.id].ok
                ? 'border-border-subtle text-text-secondary'
                : results[profile.id].model_rejected
                  ? 'border-amber-500/40 text-amber-600'
                  : 'border-red-500/40 text-red-500'"
            >
              <div>
                {{ results[profile.id].ok ? '✓' : results[profile.id].model_rejected ? '!' : '✗' }}
                {{ results[profile.id].message }}
              </div>
              <div v-if="results[profile.id].models?.length" class="mt-2">
                <div class="text-[11px] text-text-tertiary mb-1">
                  端点支持的模型（点击设为该配置的模型）<span
                    v-if="(results[profile.id].models_total || 0) > (results[profile.id].models?.length || 0)"
                  >，共 {{ results[profile.id].models_total }} 个</span>
                </div>
                <div class="flex flex-wrap gap-1">
                  <button
                    v-for="model in results[profile.id].models"
                    :key="model"
                    @click="pickModel(profile, model)"
                    :disabled="busy === profile.id"
                    class="px-1.5 py-0.5 text-[11px] font-mono border border-border-subtle rounded text-text-secondary hover:bg-hover disabled:opacity-40"
                  >
                    {{ model }}
                  </button>
                </div>
              </div>
              <pre
                v-if="results[profile.id].detail"
                class="mt-1 whitespace-pre-wrap break-all font-mono text-[11px] text-text-tertiary"
              >{{ results[profile.id].detail }}</pre>
            </div>

            <div v-if="editing === profile.id && draft" class="mt-1">
              <div class="grid grid-cols-2 gap-3">
                <div>
                  <label class="block text-xs text-text-secondary mb-1">配置名</label>
                  <input
                    v-model="draft.name"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  />
                </div>
                <div v-if="backend.auth_kind_options.length > 1">
                  <label class="block text-xs text-text-secondary mb-1">凭据类型</label>
                  <select
                    v-model="draft.auth_kind"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  >
                    <option v-for="option in backend.auth_kind_options" :key="option" :value="option">
                      {{ option }}
                    </option>
                  </select>
                </div>
                <div class="col-span-2">
                  <label class="block text-xs text-text-secondary mb-1">
                    凭据
                    <span v-if="profile.api_key_set" class="text-text-tertiary">（已配置，留空则不修改）</span>
                  </label>
                  <input
                    v-model="draft.api_key"
                    type="password"
                    autocomplete="off"
                    :placeholder="profile.api_key_set ? '••••••••' : '必填'"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  />
                </div>
                <div>
                  <label class="block text-xs text-text-secondary mb-1">接口地址</label>
                  <input
                    v-model="draft.base_url"
                    placeholder="留空使用官方端点"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  />
                </div>
                <div>
                  <label class="block text-xs text-text-secondary mb-1">
                    模型 <span class="text-text-tertiary">（留空由 CLI 自选）</span>
                  </label>
                  <input
                    v-model="draft.model"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  />
                </div>
                <div v-for="key in backend.extra_env_keys" :key="key">
                  <label class="block text-xs text-text-secondary mb-1 font-mono">{{ key }}</label>
                  <input
                    v-model="draft.extra_env[key]"
                    class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
                  />
                </div>
              </div>
              <div class="flex items-center gap-2 mt-3">
                <button
                  @click="save(backend)"
                  :disabled="busy === 'save'"
                  class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-50"
                >
                  {{ busy === 'save' ? '保存中...' : '保存' }}
                </button>
                <button
                  @click="cancelEdit"
                  class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-text-secondary hover:bg-hover"
                >
                  取消
                </button>
                <span v-if="errors._form" class="text-xs text-red-500">{{ errors._form }}</span>
              </div>
            </div>
          </li>
        </ul>

        <div
          v-if="editing === `new:${backend.backend}` && draft"
          class="px-4 py-3 border-t border-border-subtle"
        >
          <div class="grid grid-cols-2 gap-3">
            <div>
              <label class="block text-xs text-text-secondary mb-1">配置名</label>
              <input
                v-model="draft.name"
                placeholder="如 penguinapi"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              />
            </div>
            <div v-if="backend.auth_kind_options.length > 1">
              <label class="block text-xs text-text-secondary mb-1">凭据类型</label>
              <select
                v-model="draft.auth_kind"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              >
                <option v-for="option in backend.auth_kind_options" :key="option" :value="option">
                  {{ option }}
                </option>
              </select>
            </div>
            <div class="col-span-2">
              <label class="block text-xs text-text-secondary mb-1">凭据</label>
              <input
                v-model="draft.api_key"
                type="password"
                autocomplete="off"
                placeholder="必填"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              />
            </div>
            <div>
              <label class="block text-xs text-text-secondary mb-1">接口地址</label>
              <input
                v-model="draft.base_url"
                placeholder="留空使用官方端点"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              />
            </div>
            <div>
              <label class="block text-xs text-text-secondary mb-1">
                模型 <span class="text-text-tertiary">（留空由 CLI 自选）</span>
              </label>
              <input
                v-model="draft.model"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              />
            </div>
            <div v-for="key in backend.extra_env_keys" :key="key">
              <label class="block text-xs text-text-secondary mb-1 font-mono">{{ key }}</label>
              <input
                v-model="draft.extra_env[key]"
                class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
              />
            </div>
          </div>
          <div class="flex items-center gap-2 mt-3">
            <button
              @click="save(backend)"
              :disabled="busy === 'save'"
              class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-50"
            >
              {{ busy === 'save' ? '创建中...' : '创建' }}
            </button>
            <button
              @click="cancelEdit"
              class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-text-secondary hover:bg-hover"
            >
              取消
            </button>
            <span v-if="errors._form" class="text-xs text-red-500">{{ errors._form }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
