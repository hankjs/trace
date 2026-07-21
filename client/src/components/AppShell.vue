<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from "vue";
import { useRouter, useRoute } from "vue-router";
import { useSession } from "../composables/useSession";
import { useSidebarPanels } from "../composables/useSidebarPanels";
import { useContextMenu, type ContextMenuItem } from "../composables/useContextMenu";
import ContextMenu from "./ContextMenu.vue";

const router = useRouter();
const route = useRoute();
const { sessions, fetchSessions, selectSession, deleteSession, updateSessionTitle, logout } = useSession();
const { panels: sidebarPanels, activePanelId, togglePanel, closePanel, reset: resetPanels } = useSidebarPanels();
const { visible: ctxVisible, position: ctxPosition, items: ctxItems, open: ctxOpen, close: ctxClose } = useContextMenu();

// 路由切换时重置面板状态，由新页面重新注册
watch(() => route.fullPath, (_, oldPath) => {
  if (oldPath) resetPanels();
});

const renamingSessionId = ref<string | null>(null);
const renameInput = ref("");
const pendingDeleteId = ref<string | null>(null);

// 可调整面板宽度（百分比，相对于 content+panel 区域）
const panelWidthPercent = ref(50);
const isResizing = ref(false);

function confirmDelete() {
  if (pendingDeleteId.value) {
    deleteSession(pendingDeleteId.value);
    pendingDeleteId.value = null;
  }
}

function cancelDelete() {
  pendingDeleteId.value = null;
}

function openSessionMenu(e: MouseEvent, session: { id: string; title: string; work_dir: string | null }) {
  const items: ContextMenuItem[] = [
    {
      label: "重命名",
      action: () => {
        renamingSessionId.value = session.id;
        renameInput.value = session.title || "";
      },
    },
    { label: "", action: () => {}, separator: true },
    {
      label: "删除",
      destructive: true,
      action: () => { pendingDeleteId.value = session.id; },
    },
  ];
  ctxOpen(e, items);
}

function confirmRename(sessionId: string) {
  if (renameInput.value.trim()) {
    updateSessionTitle(sessionId, renameInput.value.trim());
  }
  renamingSessionId.value = null;
}

function cancelRename() {
  renamingSessionId.value = null;
}

const navCollapsed = ref(false);
const lastPanelId = ref<string | null>(null);

const rightPanelOpen = computed(() => activePanelId.value !== null);

const activeSection = computed(() => {
  const name = route.name as string;
  if (name === "sessions" || name === "chat" || name === "agent") return "sessions";
  if (name === "specs") return "specs";
  if (name === "changes" || name === "change-detail") return "changes";
  if (name === "skills") return "skills";
  if (name === "agent-settings") return "settings";
  if (name === "debug-stream") return "debug";
  if (name === "image-gen") return "image-gen";
  if (name === "terminal") return "terminal";
  return "sessions";
});

function navigateTo(name: string) {
  router.push({ name });
}

function relativeTime(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins}分`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}时`;
  const days = Math.floor(hrs / 24);
  return `${days}天`;
}

function displayTitle(title: string, workDir: string | null): string {
  if (title) return title;
  if (workDir) return workDir.split("/").pop() || workDir;
  return "未命名";
}

function handleKeydown(e: KeyboardEvent) {
  // 阻止 Backspace 键触发浏览器后退导航
  if (e.key === "Backspace" && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || (e.target as HTMLElement)?.isContentEditable)) {
    e.preventDefault();
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "b" && !e.shiftKey) {
    e.preventDefault();
    navCollapsed.value = !navCollapsed.value;
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "b" && e.shiftKey) {
    e.preventDefault();
    if (activePanelId.value) {
      lastPanelId.value = activePanelId.value;
      closePanel();
    } else {
      const target = lastPanelId.value || sidebarPanels.value[0]?.id;
      if (target) togglePanel(target);
    }
  }
}

onMounted(() => {
  fetchSessions();
  document.addEventListener("keydown", handleKeydown);
});

onUnmounted(() => {
  document.removeEventListener("keydown", handleKeydown);
});

