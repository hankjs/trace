<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { createBindCode, getBinding, unbind, type WeixinBinding } from "../api/weixin";
import { useRemoteExec } from "../composables/useRemoteExec";

const remoteExec = useRemoteExec();

interface AgentConfig {
  name: string;
  agent_type: string;
  binary_path: string;
}

const router = useRouter();

const agents = ref<AgentConfig[]>([]);
const newName = ref("");
const newType = ref("claude-code");
const newPath = ref("");
const testResult = ref<Record<string, { ok: boolean; message: string }>>({});
const isAdding = ref(false);

function goBack() {
  router.back();
}

async function loadAgents() {
  try {
    agents.value = await invoke<AgentConfig[]>("acp_get_agents");
  } catch (e: any) {
    console.error("Failed to load agents:", e);
  }
}

async function addAgent() {
  if (!newName.value.trim() || !newPath.value.trim()) return;
  try {
    await invoke("acp_add_agent", {
      name: newName.value.trim(),
      agentType: newType.value,
      binaryPath: newPath.value.trim(),
    });
    newName.value = "";
    newPath.value = "";
    isAdding.value = false;
    await loadAgents();
  } catch (e: any) {
    console.error("Failed to add agent:", e);
  }
}

async function removeAgent(name: string) {
  try {
    await invoke("acp_remove_agent", { name });
    await loadAgents();
  } catch (e: any) {
    console.error("Failed to remove agent:", e);
  }
}

async function testAgent(name: string) {
  testResult.value[name] = { ok: false, message: "Testing..." };
  try {
    const msg = await invoke<string>("acp_test_agent", { name });
    testResult.value[name] = { ok: true, message: msg };
  } catch (e: any) {
    testResult.value[name] = { ok: false, message: String(e) };
  }
}

async function browsePath() {
  const selected = await open({
    multiple: false,
    directory: false,
    title: "Select agent binary",
  });
  if (selected) {
    newPath.value = selected as string;
  }
}

onMounted(() => {
  loadAgents();
  loadBinding();
});

// ---------- 微信绑定 ----------

const binding = ref<WeixinBinding | null>(null);
const bindCode = ref("");
const bindExpiresAt = ref(0);
const now = ref(Date.now());
const generatingCode = ref(false);
const unbinding = ref(false);
let countdownTimer: ReturnType<typeof setInterval> | undefined;
let pollTimer: ReturnType<typeof setInterval> | undefined;

const codeExpired = computed(() => !bindCode.value || now.value >= bindExpiresAt.value);
const countdownText = computed(() => {
  const remain = Math.max(0, bindExpiresAt.value - now.value);
  const m = Math.floor(remain / 60000);
  const s = Math.floor((remain % 60000) / 1000);
  return `${m}:${String(s).padStart(2, "0")}`;
});

async function loadBinding() {
  const result = await getBinding();
  if (result.ok) binding.value = result.data ?? null;
}

function stopPolling() {
  clearInterval(pollTimer);
  pollTimer = undefined;
}

function startPolling() {
  stopPolling();
  pollTimer = setInterval(async () => {
    const result = await getBinding();
    if (result.ok && result.data) binding.value = result.data;
  }, 5000);
}

// 未绑定期间每 5 秒轮询一次绑定状态，拿到绑定后自动切到已绑定态并停止轮询
watch(
  binding,
  (value) => {
    if (value) stopPolling();
    else startPolling();
  },
  { immediate: true }
);

function startCountdown() {
  clearInterval(countdownTimer);
  now.value = Date.now();
  countdownTimer = setInterval(() => {
    now.value = Date.now();
    if (now.value >= bindExpiresAt.value) clearInterval(countdownTimer);
  }, 1000);
}

async function generateCode() {
  generatingCode.value = true;
  try {
    const result = await createBindCode();
    if (result.ok && result.data) {
      bindCode.value = result.data.code;
      bindExpiresAt.value = result.data.expires_at;
      startCountdown();
    }
  } finally {
    generatingCode.value = false;
  }
}

async function confirmUnbind() {
  if (!confirm("确定解绑微信？解绑后将无法通过微信机器人驱动会话。")) return;
  unbinding.value = true;
  try {
    const result = await unbind();
    if (result.ok) {
      binding.value = null;
      bindCode.value = "";
    }
  } finally {
    unbinding.value = false;
  }
}

function formatTime(iso: string) {
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString();
}

// ---------- 远程执行 ----------

const remoteAccept = ref(remoteExec.acceptRemote.value);
const remoteWorkDir = ref(remoteExec.workDir.value);
const remoteSaving = ref(false);

async function toggleRemoteAccept() {
  remoteSaving.value = true;
  try {
    await remoteExec.setAcceptRemote(remoteAccept.value);
  } finally {
    remoteSaving.value = false;
  }
}

