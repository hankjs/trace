<script setup lang="ts">
import { ref, inject } from "vue";
import type { LayoutNode, PaneDragState } from "../../terminal/layout";

interface PaneInfo {
  foreground_cmd: string;
  cwd: string;
  alive: boolean;
}

const props = defineProps<{
  node: LayoutNode;
  activePaneId: string;
}>();

const emit = defineEmits<{
  focus: [id: string];
  ctx: [payload: { id: string; x: number; y: number }];
}>();

// 由 TerminalView provide：叶子容器 div 的注册/注销回调
const registerTermEl = inject<(id: string, el: unknown) => void>("registerTermEl");
// 由 TerminalView provide：pane id -> 实时信息（5s 轮询更新）
const paneInfos = inject<Record<string, PaneInfo>>("paneInfos", {});
// 由 TerminalView provide：pane 自定义标题 + 重命名请求
const paneTitles = inject<Record<string, string>>("paneTitles", {});
const paneRename = inject<{ id: string | null }>("paneRename", { id: null });
// 由 TerminalView provide：拖拽移动 pane 的实时状态与回调
const paneDrag = inject<PaneDragState>("paneDrag", { dragId: null, targetId: null, zone: null });
const paneDragMove = inject<(x: number, y: number) => void>("paneDragMove", () => {});
const paneDragDrop = inject<(x: number, y: number) => void>("paneDragDrop", () => {});

const renameValue = ref("");

// v-focus：挂载即聚焦并全选
const vFocus = {
  mounted: (el: HTMLInputElement) => {
    el.focus();
    el.select();
  },
};

function startRename() {
  renameValue.value = paneTitles[node_id()] || paneInfos[node_id()]?.foreground_cmd || "";
  paneRename.id = node_id();
}

function node_id(): string {
  return props.node.kind === "term" ? props.node.id : "";
}

function commitRename() {
  const id = node_id();
  const v = renameValue.value.trim();
  if (v) paneTitles[id] = v;
  else delete paneTitles[id];
  paneRename.id = null;
}

function homeCwd(cwd: string): string {
  return cwd.replace(/^\/Users\/[^/]+/, "~");
}

const splitEl = ref<HTMLElement | null>(null);

function onDividerDown(e: PointerEvent) {
  if (props.node.kind !== "split") return;
  const el = splitEl.value;
  if (!el) return;
  e.preventDefault();
  const node = props.node;
  const rect = el.getBoundingClientRect();
  const onMove = (ev: PointerEvent) => {
    const r =
      node.dir === "row"
        ? (ev.clientX - rect.left) / rect.width
        : (ev.clientY - rect.top) / rect.height;
    node.ratio = Math.min(0.9, Math.max(0.1, r));
  };
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}

/** 拖标题栏移动 pane（iTerm2 风格）：超过阈值进入拖拽，落点由 TerminalView 命中计算 */
function onTitlebarDown(e: PointerEvent) {
  if (e.button !== 0) return;
  const id = node_id();
  if (!id || paneRename.id === id) return; // 重命名输入框交互优先
  const startX = e.clientX;
  const startY = e.clientY;
  let dragging = false;
  const onMove = (ev: PointerEvent) => {
    if (!dragging) {
      if (Math.abs(ev.clientX - startX) + Math.abs(ev.clientY - startY) < 6) return;
      dragging = true;
      paneDrag.dragId = id;
      document.body.style.cursor = "grabbing";
    }
    paneDragMove(ev.clientX, ev.clientY);
  };
  const onUp = (ev: PointerEvent) => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    if (dragging) {
      document.body.style.cursor = "";
      paneDragDrop(ev.clientX, ev.clientY);
    }
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}

/** 标题栏右侧设置按钮：点击效果等同右键该 pane（菜单定位在按钮下方） */
function openPaneMenu(e: MouseEvent) {
  const id = node_id();
  if (!id) return;
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
  emit("ctx", { id, x: rect.right, y: rect.bottom + 4 });
}
</script>