// Panel resize drag
const shellEl = ref<HTMLElement | null>(null);

function startResize(e: MouseEvent) {
  e.preventDefault();
  isResizing.value = true;
  document.addEventListener("mousemove", onResize);
  document.addEventListener("mouseup", stopResize);
}

function onResize(e: MouseEvent) {
  if (!shellEl.value) return;
  const shell = shellEl.value;
  const wrapper = shell.querySelector(".content-panel-area") as HTMLElement;
  if (!wrapper) return;
  const activityBar = shell.querySelector(".activity-bar") as HTMLElement;
  const actBarWidth = activityBar?.offsetWidth || 0;
  const availableWidth = wrapper.offsetWidth;
  if (availableWidth <= 0) return;
  const wrapperRight = wrapper.getBoundingClientRect().right - actBarWidth;
  const panelWidth = wrapperRight - e.clientX;
  const percent = Math.min(80, Math.max(20, (panelWidth / availableWidth) * 100));
  panelWidthPercent.value = percent;
}

function stopResize() {
  isResizing.value = false;
  document.removeEventListener("mousemove", onResize);
  document.removeEventListener("mouseup", stopResize);
}

const contentStyle = computed(() => {
  if (!rightPanelOpen.value) return {};
  return { flex: `0 0 ${100 - panelWidthPercent.value}%` };
});

const panelStyle = computed(() => {
  return { flex: `0 0 ${panelWidthPercent.value}%` };
});

defineExpose({ rightPanelOpen, navCollapsed });
</script>

