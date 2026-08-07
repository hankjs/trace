<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { createBindCode, getBinding, unbind, type WeixinBinding } from "../api/weixin";
import { createFeishuBindCode, getFeishuBinding, unbindFeishu, type FeishuBinding } from "../api/feishu";
import { getHandyAccount, putHandyAccount, testHandyAccount, type HandyAccount } from "../api/handy";
import { useRemoteTerm } from "../composables/useRemoteTerm";

interface AgentConfig {
  name: string;
  agent_type: string;
  binary_path: string;
}

const router = useRouter();

// 远程终端（admin 网页 / P2P 控本机终端）
const {
  clientId: remoteClientId,
  acceptRemote,
  isPolling: remoteIsPolling,
  setAcceptRemote,
} = useRemoteTerm();
const acceptRemoteLocal = ref(acceptRemote.value);
const remoteToggling = ref(false);

async function toggleAcceptRemote(v: boolean) {
  remoteToggling.value = true;
  try {
    await setAcceptRemote(v);
    acceptRemoteLocal.value = acceptRemote.value;
  } finally {
    remoteToggling.value = false;
  }
}

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
  loadHandy();
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

// ---------- 飞书绑定 ----------

const fsBinding = ref<FeishuBinding | null>(null);
const fsBindCode = ref("");
const fsBindExpiresAt = ref(0);
const fsNow = ref(Date.now());
const fsGeneratingCode = ref(false);
const fsUnbinding = ref(false);
let fsCountdownTimer: ReturnType<typeof setInterval> | undefined;
let fsPollTimer: ReturnType<typeof setInterval> | undefined;

const fsCodeExpired = computed(() => !fsBindCode.value || fsNow.value >= fsBindExpiresAt.value);
const fsCountdownText = computed(() => {
  const remain = Math.max(0, fsBindExpiresAt.value - fsNow.value);
  const m = Math.floor(remain / 60000);
  const s = Math.floor((remain % 60000) / 1000);
  return `${m}:${String(s).padStart(2, "0")}`;
});

async function loadFsBinding() {
  const result = await getFeishuBinding();
  if (result.ok) fsBinding.value = result.data ?? null;
}

function stopFsPolling() {
  clearInterval(fsPollTimer);
  fsPollTimer = undefined;
}

function startFsPolling() {
  stopFsPolling();
  fsPollTimer = setInterval(async () => {
    const result = await getFeishuBinding();
    if (result.ok && result.data) fsBinding.value = result.data;
  }, 5000);
}

watch(
  fsBinding,
  (value) => {
    if (value) stopFsPolling();
    else startFsPolling();
  },
  { immediate: true }
);

function startFsCountdown() {
  clearInterval(fsCountdownTimer);
  fsNow.value = Date.now();
  fsCountdownTimer = setInterval(() => {
    fsNow.value = Date.now();
    if (fsNow.value >= fsBindExpiresAt.value) clearInterval(fsCountdownTimer);
  }, 1000);
}

async function generateFsCode() {
  fsGeneratingCode.value = true;
  try {
    const result = await createFeishuBindCode();
    if (result.ok && result.data) {
      fsBindCode.value = result.data.code;
      fsBindExpiresAt.value = result.data.expires_at;
      startFsCountdown();
    }
  } finally {
    fsGeneratingCode.value = false;
  }
}

async function confirmFsUnbind() {
  if (!confirm("确定解绑飞书？解绑后将无法通过飞书机器人驱动会话。")) return;
  fsUnbinding.value = true;
  try {
    const result = await unbindFeishu();
    if (result.ok) {
      fsBinding.value = null;
      fsBindCode.value = "";
    }
  } finally {
    fsUnbinding.value = false;
  }
}

onMounted(() => {
  loadFsBinding();
});

// ---------- handy 渠道 ----------

const handyAccount = ref<HandyAccount | null>(null);
const handyBaseUrl = ref("");
const handyToken = ref("");
const handySecret = ref("");
const handyEnabled = ref(true);
const handySaving = ref(false);
const handyTesting = ref(false);
const handySaveOk = ref(false);
const handySaveMsg = ref("");
const handyTestResult = ref<{ ok: boolean; message: string } | null>(null);
const handyCopied = ref(false);

