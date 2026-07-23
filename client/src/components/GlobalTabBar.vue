<script setup lang="ts">
/**
 * 全局 tab 栏：页面 tab（会话/规格/…）与终端 tab 混排共用。
 * 左上角「+」按钮弹出页面类型选择，选中即新开对应 tab。
 */
import { ref, computed, watch, nextTick, onMounted, onUnmounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import ContextMenu from "./ContextMenu.vue";
import { useContextMenu, type ContextMenuItem } from "../composables/useContextMenu";
import {
  sortedTermTabs,
  activeTermTabId,
  termActions,
  ensureTitlePolling,
  type TermTab,
} from "../terminal/termTabs";

const route = useRoute();
const router = useRouter();

// ---------- 页面 tab ----------

const PAGE_LABELS: Record<string, string> = {
  sessions: "会话",
  specs: "规格",
  changes: "变更",
  skills: "Skills",
  "image-gen": "AI生图",
  "debug-stream": "Debug",
  "agent-settings": "设置",
};

/** 「+」可选的页面类型：终端 + 各页面 */
const newTabItems: { label: string; name: string }[] = [
  { label: "终端", name: "terminal" },
  ...Object.entries(PAGE_LABELS).map(([name, label]) => ({ label, name })),
];

/** 已打开的页面 tab（按打开顺序，单例） */
const pageTabs = ref<string[]>([]);

/** 子页面归属：chat/agent 属「会话」，change-detail 属「变更」 */
function pageOfRoute(name: string): string | null {
  if (name === "chat" || name === "agent") return "sessions";
  if (name === "change-detail") return "changes";
  return PAGE_LABELS[name] ? name : null;
}

const activePage = computed(() => pageOfRoute(route.name as string));

// 路由进入某页面（含内部跳转，如会话列表 → chat）时自动登记对应 tab
watch(
  () => route.name,
  (name) => {
    const page = pageOfRoute(name as string);
    if (page && !pageTabs.value.includes(page)) pageTabs.value.push(page);
  },
  { immediate: true },
);

function openPageTab(name: string) {
  if (!pageTabs.value.includes(name)) pageTabs.value.push(name);
  router.push({ name });
}

function closePageTab(name: string) {
  const idx = pageTabs.value.indexOf(name);
  if (idx < 0) return;
  pageTabs.value.splice(idx, 1);
  if (activePage.value === name) {
    const fallback = pageTabs.value[Math.min(idx, pageTabs.value.length - 1)];
    router.push({ name: fallback || "terminal" });
  }
}

// ---------- 终端 tab ----------

function openTerminalTab(id: string) {
  termActions.activate?.(id);
  if (route.name !== "terminal") router.push({ name: "terminal" });
}

function closeTerminalTab(id: string) {
  termActions.close?.(id);
}

function isTermTabActive(id: string): boolean {
  return route.name === "terminal" && activeTermTabId.value === id;
}

// ---------- 「+」新增 tab 选择器 ----------

const pickerOpen = ref(false);

function togglePicker() {
  pickerOpen.value = !pickerOpen.value;
}

async function onPick(name: string) {
  pickerOpen.value = false;
  if (name === "terminal") {
    // termActions.create 由 TerminalView 注入；尚未挂载过时直接跳终端页（挂载后自动恢复/新建）
    if (termActions.create) await termActions.create();
    if (route.name !== "terminal") router.push({ name: "terminal" });
  } else {
    openPageTab(name);
  }
}

function onGlobalPointerDown(e: PointerEvent) {
  if (!(e.target instanceof HTMLElement)) return;
  if (!e.target.closest(".new-tab-wrap")) pickerOpen.value = false;
}

// ---------- tab 重命名（仅终端 tab） ----------

const renamingTabId = ref<string | null>(null);
const renameValue = ref("");
const renameInputEl = ref<HTMLInputElement | null>(null);

function startRenameTab(tab: TermTab) {
  renamingTabId.value = tab.id;
  renameValue.value = tab.customTitle || tab.title;
  nextTick(() => {
    renameInputEl.value?.focus();
    renameInputEl.value?.select();
  });
}

function commitRenameTab() {
  const tab = sortedTermTabs.value.find((t) => t.id === renamingTabId.value);
  if (tab) {
    const v = renameValue.value.trim();
    tab.customTitle = v || undefined;
    if (v) tab.title = v;
  }
  renamingTabId.value = null;
}

// ---------- 右键菜单 ----------

const { visible: ctxVisible, position: ctxPosition, items: ctxItems, open: ctxOpen, close: ctxClose } = useContextMenu();

function onTermTabContextMenu(e: MouseEvent, tab: TermTab) {
  const items: ContextMenuItem[] = [
    { label: "重命名", action: () => startRenameTab(tab) },
    {
      label: tab.pinned ? "取消固定" : "固定",
      action: () => {
        tab.pinned = !tab.pinned;
      },
    },
    { label: "", action: () => {}, separator: true },
    { label: "关闭", destructive: true, action: () => closeTerminalTab(tab.id) },
  ];
  ctxOpen(e, items);
}

function onPageTabContextMenu(e: MouseEvent, name: string) {
  ctxOpen(e, [{ label: "关闭", destructive: true, action: () => closePageTab(name) }]);
}

function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") pickerOpen.value = false;
}

