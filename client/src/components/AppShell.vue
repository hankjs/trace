<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from "vue";
import { useRoute } from "vue-router";
import { useSidebarPanels } from "../composables/useSidebarPanels";
import FloatingMenu from "./FloatingMenu.vue";

const route = useRoute();
const { panels: sidebarPanels, activePanelId, togglePanel, closePanel, reset: resetPanels } = useSidebarPanels();

// 路由切换时重置面板状态，由新页面重新注册
watch(() => route.fullPath, (_, oldPath) => {
  if (oldPath) resetPanels();
});

// 可调整面板宽度（百分比，相对于 content+panel 区域）
const panelWidthPercent = ref(50);
const isResizing = ref(false);

const lastPanelId = ref<string | null>(null);

// 终端是主模式：终端页自带 tab 栏悬浮菜单，隐藏全局顶栏
const isTerminalRoute = computed(() => route.name === "terminal");

const rightPanelOpen = computed(() => activePanelId.value !== null);

function handleKeydown(e: KeyboardEvent) {
  // 阻止 Backspace 键触发浏览器后退导航
  if (e.key === "Backspace" && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || (e.target as HTMLElement)?.isContentEditable)) {
    e.preventDefault();
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

defineExpose({ rightPanelOpen });
</script>

<template>
  <div class="shell" ref="shellEl" :class="{ resizing: isResizing }">
    <!-- 顶栏：左上角悬浮菜单（终端页隐藏，终端 tab 栏自带菜单） -->
    <header v-if="!isTerminalRoute" class="topbar">
      <FloatingMenu />
      <span class="topbar-brand">Trace</span>
    </header>

    <div class="shell-body">
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
    </div>
  </div>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}

/* Top Bar */
.topbar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-3);
  height: var(--header-height);
  flex-shrink: 0;
  background: var(--color-surface-1);
  border-bottom: 1px solid var(--color-border-subtle);
}

.topbar-brand {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.shell-body {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
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
</style>
