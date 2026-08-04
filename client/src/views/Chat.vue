<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onUnmounted, onDeactivated, watch, toRef } from "vue";
import { useSession, apiRequest } from "../composables/useSession";
import { useMessageTree } from "../composables/useMessageTree";
import { useMessage } from "../composables/useMessage";
import { useSidebarPanels } from "../composables/useSidebarPanels";
import { useChatBlocks } from "../composables/useChatBlocks";
import { useChatSSE } from "../composables/useChatSSE";
import { useLocalAgent } from "../composables/useLocalAgent";
import { useChatActions } from "../composables/useChatActions";
import { listCheckpoints, rewindToCheckpoint, type Checkpoint } from "../api/checkpoints";
import AgentHeader from "../components/AgentHeader.vue";
import AgentInput from "../components/AgentInput.vue";
import FolderPicker from "../components/FolderPicker.vue";
import ChangeChatPanel from "../panels/ChangeChatPanel.vue";
import ArtifactReview from "../components/ArtifactReview.vue";
import ConversationOutline from "../components/ConversationOutline.vue";
import SpecPanel from "../panels/SpecPanel.vue";
import MessageList from "../components/chat/MessageList.vue";
import type { RenderItem, ToolCall } from "../types/chat";
import type { PendingImage, ProviderOption } from "../components/AgentInput.vue";

const props = defineProps<{ sessionId: string }>();

const { token: sessionToken, updateSessionTitle, updateSessionWorkDir, selectSession, sessions, currentSession, createSession, goBack, navigateTo } = useSession();
const { fetchTree, switchBranch, setActiveLeafId, activeLeafId, getSiblings, findLeafFromNode, treeNodes, scrollTargetId, clearScrollTarget } = useMessageTree();
const { warning: showWarning } = useMessage();
const { activePanelId, closePanel, registerPanel, reset: resetPanels } = useSidebarPanels();
registerPanel({ id: "changes", icon: "changes", title: "需求", order: 1 });
registerPanel({ id: "specs", icon: "specs", title: "Specs", order: 2 });
registerPanel({ id: "outline", icon: "outline", title: "Outline", order: 3 });

const sessionIdRef = toRef(props, "sessionId");
const isStreaming = ref(false);
const isConnected = ref(false);
const input = ref("");
const pendingImages = ref<PendingImage[]>([]);
const agentInputRef = ref<InstanceType<typeof AgentInput> | null>(null);
const messageListRef = ref<InstanceType<typeof MessageList> | null>(null);
const changesPanelRefreshKey = ref(0);
const reviewingChangeId = ref<string | null>(null);
const isCreatingSession = ref(false);
const isEditingWorkDir = ref(false);
const editWorkDir = ref("");
const editingMessageId = ref<string | null>(null);
const editingContent = ref("");
const checkpoints = ref<Checkpoint[]>([]);
const rewindingTo = ref<string | null>(null);

// Composables
const { blocks, renderItems, groupExpanded, isGroupExpanded, toggleGroup, collapseFinishedToolGroups, loadHistory, reset: resetBlocks } = useChatBlocks(sessionIdRef, isStreaming);

const messagesElProxy = computed(() => messageListRef.value?.messagesEl ?? null);

const sse = useChatSSE({
  blocks, sessionId: sessionIdRef, isStreaming, messagesEl: messagesElProxy,
  collapseFinishedToolGroups,
  onTurnComplete: () => { fetchTree(props.sessionId); },
  onChangeEvent: () => { changesPanelRefreshKey.value++; },
});

const local = useLocalAgent({
  blocks, sessionId: sessionIdRef, isStreaming, messagesEl: messagesElProxy,
  collapseFinishedToolGroups, activeLeafId, setActiveLeafId, fetchTree, currentSession,
});

const actions = useChatActions({
  blocks, sessionId: sessionIdRef, isStreaming, isConnected, renderItems, groupExpanded,
  messagesEl: messagesElProxy, input, pendingImages, agentInputRef,
  sessionEnvironment: local.sessionEnvironment,
  selectedProviderSource: local.selectedProviderSource,
  selectedProviderName: local.selectedProviderName,
  startStream: sse.startStream, stopStream: sse.stopStream,
  sendLocal: local.sendLocal, stopLocal: local.stopLocal,
});

// Derived state
const sessionTitle = computed(() => currentSession.value?.title || "");
const sessionWorkDir = computed(() => currentSession.value?.work_dir || "");
const displayDir = computed(() => currentSession.value?.work_dir || "");
const isEmpty = computed(() => blocks.value.length === 0 && !isStreaming.value);