<template>
  <div class="shell" ref="shellEl" :class="{ resizing: isResizing }">
    <!-- Left Navigation -->
    <nav class="nav" :class="{ collapsed: navCollapsed }">
      <div class="nav-header">
        <span v-if="!navCollapsed" class="nav-brand">Trace</span>
        <button
          class="nav-toggle"
          @click="navCollapsed = !navCollapsed"
          :aria-label="navCollapsed ? '展开导航' : '收起导航'"
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <path d="M3 4h10M3 8h10M3 12h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
        </button>
      </div>

      <div class="nav-sections">
        <!-- Sessions -->
        <div class="nav-section">
          <div v-if="!navCollapsed" class="nav-section-header-row">
            <button
              class="nav-section-header"
              :class="{ active: activeSection === 'sessions' }"
              @click="navigateTo('sessions')"
            >
              会话
            </button>
            <button
              class="nav-home-btn"
              @click="navigateTo('sessions')"
              title="回到会话列表"
            >
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                <path d="M3 8.5V13a1 1 0 001 1h3v-3.5h2V14h3a1 1 0 001-1V8.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
                <path d="M1.5 8L8 2l6.5 6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </button>
          </div>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'sessions' }"
            @click="navigateTo('sessions')"
            aria-label="会话"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M2 3h12v9H4l-2 2V3z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>
            </svg>
          </button>

          <!-- Session list (expanded only) -->
          <div v-if="!navCollapsed && activeSection === 'sessions'" class="nav-session-list">
            <div
              v-for="s in sessions.slice(0, 20)"
              :key="s.id"
              class="nav-session-item"
              :class="{ active: route.params.sessionId === s.id }"
              @click="selectSession(s)"
              @contextmenu="openSessionMenu($event, s)"
            >
              <template v-if="renamingSessionId === s.id">
                <input
                  class="nav-session-rename"
                  v-model="renameInput"
                  @click.stop
                  @keydown.enter="confirmRename(s.id)"
                  @keydown.escape="cancelRename"
                  @blur="confirmRename(s.id)"
                  ref="renameInputEl"
                  autofocus
                />
              </template>
              <template v-else>
                <span class="nav-session-title">{{ displayTitle(s.title, s.work_dir) }}</span>
                <span class="nav-session-env" :class="s.environment">{{ s.environment === 'local' ? '本地' : '线上' }}</span>
                <span class="nav-session-time">{{ relativeTime(s.updated_at) }}</span>
                <button
                  class="nav-session-delete"
                  @click.stop="pendingDeleteId = s.id"
                  aria-label="删除会话"
                >
                  <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                    <path d="M4.5 3V2.5a1.5 1.5 0 0 1 1.5-1.5h4a1.5 1.5 0 0 1 1.5 1.5V3M2.5 3h11M3.5 3v10a1.5 1.5 0 0 0 1.5 1.5h6a1.5 1.5 0 0 0 1.5-1.5V3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
              </template>
            </div>
          </div>
        </div>

        <!-- Specs -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'specs' }"
            @click="navigateTo('specs')"
          >
            规格
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'specs' }"
            @click="navigateTo('specs')"
            aria-label="规格"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M4 2h8v12H4V2z" stroke="currentColor" stroke-width="1.3"/>
              <path d="M6 5h4M6 7.5h4M6 10h2" stroke="currentColor" stroke-width="1.1" stroke-linecap="round"/>
            </svg>
          </button>
        </div>

        <!-- Changes -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'changes' }"
            @click="navigateTo('changes')"
          >
            变更
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'changes' }"
            @click="navigateTo('changes')"
            aria-label="变更"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M8 2v12M2 8h12" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
            </svg>
          </button>
        </div>

        <!-- Skills -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'skills' }"
            @click="navigateTo('skills')"
          >
            Skills
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'skills' }"
            @click="navigateTo('skills')"
            aria-label="Skills"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <circle cx="8" cy="8" r="5.5" stroke="currentColor" stroke-width="1.3"/>
              <path d="M8 5v3l2 1.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>

        <!-- AI生图 -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'image-gen' }"
            @click="navigateTo('image-gen')"
          >
            AI生图
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'image-gen' }"
            @click="navigateTo('image-gen')"
            aria-label="AI生图"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="2" y="2" width="12" height="12" rx="2" stroke="currentColor" stroke-width="1.3"/>
              <circle cx="5.5" cy="5.5" r="1.2" stroke="currentColor" stroke-width="1.1"/>
              <path d="M2 10.5l3.5-3.5 2.5 2.5 2-2 3.5 3.5" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>

        <!-- 终端 -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'terminal' }"
            @click="navigateTo('terminal')"
          >
            终端
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'terminal' }"
            @click="navigateTo('terminal')"
            aria-label="终端"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
              <path d="M4.5 6l2.5 2.5L4.5 11M8 11h3.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>

        <!-- Debug -->
        <div class="nav-section">
          <button
            v-if="!navCollapsed"
            class="nav-section-header"
            :class="{ active: activeSection === 'debug' }"
            @click="navigateTo('debug-stream')"
          >
            Debug
          </button>
          <button
            v-else
            class="nav-icon-btn"
            :class="{ active: activeSection === 'debug' }"
            @click="navigateTo('debug-stream')"
            aria-label="Debug"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M5 3l6 5-6 5V3z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>
      </div>

      <!-- Bottom: Settings -->
      <div class="nav-footer">
        <button
          v-if="!navCollapsed"
          class="nav-section-header"
          :class="{ active: activeSection === 'settings' }"
          @click="navigateTo('agent-settings')"
        >
          设置
        </button>
        <button
          v-else
          class="nav-icon-btn"
          :class="{ active: activeSection === 'settings' }"
          @click="navigateTo('agent-settings')"
          aria-label="设置"
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <circle cx="8" cy="8" r="2" stroke="currentColor" stroke-width="1.3"/>
            <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.5 1.5M11.5 11.5L13 13M13 3l-1.5 1.5M4.5 11.5L3 13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </nav>

    <!-- Content + Panel wrapper (flex percentages apply within this area) -->
    <div class="content-panel-area">
      <main class="content" :style="contentStyle">
        <router-view v-slot="{ Component, route }">
          <component :is="Component" :key="route.fullPath" />
        </router-view>
      </main>

      <!-- Resize Handle -->
      <div v-if="rightPanelOpen" class="panel-resize-handle" @mousedown="startResize"></div>

      <!-- Right Panel (driven by useSidebarPanels) -->
      <aside v-if="rightPanelOpen" class="panel" :style="panelStyle">
        <div class="panel-header">
          <span class="panel-title">{{ sidebarPanels.find(p => p.id === activePanelId)?.title }}</span>
          <button class="panel-close" @click="closePanel()" aria-label="关闭面板">
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <path d="M3 3l8 8M11 3l-8 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
            </svg>
          </button>
        </div>
        <div class="panel-content" id="shell-panel-content"></div>
      </aside>
    </div>

    <!-- Activity Bar -->
    <div v-if="sidebarPanels.length > 0" class="activity-bar">
      <button
        v-for="panel in sidebarPanels"
        :key="panel.id"
        class="activity-bar-btn"
        :class="{ active: activePanelId === panel.id }"
        @click="togglePanel(panel.id)"
        :aria-label="panel.title"
        :title="panel.title"
      >
        <svg v-if="panel.icon === 'changes'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>
        </svg>
        <svg v-else-if="panel.icon === 'specs'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="9" y1="21" x2="9" y2="9"/>
        </svg>
        <svg v-else-if="panel.icon === 'outline'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/>
          <line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/>
        </svg>
        <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>
        </svg>
      </button>
    </div>

    <ContextMenu :visible="ctxVisible" :position="ctxPosition" :items="ctxItems" @close="ctxClose" />

    <!-- Delete confirmation -->
    <Teleport to="body">
      <div v-if="pendingDeleteId" class="confirm-backdrop" @mousedown.self="cancelDelete">
        <div class="confirm-dialog" @keydown.escape="cancelDelete">
          <p class="confirm-text">确定删除此会话？</p>
          <div class="confirm-actions">
            <button class="confirm-btn cancel" @click="cancelDelete">取消</button>
            <button class="confirm-btn destructive" @click="confirmDelete" autofocus>删除</button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