async function loadHandy() {
  const result = await getHandyAccount();
  if (result.ok && result.data) {
    handyAccount.value = result.data;
    handyBaseUrl.value = result.data.base_url;
    handyEnabled.value = result.data.enabled;
  }
}

async function saveHandy() {
  handySaving.value = true;
  handySaveMsg.value = "";
  try {
    const result = await putHandyAccount({
      base_url: handyBaseUrl.value.trim(),
      token: handyToken.value.trim(),
      webhook_secret: handySecret.value.trim(),
      enabled: handyEnabled.value,
    });
    if (result.ok && result.data) {
      handyAccount.value = result.data;
      handyBaseUrl.value = result.data.base_url;
      handyEnabled.value = result.data.enabled;
      // 清空输入框：掩码态下空串 = 保留旧值，避免误把掩码写回去
      handyToken.value = "";
      handySecret.value = "";
      handySaveOk.value = true;
      handySaveMsg.value = "已保存";
    } else {
      handySaveOk.value = false;
      handySaveMsg.value = result.msg || "保存失败";
    }
  } finally {
    handySaving.value = false;
  }
}

async function testHandy() {
  handyTesting.value = true;
  handyTestResult.value = null;
  try {
    const result = await testHandyAccount({
      base_url: handyBaseUrl.value.trim(),
      token: handyToken.value.trim(),
    });
    if (!result.ok) {
      handyTestResult.value = { ok: false, message: result.msg || "测试失败" };
    } else if (result.data?.ok) {
      const who = result.data.token_name || "handy";
      const hook = result.data.webhook_configured ? "webhook 已配置" : "webhook 未配置";
      handyTestResult.value = { ok: true, message: `连接成功：${who}（${hook}）` };
    } else {
      handyTestResult.value = { ok: false, message: result.data?.error || "连接失败" };
    }
  } finally {
    handyTesting.value = false;
  }
}

async function copyHandyWebhookUrl() {
  const url = handyAccount.value?.webhook_url;
  if (!url) return;
  try {
    await navigator.clipboard.writeText(url);
    handyCopied.value = true;
    setTimeout(() => (handyCopied.value = false), 1500);
  } catch {
    // 剪贴板不可用时静默失败，用户可手动选中复制
  }
}

