<script setup lang="ts">
import { ref, inject } from "vue";
import type { LayoutNode } from "../../terminal/layout";

const props = defineProps<{
  node: LayoutNode;
  activePaneId: string;
}>();

const emit = defineEmits<{
  focus: [id: string];
}>();

// 由 TerminalView provide：叶子容器 div 的注册/注销回调
const registerTermEl = inject<(id: string, el: unknown) => void>("registerTermEl");

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
      <PaneNode :node="node.a" :active-pane-id="activePaneId" @focus="emit('focus', $event)" />
    </div>
    <div
      class="divider"
      :class="`divider-${node.dir}`"
      @pointerdown="onDividerDown"
    ></div>
    <div class="split-child" :style="{ flex: `${1 - node.ratio} 1 0` }">
      <PaneNode :node="node.b" :active-pane-id="activePaneId" @focus="emit('focus', $event)" />
    </div>
  </div>

  <!-- term 叶子：xterm 容器 -->
  <div
    v-else
    class="pane"
    :class="{ active: node.id === activePaneId }"
    @mousedown="emit('focus', node.id)"
  >
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
  background: var(--color-border-subtle);
  transition: background var(--duration-fast);
  z-index: 5;
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
  padding: var(--space-1);
}

.term-container :deep(.xterm) {
  height: 100%;
}
</style>