/* Left Navigation */
.nav {
  width: var(--nav-width);
  min-width: var(--nav-width);
  display: flex;
  flex-direction: column;
  background: var(--color-surface-1);
  border-right: 1px solid var(--color-border-subtle);
  transition: width var(--duration-normal) var(--ease-out-expo),
              min-width var(--duration-normal) var(--ease-out-expo);
  overflow: hidden;
}

.nav.collapsed {
  width: var(--nav-collapsed);
  min-width: var(--nav-collapsed);
}

.nav-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-3) var(--space-3);
  height: var(--header-height);
  flex-shrink: 0;
}

.nav-brand {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
  padding-left: var(--space-2);
}

.nav-toggle {
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: var(--space-1);
  border-radius: var(--radius-sm);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.nav-toggle:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.nav-sections {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-2) var(--space-2);
}

.nav-section {
  margin-bottom: var(--space-1);
}

.nav-section-header-row {
  display: flex;
  align-items: center;
  gap: 2px;
}
.nav-section-header-row .nav-section-header {
  flex: 1;
}
.nav-home-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  transition: color var(--duration-fast), background var(--duration-fast);
}
.nav-home-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.nav-section-header {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  padding: var(--space-2) var(--space-2);
  font-size: 12px;
  font-weight: 500;
  color: var(--color-text-secondary);
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: color var(--duration-fast), background var(--duration-fast);
}

.nav-section-header:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.nav-section-header.active {
  color: var(--color-accent);
}

.nav-icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  margin: 0 auto var(--space-1);
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: color var(--duration-fast), background var(--duration-fast);
}

.nav-icon-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.nav-icon-btn.active {
  color: var(--color-accent);
}

/* Session list in nav */
.nav-session-list {
  margin-top: var(--space-1);
}

.nav-session-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-1) var(--space-2);
  margin-left: var(--space-2);
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: background var(--duration-fast);
}

.nav-session-item:hover {
  background: var(--color-surface-hover);
}

.nav-session-item.active {
  background: var(--color-surface-2);
}