<template>
  <!-- split 节点：flex 二分 + 可拖拽分隔条 -->
  <div v-if="node.kind === 'split'" ref="splitEl" class="split" :class="`split-${node.dir}`">
    <div class="split-child" :style="{ flex: `${node.ratio} 1 0` }">
      <PaneNode :node="node.a" :active-pane-id="activePaneId" @focus="emit('focus', $event)" @ctx="emit('ctx', $event)" />
    </div>
    <div
      class="divider"
      :class="`divider-${node.dir}`"
      @pointerdown="onDividerDown"
      @dblclick="node.kind === 'split' && (node.ratio = 0.5)"
      title="拖拽调整比例，双击均分"
    ></div>
    <div class="split-child" :style="{ flex: `${1 - node.ratio} 1 0` }">
      <PaneNode :node="node.b" :active-pane-id="activePaneId" @focus="emit('focus', $event)" @ctx="emit('ctx', $event)" />
    </div>
  </div>

  <!-- term 叶子：标题栏 + xterm 容器 -->
  <div
    v-else
    class="pane"
    :class="{
      active: node.id === activePaneId,
      'drag-source': paneDrag.dragId === node.id,
      'drag-target': paneDrag.targetId === node.id && paneDrag.zone,
    }"
    @mousedown="emit('focus', node.id)"
    @contextmenu.stop.prevent="emit('ctx', { id: node.id, x: $event.clientX, y: $event.clientY })"
  >
    <div
      class="pane-titlebar"
      :class="{ draggable: paneRename.id !== node.id }"
      @dblclick.stop="startRename"
      @pointerdown="onTitlebarDown"
    >
      <span class="pane-title-dot" :class="{ alive: paneInfos[node.id]?.alive !== false }"></span>
      <input
        v-if="paneRename.id === node.id"
        v-model="renameValue"
        v-focus
        class="pane-title-rename"
        @keydown.enter="commitRename"
        @keydown.escape="paneRename.id = null"
        @blur="commitRename"
        @click.stop
      />
      <template v-else>
        <span class="pane-title-cmd">{{ paneTitles[node.id] || paneInfos[node.id]?.foreground_cmd || "shell" }}</span>
        <span class="pane-title-cwd">{{ homeCwd(paneInfos[node.id]?.cwd || "") }}</span>
      </template>
      <!-- 设置按钮：点击效果等同 pane 右键菜单 -->
      <button
        class="pane-menu-btn"
        @pointerdown.stop
        @click.stop="openPaneMenu"
        aria-label="面板菜单"
        title="面板菜单"
      >
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
          <circle cx="8" cy="8" r="2" stroke="currentColor" stroke-width="1.3"/>
          <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.5 1.5M11.5 11.5L13 13M13 3l-1.5 1.5M4.5 11.5L3 13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
        </svg>
      </button>
    </div>
    <div class="term-container" :ref="(el) => registerTermEl?.(node.id, el)"></div>
    <!-- 拖拽落点高亮（iTerm2 风格：边缘 = split，中央 = 交换） -->
    <div
      v-if="paneDrag.targetId === node.id && paneDrag.zone"
      class="drop-highlight"
      :class="`drop-${paneDrag.zone}`"
    ></div>
  </div>
</template>

<style scoped>
.split {
  display: flex;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
}

.split-row {
  flex-direction: row;
}

.split-col {
  flex-direction: column;
}

.split-child {
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.divider {
  flex-shrink: 0;
  position: relative;
  background: var(--color-border-subtle);
  transition: background var(--duration-fast);
  z-index: 5;
  /* 扩大可点热区（视觉保持 4px） */
}
.divider::before {
  content: "";
  position: absolute;
  inset: -3px;
}

.divider:hover {
  background: var(--color-accent);
}

.divider-row {
  width: 4px;
  cursor: col-resize;
}

.divider-col {
  height: 4px;
  cursor: row-resize;
}

.pane {
  position: relative;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  box-shadow: inset 0 0 0 1px transparent;
  transition: box-shadow var(--duration-fast);
}

.pane.active {
  box-shadow: inset 0 0 0 1px var(--color-accent);
}

.term-container {
  position: absolute;
  inset: 0;
  top: 24px; /* 标题栏高度 */
  padding: var(--space-1);
}

.pane-titlebar {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 var(--space-2);
  background: var(--color-surface-1);
  border-bottom: 1px solid var(--color-border-subtle);
  user-select: none;
}

.pane-titlebar.draggable {
  cursor: grab;
}

.pane.drag-source {
  opacity: 0.45;
}

.drop-highlight {
  position: absolute;
  inset: 0;
  z-index: 10;
  pointer-events: none;
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  box-shadow: inset 0 0 0 2px var(--color-accent);
}

/* 边缘落点：只高亮目标 pane 被 split 的那一半 */
.drop-left {
  right: 50%;
}

.drop-right {
  left: 50%;
}

.drop-top {
  bottom: 50%;
}

.drop-bottom {
  top: 50%;
}

.pane-title-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-text-muted);
  flex-shrink: 0;
}

.pane-title-dot.alive {
  background: var(--color-accent);
}

.pane-title-cmd {
  font-size: 11px;
  font-weight: 500;
  color: var(--color-text-secondary);
  white-space: nowrap;
}

.pane.active .pane-title-cmd {
  color: var(--color-text-primary);
}

.pane-title-cwd {
  font-size: 11px;
  color: var(--color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.pane-title-rename {
  flex: 1;
  min-width: 0;
  padding: 0 var(--space-1);
  font-size: 11px;
  font-family: inherit;
  color: var(--color-text-primary);
  background: var(--color-surface-0);
  border: 1px solid var(--color-accent);
  border-radius: var(--radius-sm);
  outline: none;
}

/* 标题栏右侧设置按钮：默认隐藏，悬停标题栏时显现（opacity 保持布局不抖动） */
.pane-menu-btn {
  margin-left: auto;
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
  flex-shrink: 0;
  opacity: 0;
  transition: opacity var(--duration-fast), color var(--duration-fast), background var(--duration-fast);
}

.pane-titlebar:hover .pane-menu-btn {
  opacity: 1;
}

.pane-menu-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.term-container :deep(.xterm) {
  height: 100%;
}
</style>