// Checkpoint functions
async function fetchCheckpoints() {
  try { checkpoints.value = await listCheckpoints(props.sessionId); } catch { checkpoints.value = []; }
}
function getCheckpointForMessage(messageId: string): Checkpoint | undefined {
  return checkpoints.value.find(cp => cp.message_id === messageId);
}
async function handleRewind(checkpoint: Checkpoint) {
  if (!confirm(`回退到此消息时的状态？文件和对话都会恢复到这个时间点。`)) return;
  rewindingTo.value = checkpoint.id;
  try {
    await rewindToCheckpoint(props.sessionId, checkpoint.id);
    await fetchTree(props.sessionId);
    await loadHistory();
    await fetchCheckpoints();
  } catch (e: any) { alert(`回退失败: ${e.message || e}`); }
  finally { rewindingTo.value = null; }
}

// Header actions
function handleImagesChange(images: PendingImage[]) { pendingImages.value = images; }
async function handleUpdateTitle(newTitle: string) { await updateSessionTitle(props.sessionId, newTitle); }
function startEditWorkDir() { editWorkDir.value = currentSession.value?.work_dir || ""; isEditingWorkDir.value = true; }
function cancelEditWorkDir() { isEditingWorkDir.value = false; }
async function confirmEditWorkDir() {
  const newDir = editWorkDir.value.trim() || null;
  isEditingWorkDir.value = false;
  if (newDir !== (currentSession.value?.work_dir || null)) await updateSessionWorkDir(props.sessionId, newDir);
}
async function startSessionFromCurrentDir() {
  if (isCreatingSession.value) return;
  if (blocks.value.length === 0) { showWarning("当前还未开始对话"); return; }
  isCreatingSession.value = true;
  try {
    const workDir = currentSession.value?.work_dir || undefined;
    const environment = currentSession.value?.environment || "remote";
    const session = await createSession(workDir, environment, "chat");
    if (session) await navigateTo("chat", { sessionId: session.id });
    else showWarning("新建会话失败");
  } catch (e: any) { showWarning(e?.message || "新建会话失败"); }
  finally { isCreatingSession.value = false; }
}

// Message editing
function startEditMessage(item: Extract<RenderItem, { kind: "user" }>) {
  if (!item.messageId) return;
  editingMessageId.value = item.messageId;
  editingContent.value = item.content;
}
function cancelEditMessage() { editingMessageId.value = null; editingContent.value = ""; }
async function submitEditMessage() {
  if (!editingContent.value.trim() || !editingMessageId.value) return;
  const item = renderItems.value.find(i => i.kind === "user" && i.messageId === editingMessageId.value);
  if (!item || item.kind !== "user") return;
  const content = editingContent.value.trim();
  const parentId = item.parentId || "root";
  editingMessageId.value = null;
  editingContent.value = "";
  await actions.sendWithParent(content, parentId);
}

// Branch navigation
function getBranchSiblings(messageId: string) { return getSiblings(messageId); }
function getBranchIndex(messageId: string): { current: number; total: number } {
  const siblings = getSiblings(messageId).filter(s => s.role === "user");
  const idx = siblings.findIndex(s => s.id === messageId);
  return { current: idx, total: siblings.length };
}
async function switchToBranch(siblingId: string) {
  const leafId = findLeafFromNode(siblingId);
  await switchBranch(props.sessionId, leafId);
  await loadHistory(leafId);
  await fetchTree(props.sessionId);
}

// Panel handlers
function handleNavigateSession(sessionId: string) {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session) selectSession(session);
}
function handleReviewChange(changeId: string) { reviewingChangeId.value = changeId; closePanel(); }
function handleReviewConfirmed() { reviewingChangeId.value = null; changesPanelRefreshKey.value++; }

// Tool toggle
function toggleToolCall(tc: ToolCall) { tc.expanded = !tc.expanded; }

// Initial prompt restore
async function restoreInitialPrompt() {
  const key = `hank_initial_prompt:${props.sessionId}`;
  const raw = sessionStorage.getItem(key);
  if (!raw) return;
  if (blocks.value.length > 0 || input.value.trim()) { sessionStorage.removeItem(key); return; }
  try {
    const parsed = JSON.parse(raw) as { content?: string; autoSend?: boolean };
    if (parsed.autoSend && (!isConnected.value || isStreaming.value)) return;
    sessionStorage.removeItem(key);
    input.value = parsed.content || "";
    if (parsed.autoSend && input.value.trim()) await actions.send();
  } catch { sessionStorage.removeItem(key); input.value = raw; }
}