async function chooseRemoteWorkDir() {
  const selected = await open({ multiple: false, directory: true, title: "选择远程执行工作目录" });
  if (selected) {
    remoteWorkDir.value = selected as string;
    await remoteExec.setWorkDir(remoteWorkDir.value);
  }
}

async function saveRemoteWorkDir() {
  await remoteExec.setWorkDir(remoteWorkDir.value.trim());
}

onUnmounted(() => {
  clearInterval(countdownTimer);
  stopPolling();
});
</script>

<template>
  <div class="settings-page">
    <div class="settings-panel">
      <div class="settings-header">
        <button class="back-btn" @click="goBack()">&larr;</button>
        <h2>设置</h2>
      </div>

      <h3 class="section-title">Local Agents</h3>
      <div class="agent-list">
        <div v-for="agent in agents" :key="agent.name" class="agent-item">
          <div class="agent-info">
            <span class="agent-name">{{ agent.name }}</span>
            <span class="agent-type">{{ agent.agent_type }}</span>
            <span class="agent-path">{{ agent.binary_path }}</span>
          </div>
          <div class="agent-actions">
            <button class="btn-sm" @click="testAgent(agent.name)">Test</button>
            <button class="btn-sm btn-danger" @click="removeAgent(agent.name)">Remove</button>
          </div>
          <div v-if="testResult[agent.name]" class="test-result" :class="{ ok: testResult[agent.name].ok }">
            {{ testResult[agent.name].message }}
          </div>
        </div>
        <div v-if="agents.length === 0" class="empty-state">
          No agents configured. Add one below.
        </div>
      </div>

      <div v-if="isAdding" class="add-form">
        <input v-model="newName" placeholder="Agent name (e.g. claude-code)" class="input" />
        <select v-model="newType" class="input">
          <option value="claude-code">Claude Code</option>
          <option value="codex">Codex</option>
          <option value="custom">Custom</option>
        </select>
        <div class="path-row">
          <input v-model="newPath" placeholder="Binary path" class="input flex-1" />
          <button class="btn-sm" @click="browsePath">Browse</button>
        </div>
        <div class="form-actions">
          <button class="btn-primary" @click="addAgent">Add</button>
          <button class="btn-sm" @click="isAdding = false">Cancel</button>
        </div>
      </div>
      <button v-else class="btn-primary" @click="isAdding = true">+ Add Agent</button>

      <h3 class="section-title weixin-title">远程执行</h3>
      <div class="weixin-section">
        <p class="weixin-desc">开启后，server 端会话的 fs/shell 工具调用可下发到本机，在下方工作目录中执行。</p>
        <label class="remote-toggle-row">
          <input type="checkbox" v-model="remoteAccept" :disabled="remoteSaving" @change="toggleRemoteAccept" />
          <span>接受远程执行任务</span>
        </label>
        <div class="remote-status" :class="{ on: remoteExec.isPolling.value }">
          {{ remoteExec.isPolling.value ? "已连接，等待任务" : remoteAccept ? "未连接" : "未开启" }}
        </div>
        <div class="remote-workdir-row">
          <input v-model="remoteWorkDir" class="input flex-1" placeholder="工作目录（远程任务在此目录执行）" @change="saveRemoteWorkDir" />
          <button class="btn-sm" @click="chooseRemoteWorkDir">选择目录</button>
        </div>
        <p class="weixin-hint">client_id：<code>{{ remoteExec.clientId }}</code></p>
      </div>

      <h3 class="section-title weixin-title">微信绑定</h3>
      <div class="weixin-section">
        <p class="weixin-desc">绑定后可在微信中通过机器人远程驱动会话。</p>

        <!-- 已绑定 -->
        <div v-if="binding" class="weixin-bound">
          <div class="weixin-bound-info">
            <span class="weixin-label">微信用户</span>
            <span class="weixin-value">{{ binding.ilink_user_id }}</span>
          </div>
          <div class="weixin-bound-info">
            <span class="weixin-label">绑定时间</span>
            <span class="weixin-value">{{ formatTime(binding.created_at) }}</span>
          </div>
          <button class="btn-sm btn-danger" :disabled="unbinding" @click="confirmUnbind">
            {{ unbinding ? "解绑中..." : "解绑" }}
          </button>
        </div>

        <!-- 未绑定 -->
        <div v-else class="weixin-unbound">
          <template v-if="bindCode && !codeExpired">
            <div class="bind-code">{{ bindCode }}</div>
            <p class="weixin-hint">有效期剩余 {{ countdownText }}</p>
            <p class="weixin-hint">打开微信，向机器人发送：<code>bind {{ bindCode }}</code></p>
          </template>
          <button class="btn-primary" :disabled="generatingCode" @click="generateCode">
            {{ generatingCode ? "生成中..." : bindCode ? "重新生成绑定码" : "生成绑定码" }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-page {
  height: 100%;
  overflow-y: auto;
  background: var(--color-surface-0, #0a0a0a);
}
.settings-panel {
  padding: 2rem 1.5rem;
  max-width: 600px;
  margin: 0 auto;
}
.settings-header {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 1.5rem;
}
.settings-header h2 {
  font-size: 1.125rem;
  font-weight: 600;
}
.back-btn {
  background: none;
  border: none;
  font-size: 1.25rem;
  cursor: pointer;
  color: var(--color-text-secondary);
  padding: 4px 8px;
  border-radius: 4px;
}
.back-btn:hover {
  background: var(--color-surface-1, #1a1a1a);
  color: var(--color-text-primary);
}
.agent-list { margin-bottom: 1rem; }
.agent-item { padding: 0.75rem; border: 1px solid var(--color-border, #333); border-radius: 0.5rem; margin-bottom: 0.5rem; }
.agent-info { display: flex; flex-direction: column; gap: 0.25rem; margin-bottom: 0.5rem; }
.agent-name { font-weight: 600; }
.agent-type { font-size: 0.8rem; color: var(--color-text-secondary); }
.agent-path { font-size: 0.75rem; font-family: monospace; color: var(--color-text-tertiary, #888); }
.agent-actions { display: flex; gap: 0.5rem; }
.test-result { margin-top: 0.5rem; font-size: 0.8rem; color: var(--color-error, #f44); }
.test-result.ok { color: var(--color-success, #4f4); }
.empty-state { color: var(--color-text-secondary); text-align: center; padding: 2rem; }
.add-form { display: flex; flex-direction: column; gap: 0.5rem; }
.path-row { display: flex; gap: 0.5rem; }
.input { padding: 0.5rem; border: 1px solid var(--color-border, #333); border-radius: 0.375rem; background: var(--color-surface-1, #1a1a1a); color: var(--color-text-primary); width: 100%; }
.form-actions { display: flex; gap: 0.5rem; }
.btn-sm { padding: 0.25rem 0.75rem; border-radius: 0.375rem; border: 1px solid var(--color-border, #333); background: var(--color-surface-1, #1a1a1a); color: var(--color-text-primary); cursor: pointer; font-size: 0.8rem; }
.btn-sm:hover { background: var(--color-surface-2, #2a2a2a); }
.btn-danger { color: var(--color-error, #f44); border-color: var(--color-error, #f44); }
.btn-primary { padding: 0.5rem 1rem; border-radius: 0.375rem; border: none; background: var(--color-accent, #6366f1); color: white; cursor: pointer; font-size: 0.875rem; }
.btn-primary:hover { opacity: 0.9; }
.btn-primary:disabled { opacity: 0.5; cursor: default; }
.section-title { font-size: 0.8rem; font-weight: 600; color: var(--color-text-secondary); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.75rem; }
.weixin-title { margin-top: 2rem; }
.weixin-section { padding: 1rem; border: 1px solid var(--color-border, #333); border-radius: 0.5rem; }
.weixin-desc { font-size: 0.85rem; color: var(--color-text-secondary); margin-bottom: 0.75rem; }
.weixin-unbound { display: flex; flex-direction: column; align-items: flex-start; gap: 0.5rem; }
.bind-code { font-size: 2rem; font-weight: 700; font-family: monospace; letter-spacing: 0.3em; color: var(--color-text-primary); }
.weixin-hint { font-size: 0.8rem; color: var(--color-text-secondary); }
.weixin-hint code { font-family: monospace; background: var(--color-surface-1, #1a1a1a); padding: 0.1rem 0.4rem; border-radius: 0.25rem; color: var(--color-text-primary); }
.weixin-bound { display: flex; flex-direction: column; gap: 0.5rem; align-items: flex-start; }
.weixin-bound-info { display: flex; gap: 0.75rem; font-size: 0.85rem; }
.weixin-label { color: var(--color-text-secondary); min-width: 4rem; }
.weixin-value { font-family: monospace; color: var(--color-text-primary); }
.remote-toggle-row { display: flex; align-items: center; gap: 0.5rem; font-size: 0.875rem; color: var(--color-text-primary); cursor: pointer; margin-bottom: 0.5rem; }
.remote-status { font-size: 0.8rem; color: var(--color-text-secondary); margin-bottom: 0.75rem; }
.remote-status.on { color: var(--color-success, #4f4); }
.remote-workdir-row { display: flex; gap: 0.5rem; margin-bottom: 0.5rem; }
</style>