onUnmounted(() => {
  clearInterval(countdownTimer);
  stopPolling();
  clearInterval(fsCountdownTimer);
  stopFsPolling();
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

      <h3 class="section-title weixin-title">远程终端</h3>
      <div class="weixin-section">
        <p class="weixin-desc">
          开启后本机会向 server 注册并长轮询；admin 网页可查看/操作本机终端，优先走 WebRTC 直连（失败回落中转）。
        </p>
        <label class="remote-toggle-row">
          <input
            type="checkbox"
            :checked="acceptRemoteLocal"
            :disabled="remoteToggling"
            @change="toggleAcceptRemote(($event.target as HTMLInputElement).checked)"
          />
          允许远程终端控制
        </label>
        <div class="weixin-bound-info" style="margin-top: 8px">
          <span class="weixin-label">Client ID</span>
          <span class="weixin-value" style="font-family: monospace; font-size: 12px">{{ remoteClientId }}</span>
        </div>
        <div class="weixin-bound-info">
          <span class="weixin-label">状态</span>
          <span class="weixin-value">
            {{ acceptRemoteLocal ? (remoteIsPolling ? "在线轮询中" : "已开启（轮询未运行）") : "已关闭" }}
          </span>
        </div>
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

      <h3 class="section-title weixin-title">飞书绑定</h3>
      <div class="weixin-section">
        <p class="weixin-desc">绑定后可在飞书话题群中 @机器人 派发任务、审批确认。</p>

        <!-- 已绑定 -->
        <div v-if="fsBinding" class="weixin-bound">
          <div class="weixin-bound-info">
            <span class="weixin-label">飞书用户</span>
            <span class="weixin-value">{{ fsBinding.open_id }}</span>
          </div>
          <div class="weixin-bound-info">
            <span class="weixin-label">绑定时间</span>
            <span class="weixin-value">{{ formatTime(fsBinding.created_at) }}</span>
          </div>
          <button class="btn-sm btn-danger" :disabled="fsUnbinding" @click="confirmFsUnbind">
            {{ fsUnbinding ? "解绑中..." : "解绑" }}
          </button>
        </div>

        <!-- 未绑定 -->
        <div v-else class="weixin-unbound">
          <template v-if="fsBindCode && !fsCodeExpired">
            <div class="bind-code">{{ fsBindCode }}</div>
            <p class="weixin-hint">有效期剩余 {{ fsCountdownText }}</p>
            <p class="weixin-hint">打开飞书，向机器人发送：<code>bind {{ fsBindCode }}</code></p>
          </template>
          <button class="btn-primary" :disabled="fsGeneratingCode" @click="generateFsCode">
            {{ fsGeneratingCode ? "生成中..." : fsBindCode ? "重新生成绑定码" : "生成绑定码" }}
          </button>
        </div>
      </div>

      <h3 class="section-title weixin-title">handy 渠道</h3>
      <div class="weixin-section">
        <p class="weixin-desc">配置 handy 连接后，agent 进度卡片和人工闸门会推送到你的 handy 网页，可在 handy 侧直接操作回推。</p>

        <!-- 已保存：展示 webhook 回推地址 -->
        <div v-if="handyAccount?.webhook_url" class="handy-webhook">
          <p class="weixin-hint">
            Webhook 地址：到 handy 的「接入凭证」页新建凭证时把这个地址填进 Webhook 地址，再把 handy 返回的 token 和 secret 填回下面保存。
          </p>
          <div class="handy-webhook-row">
            <code class="handy-webhook-url">{{ handyAccount.webhook_url }}</code>
            <button class="btn-sm" @click="copyHandyWebhookUrl">{{ handyCopied ? "已复制" : "复制" }}</button>
          </div>
        </div>

        <div class="add-form handy-form">
          <input v-model="handyBaseUrl" placeholder="handy 服务地址，如 https://handy.example.com" class="input" />
          <input
            v-model="handyToken"
            type="password"
            :placeholder="handyAccount?.token ? 'API token（已配置，留空不变）' : 'API token（hnk_ 开头）'"
            class="input"
          />
          <input
            v-model="handySecret"
            type="password"
            :placeholder="handyAccount?.webhook_secret ? 'webhook_secret（已配置，留空不变）' : 'webhook_secret'"
            class="input"
          />
          <label class="remote-toggle-row handy-toggle">
            <input v-model="handyEnabled" type="checkbox" />
            启用 handy 推送
          </label>
          <div class="form-actions">
            <button class="btn-primary" :disabled="handySaving || !handyBaseUrl.trim()" @click="saveHandy">
              {{ handySaving ? "保存中..." : "保存" }}
            </button>
            <button class="btn-sm" :disabled="handyTesting" @click="testHandy">
              {{ handyTesting ? "测试中..." : "测试连接" }}
            </button>
          </div>
          <div v-if="handySaveMsg" class="test-result" :class="{ ok: handySaveOk }">{{ handySaveMsg }}</div>
          <div v-if="handyTestResult" class="test-result" :class="{ ok: handyTestResult.ok }">
            {{ handyTestResult.message }}
          </div>
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
.handy-webhook { margin-bottom: 0.75rem; }
.handy-webhook-row { display: flex; align-items: center; gap: 0.5rem; margin-top: 0.25rem; }
.handy-webhook-url { flex: 1; font-family: monospace; font-size: 0.8rem; background: var(--color-surface-1, #1a1a1a); padding: 0.4rem 0.5rem; border-radius: 0.25rem; color: var(--color-text-primary); word-break: break-all; }
.handy-form { margin-top: 0.25rem; }
.handy-toggle { margin-bottom: 0; }
</style>