// Lifecycle
onMounted(async () => {
  isConnected.value = !!sessionToken.value;
  await loadHistory();
  await fetchTree(props.sessionId);
  await fetchCheckpoints();
  nextTick(() => { messageListRef.value?.scrollToBottom(); });

  // Fetch server providers
  try {
    const result = await apiRequest<{ providers: Array<{ name: string; type: string; default_model: string }>; default_provider: string }>("/api/providers");
    if (result.ok && result.data) local.serverProviders.value = result.data.providers;
  } catch { /* offline */ }

  await local.initListeners();
  await restoreInitialPrompt();
});

onUnmounted(() => { local.cleanup(); resetPanels(); });
onDeactivated(() => { resetPanels(); });

watch(() => props.sessionId, async () => {
  resetBlocks();
  checkpoints.value = [];
  editingMessageId.value = null;
  editingContent.value = "";
  isStreaming.value = false;
  closePanel();
  sse.resetState();
  await loadHistory();
  await fetchTree(props.sessionId);
  await fetchCheckpoints();
  await restoreInitialPrompt();
  nextTick(() => { messageListRef.value?.scrollToBottom(); });
});

watch(isConnected, async (connected) => { if (connected) await restoreInitialPrompt(); });

watch(activeLeafId, async (newLeaf, oldLeaf) => {
  if (newLeaf && newLeaf !== oldLeaf && !isStreaming.value) {
    await loadHistory(newLeaf);
    nextTick(() => { messageListRef.value?.scrollToMessageId(scrollTargetId.value); });
  }
});

watch(scrollTargetId, (id) => { if (id) { clearScrollTarget(); nextTick(() => { messageListRef.value?.scrollToMessageId(id); }); } });
</script>

<template>
  <div class="flex h-full overflow-hidden">
    <div class="flex flex-col flex-1 h-full overflow-hidden">
    <AgentHeader
      :title="sessionTitle"
      :work-dir="sessionWorkDir"
      :show-work-dir="false"
      @back="goBack()"
      @update:title="handleUpdateTitle"
    >
      <template #badges>
        <span class="env-tag" :class="local.sessionEnvironment.value">{{ local.sessionEnvironment.value === 'local' ? 'Local' : 'Remote' }}</span>
        <template v-if="isEditingWorkDir">
          <div class="workdir-edit">
            <FolderPicker v-model="editWorkDir" />
            <button class="title-action-btn confirm" @click="confirmEditWorkDir" aria-label="Confirm work dir">&#10003;</button>
            <button class="title-action-btn cancel" @click="cancelEditWorkDir" aria-label="Cancel edit">&#10005;</button>
          </div>
        </template>
        <span v-else-if="displayDir" class="context-dir" @click="startEditWorkDir">{{ displayDir }}</span>
        <span
          v-if="local.sessionEnvironment.value === 'local'"
          class="agent-status"
          :class="[local.localAgentStatus.value, { clickable: local.localAgentStatus.value === 'not_configured' }]"
          @click="local.localAgentStatus.value === 'not_configured' && navigateTo('agent-settings')"
        >
          {{ local.localAgentStatus.value === 'running' ? 'Running' : local.localAgentStatus.value === 'stopped' ? 'Stopped' : 'Not Configured' }}
        </span>
        <span v-if="actions.activeApplyChangeId.value" class="apply-indicator">Applying Change</span>
      </template>
      <template #actions>
        <button class="new-session-btn" :disabled="isCreatingSession" @click="startSessionFromCurrentDir" title="使用当前目录新建会话" aria-label="使用当前目录新建会话">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>
          <span>{{ isCreatingSession ? '创建中' : '新会话' }}</span>
        </button>
        <button v-if="local.sessionEnvironment.value === 'local'" class="settings-icon-btn" @click="navigateTo('agent-settings')" aria-label="Local Agent Settings">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
        </button>
      </template>
    </AgentHeader>

    <ArtifactReview v-if="reviewingChangeId" :change-id="reviewingChangeId" @confirmed="handleReviewConfirmed" @close="reviewingChangeId = null" />

    <MessageList
      v-if="!isEmpty"
      ref="messageListRef"
      :render-items="renderItems"
      :is-streaming="isStreaming"
      :blocks-length="blocks.length"
      :editing-message-id="editingMessageId"
      :editing-content="editingContent"
      :rewinding-to="rewindingTo"
      :get-branch-index="getBranchIndex"
      :get-branch-siblings="getBranchSiblings"
      :get-checkpoint-for-message="getCheckpointForMessage"
      :is-group-expanded="isGroupExpanded"
      @start-edit="startEditMessage"
      @cancel-edit="cancelEditMessage"
      @submit-edit="submitEditMessage"
      @update:editing-content="editingContent = $event"
      @switch-branch="switchToBranch"
      @rewind="handleRewind"
      @toggle-tool="toggleToolCall"
      @toggle-group="toggleGroup"
      @retry="actions.resend()"
      @navigate-settings="navigateTo('agent-settings')"
      @select-ask-option="actions.selectAskUserOption($event[0], $event[1], $event[2])"
      @start-ask-custom="actions.startCustomAskUser($event[0], $event[1])"
      @submit-ask="actions.submitAskUser($event)"
      @select-structured-option="actions.selectStructuredAskOption($event[0], $event[1], $event[2])"
      @start-structured-custom="actions.startStructuredAskCustom($event[0], $event[1])"
      @submit-structured="actions.submitStructuredAsk($event)"
    />
    <div v-else class="flex-1"></div>

    <AgentInput
      ref="agentInputRef"
      v-model="input"
      :is-streaming="isStreaming"
      :is-connected="isConnected"
      :is-empty="isEmpty"
      :provider-options="local.providerOptions.value"
      :selected-provider="local.selectedProvider.value"
      :show-image-upload="true"
      :disable-image-upload="local.sessionEnvironment.value === 'local' && local.selectedProviderSource.value === 'local'"
      @update:selected-provider="local.selectedProvider.value = $event"
      @send="actions.send()"
      @stop="actions.stop()"
      @images-change="handleImagesChange"
    />
    </div>

    <Teleport to="#shell-panel-content" v-if="activePanelId">
      <ChangeChatPanel
        v-if="activePanelId === 'changes' && sessionWorkDir"
        :work-dir="sessionWorkDir"
        :session-id="props.sessionId"
        :refresh-key="changesPanelRefreshKey"
        @navigate-session="handleNavigateSession"
        @apply-change="(id: string) => actions.handleApplyChange(id, closePanel, showWarning)"
        @review-change="handleReviewChange"
      />
      <SpecPanel v-if="activePanelId === 'specs'" />
      <ConversationOutline
        v-if="activePanelId === 'outline' && treeNodes.length > 0"
        :session-id="props.sessionId"
        :key="'outline-' + props.sessionId"
      />
    </Teleport>
  </div>
