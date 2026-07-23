<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from "vue";
import { useRouter, useRoute } from "vue-router";

const router = useRouter();
const route = useRoute();

const menuItems: { label: string; name: string }[] = [
  { label: "会话", name: "sessions" },
  { label: "规格", name: "specs" },
  { label: "变更", name: "changes" },
  { label: "Skills", name: "skills" },
  { label: "AI生图", name: "image-gen" },
  { label: "终端", name: "terminal" },
  { label: "Debug", name: "debug-stream" },
  { label: "设置", name: "agent-settings" },
];

const open = ref(false);

// 高亮当前路由所属菜单项
const activeName = computed(() => {
  const name = route.name as string;
  if (name === "sessions" || name === "chat" || name === "agent") return "sessions";
  if (name === "change-detail") return "changes";
  return name;
});

function toggle() {
  open.value = !open.value;
}

function close() {
  open.value = false;
}

function go(name: string) {
  open.value = false;
  router.push({ name });
}

function onGlobalPointerDown(e: PointerEvent) {
  if (!(e.target instanceof HTMLElement)) return;
  if (!e.target.closest(".floating-menu")) close();
}

function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") close();
}

onMounted(() => {
  document.addEventListener("pointerdown", onGlobalPointerDown);
  document.addEventListener("keydown", onGlobalKeydown);
});

onUnmounted(() => {
  document.removeEventListener("pointerdown", onGlobalPointerDown);
  document.removeEventListener("keydown", onGlobalKeydown);
});
</script>

<template>
  <div class="floating-menu">
    <button
      class="fm-btn"
      :class="{ open }"
      @click="toggle"
      aria-label="菜单"
      title="菜单"
    >
      <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
        <path d="M2 3.5h10M2 7h10M2 10.5h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
      </svg>
    </button>
    <div v-if="open" class="fm-dropdown">
      <button
        v-for="item in menuItems"
        :key="item.name"
        class="fm-item"
        :class="{ active: activeName === item.name }"
        @click="go(item.name)"
      >
        {{ item.label }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.floating-menu {
  position: relative;
  flex-shrink: 0;
}

.fm-btn {
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

.fm-btn:hover,
.fm-btn.open {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.fm-dropdown {
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

.fm-item {
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

.fm-item:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.fm-item.active {
  color: var(--color-accent);
}
</style>