.nav-session-title {
  font-size: 12px;
  color: var(--color-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}

.nav-session-item.active .nav-session-title {
  color: var(--color-text-primary);
}

.nav-session-env {
  font-size: 10px;
  font-weight: 500;
  padding: 0 5px;
  border-radius: 3px;
  flex-shrink: 0;
  margin-left: var(--space-1);
  line-height: 16px;
}

.nav-session-env.remote {
  color: var(--color-env-remote);
  background: var(--color-env-remote-bg);
}

.nav-session-env.local {
  color: var(--color-env-local);
  background: var(--color-env-local-bg);
}

.nav-session-time {
  font-size: 10px;
  color: var(--color-text-muted);
  flex-shrink: 0;
  margin-left: var(--space-2);
}

.nav-session-delete {
  display: none;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  margin-left: var(--space-1);
  background: none;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 0;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.nav-session-item:hover .nav-session-delete {
  display: flex;
}

.nav-session-item:hover .nav-session-time {
  display: none;
}

.nav-session-delete:hover {
  color: var(--color-error);
  background: oklch(0.65 0.18 25 / 0.12);
}

.nav-session-rename {
  width: 100%;
  background: var(--color-surface-0);
  border: 1px solid var(--color-accent);
  border-radius: var(--radius-sm);
  padding: 1px var(--space-1);
  font-size: 12px;
  color: var(--color-text-primary);
  outline: none;
  font-family: inherit;
}

.nav-footer {
  padding: var(--space-2);
  border-top: 1px solid var(--color-border-subtle);
}

/* Center Content */
.content-panel-area {
  flex: 1;
  min-width: 0;
  display: flex;
  overflow: hidden;
}

.content {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

/* Resize Handle */
.panel-resize-handle {
  width: 4px;
  cursor: col-resize;
  background: transparent;
  transition: background var(--duration-fast);
  flex-shrink: 0;
  position: relative;
  z-index: 10;
}
.panel-resize-handle:hover,
.shell.resizing .panel-resize-handle {
  background: var(--color-accent);
}

/* Right Panel */
.panel {
  min-width: 200px;
  background: var(--color-surface-1);
  border-left: 1px solid var(--color-border-subtle);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.shell.resizing { cursor: col-resize; user-select: none; }

.panel-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-2) var(--space-3);
  height: var(--header-height);
  flex-shrink: 0;
  border-bottom: 1px solid var(--color-border-subtle);
}

.panel-close {
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: var(--space-1);
  border-radius: var(--radius-sm);
  display: flex;
  align-items: center;
  transition: color var(--duration-fast);
}

.panel-close:hover {
  color: var(--color-text-primary);
}

.panel-content {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  min-width: 0;
  padding: 0 var(--space-3) var(--space-3);
}

.panel-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

/* Activity Bar */
.activity-bar {
  width: 40px;
  min-width: 40px;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding-top: var(--space-2);
  gap: var(--space-1);
  background: var(--color-surface-0);
  border-left: 1px solid var(--color-border-subtle);
}

.activity-bar-btn {
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  cursor: pointer;
  border-radius: var(--radius-sm);
  position: relative;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.activity-bar-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.activity-bar-btn.active {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.activity-bar-btn.active::before {
  content: '';
  position: absolute;
  left: -4px;
  top: 6px;
  bottom: 6px;
  width: 2px;
  background: var(--color-accent);
  border-radius: 1px;
}

/* Delete confirmation dialog */
.confirm-backdrop {
  position: fixed;
  inset: 0;
  z-index: 10000;
  background: oklch(0 0 0 / 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
}

.confirm-dialog {
  background: var(--color-surface-2);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  padding: var(--space-4) var(--space-5);
  min-width: 240px;
}

.confirm-text {
  font-size: 13px;
  color: var(--color-text-primary);
  margin: 0 0 var(--space-4);
}

.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
}

.confirm-btn {
  padding: var(--space-1) var(--space-3);
  font-size: 12px;
  font-weight: 500;
  border-radius: var(--radius-sm);
  border: none;
  cursor: pointer;
  transition: background var(--duration-fast);
}

.confirm-btn.cancel {
  background: var(--color-surface-3);
  color: var(--color-text-secondary);
}

.confirm-btn.cancel:hover {
  background: var(--color-surface-hover);
  color: var(--color-text-primary);
}

.confirm-btn.destructive {
  background: var(--color-error);
  color: var(--color-surface-0);
}

.confirm-btn.destructive:hover {
  background: oklch(0.60 0.18 25);
}
</style>
