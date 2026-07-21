<script setup lang="ts">
import { ref, inject } from "vue";
import type { LayoutNode } from "../../terminal/layout";

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
    :class="{ active: node.id === activePaneId }"
    @mousedown="emit('focus', node.id)"
    @contextmenu.stop.prevent="emit('ctx', { id: node.id, x: $event.clientX, y: $event.clientY })"
  >
    <div class="pane-titlebar" @dblclick.stop="startRename">
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
    </div>
    <div class="term-container" :ref="(el) => registerTermEl?.(node.id, el)"></div>
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

.term-container :deep(.xterm) {
  height: 100%;
}
</style>
