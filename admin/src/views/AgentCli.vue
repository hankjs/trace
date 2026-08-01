<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api, type AgentCliConfig, type AgentCliTestResult } from '../composables/api'

/** 每个后端一份可编辑草稿；api_key 单独存，留空表示不修改已存凭据。 */
interface Draft {
  auth_kind: string
  api_key: string
  base_url: string
  model: string
  extra_env: Record<string, string>
  enabled: boolean
}

const configs = ref<AgentCliConfig[]>([])
const drafts = ref<Record<string, Draft>>({})
const loading = ref(true)
const saving = ref<string | null>(null)
const testing = ref<string | null>(null)
const results = ref<Record<string, AgentCliTestResult>>({})
const errors = ref<Record<string, string>>({})
const notice = ref<Record<string, string>>({})

const BACKEND_TITLES: Record<string, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
}

const SOURCE_LABELS: Record<string, string> = {
  db: '本页配置',
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

function toDraft(config: AgentCliConfig): Draft {
  // extra_env 按后端白名单补全空键，让每个可配项都有输入框。
  const extra: Record<string, string> = {}
  for (const key of config.extra_env_keys) {
    extra[key] = config.extra_env[key] ?? ''
  }
  return {
    auth_kind: config.auth_kind,
    api_key: '',
    base_url: config.base_url,
    model: config.model,
    extra_env: extra,
    enabled: config.enabled,
  }
}

async function load() {
  loading.value = true
  try {
    configs.value = await api.listAgentCliConfigs()
    const next: Record<string, Draft> = {}
    for (const config of configs.value) {
      next[config.backend] = toDraft(config)
    }
    drafts.value = next
  } catch (e) {
    errors.value = { _load: e instanceof Error ? e.message : '加载失败' }
  }
  loading.value = false
}

async function save(backend: string) {
  const draft = drafts.value[backend]
  if (!draft) return
  saving.value = backend
  delete errors.value[backend]
  delete notice.value[backend]
  try {
    // 只提交非空的附加变量，空值表示不设置该变量。
    const extra: Record<string, string> = {}
    for (const [key, value] of Object.entries(draft.extra_env)) {
      if (value.trim()) extra[key] = value.trim()
    }
    await api.updateAgentCliConfig(backend, {
      auth_kind: draft.auth_kind,
      api_key: draft.api_key,
      base_url: draft.base_url,
      model: draft.model,
      extra_env: extra,
      enabled: draft.enabled,
    })
    notice.value[backend] = '已保存，下一轮任务生效'
    await load()
  } catch (e) {
    errors.value[backend] = e instanceof Error ? e.message : '保存失败'
  }
  saving.value = null
}

async function test(backend: string) {
  testing.value = backend
  delete results.value[backend]
  delete errors.value[backend]
  try {
    results.value[backend] = await api.testAgentCliConfig(backend)
  } catch (e) {
    errors.value[backend] = e instanceof Error ? e.message : '测试失败'
  }
  testing.value = null
}

/** 停用只是不再使用，凭据仍留在库里；轮换掉泄露的 key 需要真正删除。 */
async function remove(backend: string) {
  if (!confirm(`清除 ${BACKEND_TITLES[backend] || backend} 在库里的凭据？之后回退到服务器上的 agent-cli.env。`)) {
    return
  }
  delete errors.value[backend]
  delete notice.value[backend]
  try {
    await api.deleteAgentCliConfig(backend)
    notice.value[backend] = '已清除，回退到服务器环境文件'
    await load()
  } catch (e) {
    errors.value[backend] = e instanceof Error ? e.message : '清除失败'
  }
}

onMounted(load)
</script>

<template>
  <div>
    <div class="mb-6">
      <h1 class="text-lg font-semibold text-text-primary">Agent CLI 凭据</h1>
      <p class="mt-1 text-xs text-text-tertiary">
        飞书任务里 Codex / Claude Code 使用的第三方 API 配置。保存后下一轮任务即生效，无需重启服务。
        留空凭据表示保留已有值；停用后回退到服务器上的 agent-cli.env。
      </p>
    </div>

    <div v-if="errors._load" class="mb-4 text-sm text-red-500">{{ errors._load }}</div>
    <div v-if="loading" class="text-sm text-text-tertiary">加载中...</div>

    <div v-else class="space-y-4">
      <div
        v-for="config in configs"
        :key="config.backend"
        class="p-4 border border-border-subtle rounded-lg"
      >
        <div class="flex items-center justify-between mb-3">
          <div>
            <h2 class="text-sm font-medium text-text-primary">
              {{ BACKEND_TITLES[config.backend] || config.backend }}
            </h2>
            <p class="mt-0.5 text-[11px]" :class="sourceTone(config.effective_source)">
              当前生效：{{ sourceLabel(config.effective_source) }}
              <span v-if="config.updated_at" class="text-text-tertiary">
                · {{ new Date(config.updated_at).toLocaleString() }} 由 {{ config.updated_by || '未知' }} 更新
              </span>
            </p>
          </div>
          <label class="flex items-center gap-2 text-xs text-text-secondary">
            <input v-model="drafts[config.backend].enabled" type="checkbox" />
            启用本页配置
          </label>
        </div>

        <div class="grid grid-cols-2 gap-3">
          <div v-if="config.auth_kind_options.length > 1">
            <label class="block text-xs text-text-secondary mb-1">凭据类型</label>
            <select
              v-model="drafts[config.backend].auth_kind"
              class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
            >
              <option v-for="option in config.auth_kind_options" :key="option" :value="option">
                {{ option }}
              </option>
            </select>
          </div>
          <div :class="config.auth_kind_options.length > 1 ? '' : 'col-span-2'">
            <label class="block text-xs text-text-secondary mb-1">
              凭据
              <span v-if="config.api_key_set" class="text-text-tertiary">（已配置，留空则不修改）</span>
            </label>
            <input
              v-model="drafts[config.backend].api_key"
              type="password"
              autocomplete="off"
              :placeholder="config.api_key_set ? '••••••••' : '必填'"
              class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
            />
          </div>
          <div>
            <label class="block text-xs text-text-secondary mb-1">接口地址</label>
            <input
              v-model="drafts[config.backend].base_url"
              placeholder="留空使用官方端点"
              class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
            />
          </div>
          <div>
            <label class="block text-xs text-text-secondary mb-1">
              模型 <span class="text-text-tertiary">（留空由 CLI 自选）</span>
            </label>
            <input
              v-model="drafts[config.backend].model"
              class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
            />
          </div>
          <div v-for="key in config.extra_env_keys" :key="key">
            <label class="block text-xs text-text-secondary mb-1 font-mono">{{ key }}</label>
            <input
              v-model="drafts[config.backend].extra_env[key]"
              class="w-full px-2 py-1.5 text-sm border border-border-subtle rounded bg-transparent text-text-primary"
            />
          </div>
        </div>

        <div class="flex items-center gap-2 mt-3">
          <button
            @click="save(config.backend)"
            :disabled="saving === config.backend"
            class="px-3 py-1.5 text-xs bg-accent text-white rounded-md hover:opacity-90 disabled:opacity-50"
          >
            {{ saving === config.backend ? '保存中...' : '保存' }}
          </button>
          <button
            @click="test(config.backend)"
            :disabled="testing === config.backend || !config.api_key_set"
            class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-text-secondary hover:bg-hover disabled:opacity-40"
            :title="config.api_key_set ? '向配置的端点发一次最小请求' : '先保存凭据'"
          >
            {{ testing === config.backend ? '测试中...' : '测试连通性' }}
          </button>
          <button
            v-if="config.api_key_set"
            @click="remove(config.backend)"
            class="px-3 py-1.5 text-xs border border-border-subtle rounded-md text-red-500 hover:bg-hover"
            title="清除库里的凭据，回退到服务器环境文件"
          >
            清除凭据
          </button>
          <span v-if="notice[config.backend]" class="text-xs text-accent">
            {{ notice[config.backend] }}
          </span>
          <span v-if="errors[config.backend]" class="text-xs text-red-500">
            {{ errors[config.backend] }}
          </span>
        </div>

        <div
          v-if="results[config.backend]"
          class="mt-3 p-2 text-xs rounded border"
          :class="results[config.backend].ok
            ? 'border-border-subtle text-text-secondary'
            : 'border-red-500/40 text-red-500'"
        >
          <div>{{ results[config.backend].ok ? '✓' : '✗' }} {{ results[config.backend].message }}</div>
          <pre
            v-if="results[config.backend].detail"
            class="mt-1 whitespace-pre-wrap break-all font-mono text-[11px] text-text-tertiary"
          >{{ results[config.backend].detail }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>