</template>

<style scoped>
.env-tag { font-size: 10px; padding: 1px 6px; border-radius: 3px; font-weight: 600; text-transform: uppercase; margin-left: 6px; }
.env-tag.local { color: var(--color-env-local); background: var(--color-env-local-bg); }
.env-tag.remote { color: var(--color-env-remote); background: var(--color-env-remote-bg); }
.context-dir { font-family: var(--font-mono); font-size: 12px; color: var(--color-text-muted); cursor: pointer; padding: 2px 6px; border-radius: 4px; transition: background 0.12s; }
.context-dir:hover { background: var(--color-surface-1); }
.workdir-edit { display: flex; align-items: center; gap: 6px; flex: 1; min-width: 0; }
.title-action-btn { background: none; border: none; font-size: 14px; cursor: pointer; padding: 2px 6px; border-radius: 4px; transition: color 0.12s; }
.title-action-btn.confirm { color: var(--color-success); }
.title-action-btn.cancel { color: var(--color-error); }
.title-action-btn:hover { opacity: 0.7; }
.apply-indicator { font-size: 11px; padding: 2px 8px; border-radius: var(--radius-sm); font-weight: 500; color: var(--color-info); background: var(--color-info-surface); }
.agent-status { font-size: 11px; padding: 2px 8px; border-radius: var(--radius-sm); font-weight: 500; }
.agent-status.running { color: var(--color-success); background: var(--color-success-surface); }
.agent-status.stopped { color: var(--color-error); background: var(--color-error-surface); }
.agent-status.not_configured { color: var(--color-text-muted); background: var(--color-surface-1); }
.agent-status.clickable { cursor: pointer; }
.agent-status.clickable:hover { color: var(--color-accent); text-decoration: underline; }
.new-session-btn { display: inline-flex; align-items: center; gap: 6px; height: 28px; padding: 4px 10px; border: 1px solid var(--color-border-subtle); border-radius: 5px; background: var(--color-surface-1); color: var(--color-text-secondary); font-size: 12px; font-weight: 500; line-height: 1; white-space: nowrap; cursor: pointer; transition: color 0.12s, border-color 0.12s, background 0.12s, opacity 0.12s; }
.new-session-btn:hover:not(:disabled) { color: var(--color-text-primary); border-color: var(--color-border); background: var(--color-surface-hover, var(--color-surface-2)); }
.new-session-btn:disabled { opacity: 0.55; cursor: not-allowed; }
.new-session-btn svg { flex-shrink: 0; }
.settings-icon-btn { background: none; border: none; color: var(--color-text-muted); cursor: pointer; padding: 4px; border-radius: 4px; display: flex; align-items: center; }
.settings-icon-btn:hover { color: var(--color-text-primary); background: var(--color-surface-1); }
</style>