onMounted(() => {
  ensureTitlePolling();
  document.addEventListener("pointerdown", onGlobalPointerDown);
  document.addEventListener("keydown", onGlobalKeydown);
});

onUnmounted(() => {
  document.removeEventListener("pointerdown", onGlobalPointerDown);
  document.removeEventListener("keydown", onGlobalKeydown);
});
</script>

<template>
  <div class="global-tab-bar">
    <!-- 新增 tab（弹出页面类型选择） -->
    <div class="new-tab-wrap">
      <button
        class="bar-btn"
        :class="{ open: pickerOpen }"
        @click="togglePicker"
        aria-label="新建标签页"
        title="新建标签页"
      >
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path d="M7 2v10M2 7h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
        </svg>
      </button>
      <div v-if="pickerOpen" class="new-tab-dropdown">
        <button
          v-for="item in newTabItems"
          :key="item.name"
          class="new-tab-item"
          @click="onPick(item.name)"
        >
          {{ item.label }}
        </button>
      </div>
    </div>

    <div class="tabs-scroll">
      <!-- 页面 tab -->
      <div
        v-for="name in pageTabs"
        :key="`page-${name}`"
        class="tab"
        :class="{ active: activePage === name }"
        @click="openPageTab(name)"
        @contextmenu="onPageTabContextMenu($event, name)"
      >
        <span class="tab-title">{{ PAGE_LABELS[name] }}</span>
        <button class="tab-close" @click.stop="closePageTab(name)" aria-label="关闭标签页">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </button>
      </div>

      <!-- 终端 tab -->
      <div
        v-for="tab in sortedTermTabs"
        :key="tab.id"
        class="tab"
        :class="{ active: isTermTabActive(tab.id), dead: !tab.alive, pinned: tab.pinned }"
        @click="openTerminalTab(tab.id)"
        @dblclick="startRenameTab(tab)"
        @contextmenu="onTermTabContextMenu($event, tab)"
      >
        <span class="tab-dot" :class="{ alive: tab.alive }"></span>
        <svg v-if="tab.pinned" class="tab-pin" width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
          <path d="M9.5 1.5l5 5-1.8 1.8-1-.4L9 10.6V13l-1.5 1.5L5 12l-2.5 2.5-1-1L4 11 1.5 8.5 3 7h2.4l2.7-2.7-.4-1 1.8-1.8z"/>
        </svg>
        <input
          v-if="renamingTabId === tab.id"
          ref="renameInputEl"
          v-model="renameValue"
          class="tab-rename"
          @click.stop
          @dblclick.stop
          @keydown.enter="commitRenameTab"
          @keydown.escape="renamingTabId = null"
          @blur="commitRenameTab"
        />
        <span v-else class="tab-title">{{ tab.title }}</span>
        <button class="tab-close" @click.stop="closeTerminalTab(tab.id)" aria-label="关闭终端">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </div>

    <ContextMenu :visible="ctxVisible" :position="ctxPosition" :items="ctxItems" @close="ctxClose" />
  </div>
</template>

<style scoped>
.global-tab-bar {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: var(--space-1) var(--space-2);
  height: var(--header-height);
  flex-shrink: 0;
  background: var(--color-surface-1);
  border-bottom: 1px solid var(--color-border-subtle);
}

/* 新增 tab 选择器（dropdown 不能被裁剪，故滚动只作用于 .tabs-scroll） */
.new-tab-wrap {
  position: relative;
  flex-shrink: 0;
  margin-right: var(--space-1);
}

.bar-btn {
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

.bar-btn:hover,
.bar-btn.open {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.new-tab-dropdown {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  z-index: 100;
  min-width: 120px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  background: var(--color-surface-2);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md, 8px);
  box-shadow: 0 8px 24px oklch(0 0 0 / 0.45);
}

.new-tab-item {
  display: block;
  width: 100%;
  text-align: left;
  padding: var(--space-1) var(--space-2);
  font-size: 12px;
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.new-tab-item:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

/* tab 列表单独滚动：若 overflow 放在 .global-tab-bar 上，新增下拉会被裁剪 */
.tabs-scroll {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 2px;
  overflow-x: auto;
}

.tab {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: 3px var(--space-2);
  font-size: 12px;
  color: var(--color-text-muted);
  border-radius: var(--radius-sm);
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
  flex-shrink: 0;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.tab:hover {
  color: var(--color-text-secondary);
  background: var(--color-surface-hover);
}

.tab.active {
  color: var(--color-text-primary);
  background: var(--color-surface-2);
}

.tab.dead .tab-title {
  opacity: 0.5;
}

.tab-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-text-muted);
  flex-shrink: 0;
}

.tab-dot.alive {
  background: var(--color-accent);
}

.tab-title {
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tab-pin {
  color: var(--color-accent);
  flex-shrink: 0;
}

.tab-rename {
  width: 110px;
  padding: 0 var(--space-1);
  font-size: 12px;
  font-family: inherit;
  color: var(--color-text-primary);
  background: var(--color-surface-0);
  border: 1px solid var(--color-accent);
  border-radius: var(--radius-sm);
  outline: none;
}

.tab-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 0;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.tab-close:hover {
  color: var(--color-error);
  background: var(--color-surface-hover);
}
</style>
