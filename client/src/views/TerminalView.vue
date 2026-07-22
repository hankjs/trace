<script setup lang="ts">
import { ref, reactive, computed, onMounted, onUnmounted, nextTick, provide } from "vue";
import { useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { CanvasAddon } from "@xterm/addon-canvas";
import "@xterm/xterm/css/xterm.css";
import PaneNode from "../components/terminal/PaneNode.vue";
import ContextMenu from "../components/ContextMenu.vue";
import { useContextMenu, type ContextMenuItem } from "../composables/useContextMenu";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { registerTerm, unregisterTerm } from "../terminal/screenRegistry";
import {
  type LayoutNode,
  createLeaf,
  splitNode,
  removeNode,
  firstLeaf,
  allLeaves,
} from "../terminal/layout";

interface TermInfo {
  id: string;
  shell: string;
  cwd: string;
  foreground_cmd: string;
  alive: boolean;
  created_at: string;
}

interface Tab {
  id: string;
  title: string;
  /** 用户重命名后的固定标题（不再被前台进程名覆盖） */
  customTitle?: string;
  pinned?: boolean;
  alive: boolean;
  root: LayoutNode;
  activePaneId: string;
}

interface TermInstance {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  unlisten: UnlistenFn;
  observer: ResizeObserver | null;
}

const tabs = ref<Tab[]>([]);
const activeTabId = ref("");
const containers = new Map<string, HTMLElement>();
const instances = new Map<string, TermInstance>();
/** 每个 pane 的启动 cwd，用于分屏时继承 */
const paneCwds = new Map<string, string>();
/** pane id -> 实时信息（标题栏显示用，5s 轮询更新） */
const paneInfos = reactive<Record<string, { foreground_cmd: string; cwd: string; alive: boolean }>>({});
provide("paneInfos", paneInfos);
/** pane 自定义标题 + 进行中的重命名请求（PaneNode 双击标题栏或右键菜单触发） */
const paneTitles = reactive<Record<string, string>>({});
const paneRename = reactive<{ id: string | null }>({ id: null });
provide("paneTitles", paneTitles);
provide("paneRename", paneRename);
let pollTimer: ReturnType<typeof setInterval> | null = null;
let unlistenFocus: (() => void) | null = null;
let unlistenDrag: (() => void) | null = null;

// 搜索框状态
const searchOpen = ref(false);
const searchQuery = ref("");
const searchInputEl = ref<HTMLInputElement | null>(null);

provide("registerTermEl", (id: string, el: unknown) => {
  if (el instanceof HTMLElement) {
    const prev = containers.get(id);
    containers.set(id, el);
    // 树结构变化（分屏/收缩）会导致叶子容器 div 被 Vue 重建，
    // 已存在的 xterm 实例需要把 DOM 搬到新容器里
    if (prev !== el) reattachInstance(id, el);
  } else {
    containers.delete(id);
  }
});

/** 实例已存在但容器 div 被重建时，把 xterm DOM 移入新容器并重新观察尺寸 */
function reattachInstance(id: string, el: HTMLElement) {
  const inst = instances.get(id);
  if (!inst) return;
  nextTick(() => {
    try {
      const termEl = inst.term.element;
      if (termEl && termEl.parentElement !== el) {
        el.appendChild(termEl);
      }
      inst.observer?.disconnect();
      const observer = new ResizeObserver(() => {
        if (el.clientWidth === 0 || el.clientHeight === 0) return;
        inst.fit.fit();
        invoke("term_resize", { id, cols: inst.term.cols, rows: inst.term.rows }).catch(() => {});
      });
      observer.observe(el);
      inst.observer = observer;
      inst.fit.fit();
    } catch (e) {
      // 搬家失败（如 canvas 状态损坏）：销毁重建 + 回放 scrollback 兜底
      console.warn("[Terminal] reattach failed, recreating:", e);
      disposeInstance(id);
      attachWithReplay(id);
    }
  });
}

/** 从 PTY scrollback 回放重建一个 pane（reattach 兜底） */
async function attachWithReplay(id: string) {
  let replay = "";
  try {
    replay = await invoke<string>("term_read", { id, maxBytes: null });
  } catch {
    /* 读取失败则空白重建 */
  }
  await attachInstance(id, replay);
}

function basename(p: string): string {
  return p.split("/").filter(Boolean).pop() || p;
}

function activeTab(): Tab | undefined {
  return tabs.value.find((t) => t.id === activeTabId.value);
}

// ---------- 悬浮菜单（终端为主模式，替代左侧导航） ----------

const router = useRouter();
const menuOpen = ref(false);
const menuItems: { label: string; name: string }[] = [
  { label: "会话", name: "sessions" },
  { label: "规格", name: "specs" },
  { label: "变更", name: "changes" },
  { label: "Skills", name: "skills" },
  { label: "AI生图", name: "image-gen" },
  { label: "Debug", name: "debug-stream" },
  { label: "设置", name: "agent-settings" },
];

function toggleMenu() {
  menuOpen.value = !menuOpen.value;
}

function closeMenu() {
  menuOpen.value = false;
}

function goMenuItem(name: string) {
  menuOpen.value = false;
  router.push({ name });
}

function onGlobalPointerDown(e: PointerEvent) {
  if (!(e.target instanceof HTMLElement)) return;
  if (!e.target.closest(".menu-wrap")) closeMenu();
}

function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") closeMenu();
}

// ---------- 右键菜单（tab / pane） ----------

const {
  visible: ctxVisible,
  position: ctxPosition,
  items: ctxItems,
  open: ctxOpen,
  close: ctxClose,
} = useContextMenu();

/** pin 的 tab 排在前面（稳定排序） */
const sortedTabs = computed(() => {
  return tabs.value
    .map((t, i) => ({ t, i }))
    .sort((a, b) => Number(b.t.pinned ?? false) - Number(a.t.pinned ?? false) || a.i - b.i)
    .map(({ t }) => t);
});

// tab 重命名状态
const renamingTabId = ref<string | null>(null);
const renameValue = ref("");
const renameInputEl = ref<HTMLInputElement | null>(null);

function startRenameTab(tab: Tab) {
  renamingTabId.value = tab.id;
  renameValue.value = tab.customTitle || tab.title;
  nextTick(() => {
    renameInputEl.value?.focus();
    renameInputEl.value?.select();
  });
}

function commitRenameTab() {
  const tab = tabs.value.find((t) => t.id === renamingTabId.value);
  if (tab) {
    const v = renameValue.value.trim();
    tab.customTitle = v || undefined;
    if (v) tab.title = v;
  }
  renamingTabId.value = null;
}

function onTabContextMenu(e: MouseEvent, tab: Tab) {
  const items: ContextMenuItem[] = [
    { label: "重命名", action: () => startRenameTab(tab) },
    {
      label: tab.pinned ? "取消固定" : "固定",
      action: () => {
        tab.pinned = !tab.pinned;
      },
    },
    { label: "", action: () => {}, separator: true },
    { label: "关闭", destructive: true, action: () => closeTab(tab.id) },
  ];
  ctxOpen(e, items);
}

function onPaneContextMenu(tab: Tab, payload: { id: string; x: number; y: number }) {
  tab.activePaneId = payload.id;
  instances.get(payload.id)?.term.focus();
  const items: ContextMenuItem[] = [
    { label: "重命名", action: () => { paneRename.id = payload.id; } },
    { label: "向右分屏", action: () => splitPane("row") },
    { label: "向下分屏", action: () => splitPane("col") },
    { label: "搜索", action: () => openSearch() },
    { label: "", action: () => {}, separator: true },
    { label: "关闭面板", destructive: true, action: () => closePane(payload.id) },
  ];
  // 构造一个合成的 MouseEvent 位置（ContextMenu.open 需要事件）
  ctxOpen(new MouseEvent("contextmenu", { clientX: payload.x, clientY: payload.y }), items);
}

// ---------- 终端 cwd 捕获（OSC 7） ----------
// 通知捕获（OSC 9 / 777 / 133 / BEL）已上移到 Rust reader 线程（terminal.rs），
// 经全局 term-notify 事件由 termNotify.ts 统一上报；此处只保留视图专属的 cwd 跟踪。

function onOsc7(id: string, data: string) {
  // file://host/path → 取 path，实时更新标题栏 cwd
  try {
    const url = new URL(data);
    const path = decodeURIComponent(url.pathname);
    if (path && paneInfos[id]) paneInfos[id].cwd = path;
  } catch {
    /* 非法 URL 忽略 */
  }
}

// ---------- xterm 实例管理（按 pane） ----------

async function attachInstance(id: string, replay?: string, retries = 3) {
  await nextTick();
  const el = containers.get(id);
  if (!el) {
    // ref 注册晚于 nextTick 时重试，避免 pane 永远空白
    if (retries > 0 && !instances.has(id)) {
      setTimeout(() => attachInstance(id, replay, retries - 1), 50);
    }
    return;
  }
  if (instances.has(id)) return;

  const term = new Terminal({
    fontSize: 15,
    // 与 iTerm 一致：Fira Code 正文；内置 Symbols Nerd Font Mono 兜底图标/powerline 字形
    fontFamily:
      "'Fira Code', Menlo, 'Symbols Nerd Font Mono', 'MesloLGS NF', Monaco, 'Courier New', monospace",
    cursorBlink: true,
    scrollback: 10000,
    theme: {
      background: "#0d1117",
      foreground: "#e6edf3",
      cursor: "#e6edf3",
      selectionBackground: "#264f78",
    },
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.open(el);
  // Canvas 渲染器：字符严格按单元格网格绘制（含 box-drawing 自定义字形），
  // 修齐 DOM 渲染器下字体回退导致的 TUI 边框错位
  term.loadAddon(new CanvasAddon());
  fit.fit();

  if (replay) term.write(replay.replace(/\n/g, "\r\n"));

  const unlisten = await listen<string>(`term-output/${id}`, (e) => {
    term.write(e.payload);
  });

  term.onData((data) => {
    invoke("term_write", { id, data }).catch(() => {});
  });

  // OSC 7 实时 cwd 上报
  term.parser.registerOscHandler(7, (data) => {
    onOsc7(id, data);
    return true;
  });

  term.attachCustomKeyEventHandler((e) => handleKey(e, id));

  const inst: TermInstance = { term, fit, search, unlisten, observer: null };
  const observer = new ResizeObserver(() => {
    if (el.clientWidth === 0 || el.clientHeight === 0) return;
    fit.fit();
    invoke("term_resize", { id, cols: term.cols, rows: term.rows }).catch(() => {});
  });
  observer.observe(el);
  inst.observer = observer;
  instances.set(id, inst);
  registerTerm(id, term);

  // 初始同步一次真实尺寸
  invoke("term_resize", { id, cols: term.cols, rows: term.rows }).catch(() => {});
  term.focus();
}

function disposeInstance(id: string) {
  const inst = instances.get(id);
  if (!inst) return;
  inst.observer?.disconnect();
  inst.unlisten();
  inst.term.dispose();
  instances.delete(id);
  unregisterTerm(id);
}

function fitPane(id: string) {
  const inst = instances.get(id);
  if (!inst) return;
  inst.fit.fit();
  invoke("term_resize", { id, cols: inst.term.cols, rows: inst.term.rows }).catch(() => {});
}

function fitAndFocus(tab: Tab) {
  for (const leafId of allLeaves(tab.root)) fitPane(leafId);
  instances.get(tab.activePaneId)?.term.focus();
}

// ---------- 快捷键 ----------

function handleKey(e: KeyboardEvent, paneId: string): boolean {
  if (e.type !== "keydown") return true;
  const key = e.key.toLowerCase();
  // ⌘D 向右分屏 / ⌘⇧D 向下分屏
  if (e.metaKey && !e.ctrlKey && !e.altKey && key === "d") {
    splitPane(e.shiftKey ? "col" : "row");
    return false;
  }
  // ⌘W 关闭当前 pane
  if (e.metaKey && !e.ctrlKey && !e.altKey && key === "w") {
    closePane(paneId);
    return false;
  }
  // ⌘F 搜索
  if (e.metaKey && !e.ctrlKey && !e.altKey && key === "f") {
    openSearch();
    return false;
  }
  // ⌘⌥方向键 移动焦点
  if (e.metaKey && e.altKey && e.key.startsWith("Arrow")) {
    moveFocus(e.key);
    return false;
  }
  return true;
}

function moveFocus(key: string) {
  const tab = activeTab();
  if (!tab) return;
  const leaves = allLeaves(tab.root);
  const idx = leaves.indexOf(tab.activePaneId);
  if (idx < 0 || leaves.length < 2) return;
  const delta = key === "ArrowRight" || key === "ArrowDown" ? 1 : -1;
  const next = leaves[(idx + delta + leaves.length) % leaves.length];
  tab.activePaneId = next;
  instances.get(next)?.term.focus();
}

/**
 * 估算新 pane 的初始行列数，避免 PTY 以 80x24 启动后再 resize 引发 TUI 重绘伪影。
 * 有现存实例时用其 FitAddon 实测值（分屏则对半）；否则按窗口大小粗估。
 */
function estimateSize(dir?: "row" | "col"): { cols: number; rows: number } {
  const tab = activeTab();
  const inst = tab && instances.get(tab.activePaneId);
  const dims = inst?.fit.proposeDimensions();
  if (dims) {
    if (dir === "row")
      return { cols: Math.max(20, Math.floor(dims.cols / 2) - 1), rows: dims.rows };
    if (dir === "col")
      return { cols: dims.cols, rows: Math.max(5, Math.floor(dims.rows / 2) - 1) };
    return dims;
  }
  // Fira Code 15px 的单元格约 9.2 x 18.6 px
  return {
    cols: Math.max(40, Math.floor((window.innerWidth - 220) / 9.2)),
    rows: Math.max(10, Math.floor((window.innerHeight - 120) / 18.6)),
  };
}

// ---------- pane 操作 ----------

async function splitPane(dir: "row" | "col") {
  const tab = activeTab();
  if (!tab) return;
  // 继承分割前 pane 前台进程的实时工作目录（cd 过也能跟上），失败退回创建时的 cwd
  const cwd = await invoke<string>("term_foreground_cwd", { id: tab.activePaneId }).catch(
    () => paneCwds.get(tab.activePaneId) || null,
  );
  const { cols, rows } = estimateSize(dir);
  const info = await invoke<TermInfo>("term_create", { cols, rows, cwd });
  paneCwds.set(info.id, info.cwd);
  const newRoot = splitNode(tab.root, tab.activePaneId, dir, info.id);
  if (newRoot === tab.root) {
    invoke("term_close", { id: info.id }).catch(() => {});
    return;
  }
  tab.root = newRoot;
  tab.activePaneId = info.id;
  await attachInstance(info.id);
}

async function closePane(paneId: string | undefined) {
  if (!paneId) return;
  const tab = tabs.value.find((t) => allLeaves(t.root).includes(paneId));
  if (!tab) return;
  invoke("term_close", { id: paneId }).catch(() => {});
  // dispose 失败不能阻断树更新，否则会留下空框
  try {
    disposeInstance(paneId);
  } catch (e) {
    console.warn("[Terminal] dispose failed during closePane:", e);
    instances.delete(paneId);
  }
  paneCwds.delete(paneId);
  containers.delete(paneId);
  delete paneInfos[paneId];
  delete paneTitles[paneId];
  if (paneRename.id === paneId) paneRename.id = null;
  const newRoot = removeNode(tab.root, paneId);
  if (!newRoot) {
    // 最后一个 pane：关闭整个 tab
    closeTab(tab.id);
    return;
  }
  tab.root = newRoot;
  tab.activePaneId = firstLeaf(newRoot) || "";
  await nextTick();
  if (tab.id === activeTabId.value) {
    instances.get(tab.activePaneId)?.term.focus();
  }
}

function onPaneFocus(tab: Tab, paneId: string) {
  tab.activePaneId = paneId;
  instances.get(paneId)?.term.focus();
}

// ---------- 文件拖拽（拖入文件 → 插入本地路径） ----------

/** shell 安全转义：安全字符原样保留，其余单引号包裹（iTerm2 拖放同款行为） */
function shellQuote(p: string): string {
  if (/^[A-Za-z0-9_@%+=:,./~^-]+$/.test(p)) return p;
  return `'${p.replace(/'/g, `'\\''`)}'`;
}

/** 拖放落点定位 pane：物理像素转 CSS 像素后命中测试，未命中退回当前激活 pane */
function paneAtPosition(pos: { x: number; y: number }): string | null {
  const scale = window.devicePixelRatio || 1;
  const x = pos.x / scale;
  const y = pos.y / scale;
  for (const [id, el] of containers) {
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) continue;
    if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return id;
  }
  return activeTab()?.activePaneId || null;
}

function onFileDrop(paths: string[], position: { x: number; y: number }) {
  if (paths.length === 0) return;
  const paneId = paneAtPosition(position);
  const inst = paneId ? instances.get(paneId) : undefined;
  if (!paneId || !inst) return;
  let data = paths.map(shellQuote).join(" ");
  // 与 iTerm2 一致：应用开启 bracketed paste 时按"粘贴"投递，
  // CLI（kimi/claude code 等）靠这个标记把粘贴的图片路径识别为附件
  if (inst.term.modes.bracketedPasteMode) {
    data = `\x1b[200~${data}\x1b[201~`;
  }
  invoke("term_write", { id: paneId, data }).catch(() => {});
  inst.term.focus();
}

// ---------- tab 操作 ----------

async function newTerminal() {
  const { cols, rows } = estimateSize();
  const info = await invoke<TermInfo>("term_create", { cols, rows, cwd: null });
  paneCwds.set(info.id, info.cwd);
  const tab: Tab = {
    id: info.id,
    title: basename(info.shell),
    alive: true,
    root: createLeaf(info.id),
    activePaneId: info.id,
  };
  tabs.value.push(tab);
  activeTabId.value = tab.id;
  await attachInstance(info.id);
}

function closeTab(id: string) {
  const idx = tabs.value.findIndex((t) => t.id === id);
  if (idx < 0) return;
  const tab = tabs.value[idx];
  for (const leafId of allLeaves(tab.root)) {
    invoke("term_close", { id: leafId }).catch(() => {});
    disposeInstance(leafId);
    paneCwds.delete(leafId);
    containers.delete(leafId);
    delete paneInfos[leafId];
    delete paneTitles[leafId];
    if (paneRename.id === leafId) paneRename.id = null;
  }
  tabs.value.splice(idx, 1);
  if (activeTabId.value === id) {
    const next = tabs.value[Math.min(idx, tabs.value.length - 1)];
    activeTabId.value = next?.id || "";
    if (next) nextTick().then(() => fitAndFocus(next));
  }
}

async function activateTab(id: string) {
  activeTabId.value = id;
  await nextTick();
  const tab = activeTab();
  if (tab) fitAndFocus(tab);
}

// ---------- 搜索 ----------

function openSearch() {
  if (!activeTab()) return;
  searchOpen.value = true;
  nextTick(() => {
    searchInputEl.value?.focus();
    searchInputEl.value?.select();
  });
}

function closeSearch() {
  searchOpen.value = false;
  const tab = activeTab();
  if (tab) instances.get(tab.activePaneId)?.term.focus();
}

function searchNext() {
  const tab = activeTab();
  if (!tab || !searchQuery.value) return;
  instances.get(tab.activePaneId)?.search.findNext(searchQuery.value);
}

function searchPrev() {
  const tab = activeTab();
  if (!tab || !searchQuery.value) return;
  instances.get(tab.activePaneId)?.search.findPrevious(searchQuery.value);
}

function onSearchKeydown(e: KeyboardEvent) {
  if (e.key === "Enter") {
    if (e.shiftKey) searchPrev();
    else searchNext();
  } else if (e.key === "Escape") {
    closeSearch();
  }
}

// ---------- 标题轮询 / 生命周期 ----------

async function refreshTitles() {
  try {
    const list = await invoke<TermInfo[]>("term_list");
    const byId = new Map(list.map((t) => [t.id, t]));
    for (const info of list) {
      paneInfos[info.id] = {
        foreground_cmd: info.foreground_cmd,
        cwd: info.cwd,
        alive: info.alive,
      };
    }
    for (const tab of tabs.value) {
      const leaves = allLeaves(tab.root);
      const infos = leaves
        .map((id) => byId.get(id))
        .filter((i): i is TermInfo => !!i);
      const activeInfo = byId.get(tab.activePaneId);
      if (activeInfo && !tab.customTitle) tab.title = activeInfo.foreground_cmd;
      tab.alive = infos.some((i) => i.alive);
    }
  } catch {
    /* 忽略轮询错误 */
  }
}

onMounted(async () => {
  // 恢复已有 PTY 会话：每个存活会话一个 tab 单 pane（不恢复分屏布局）
  try {
    const list = await invoke<TermInfo[]>("term_list");
    for (const info of list) {
      paneCwds.set(info.id, info.cwd);
      tabs.value.push({
        id: info.id,
        title: info.foreground_cmd,
        alive: info.alive,
        root: createLeaf(info.id),
        activePaneId: info.id,
      });
    }
    for (const info of list) {
      let replay = "";
      try {
        replay = await invoke<string>("term_read", { id: info.id, maxBytes: null });
      } catch {
        /* scrollback 读取失败则跳过回放 */
      }
      await attachInstance(info.id, replay);
    }
    if (list.length > 0) {
      activeTabId.value = list[0].id;
      await nextTick();
      instances.get(list[0].id)?.term.focus();
    }
  } catch {
    /* term_list 失败则直接新建 */
  }
  if (tabs.value.length === 0) await newTerminal();
  await refreshTitles();
  pollTimer = setInterval(refreshTitles, 5000);
  document.addEventListener("pointerdown", onGlobalPointerDown);
  document.addEventListener("keydown", onGlobalKeydown);
  // 窗口焦点映射到 xterm textarea 焦点：kimi 等 TUI 靠 CSI I/O 焦点追踪
  // 决定是否发"失焦通知"，webview 里窗口切走不会自动 blur，需要显式同步
  unlistenFocus = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) {
      const tab = activeTab();
      if (tab) instances.get(tab.activePaneId)?.term.focus();
    } else {
      for (const inst of instances.values()) inst.term.blur();
    }
  });
  // 拖文件进终端：插入 shell 转义后的本地路径（Tauri 原生拖放事件，
  // webview 的 HTML5 drop 在 dragDropEnabled 下收不到文件路径）
  unlistenDrag = await getCurrentWindow().onDragDropEvent((event) => {
    if (event.payload.type === "drop") {
      onFileDrop(event.payload.paths, event.payload.position);
    }
  });
});

onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer);
  unlistenFocus?.();
  unlistenFocus = null;
  unlistenDrag?.();
  unlistenDrag = null;
  document.removeEventListener("pointerdown", onGlobalPointerDown);
  document.removeEventListener("keydown", onGlobalKeydown);
  // 只销毁前端实例，PTY 会话保留，回到页面时可重连
  for (const id of [...instances.keys()]) disposeInstance(id);
});
</script>

<template>
  <div class="terminal-view" @contextmenu.prevent>
    <!-- Tab Bar -->
    <div class="tab-bar">
      <!-- 悬浮菜单（替代左侧导航） -->
      <div class="menu-wrap">
        <button
          class="tab-action-btn menu-btn"
          :class="{ open: menuOpen }"
          @click="toggleMenu"
          aria-label="菜单"
          title="菜单"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <path d="M2 3.5h10M2 7h10M2 10.5h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
          </svg>
        </button>
        <div v-if="menuOpen" class="menu-dropdown">
          <button
            v-for="item in menuItems"
            :key="item.name"
            class="menu-item"
            @click="goMenuItem(item.name)"
          >
            {{ item.label }}
          </button>
        </div>
      </div>

      <div
        v-for="tab in sortedTabs"
        :key="tab.id"
        class="tab"
        :class="{ active: tab.id === activeTabId, dead: !tab.alive, pinned: tab.pinned }"
        @click="activateTab(tab.id)"
        @dblclick="startRenameTab(tab)"
        @contextmenu="onTabContextMenu($event, tab)"
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
        <button class="tab-close" @click.stop="closeTab(tab.id)" aria-label="关闭终端">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </button>
      </div>

      <button class="tab-add" @click="newTerminal" aria-label="新建终端" title="新建终端">
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path d="M7 2v10M2 7h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
        </svg>
      </button>

      <div class="tab-actions">
        <button
          class="tab-action-btn"
          @click="splitPane('row')"
          :disabled="!activeTab()"
          aria-label="向右分屏"
          title="向右分屏 (⌘D)"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <rect x="1.5" y="2" width="11" height="10" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
            <path d="M7 2v10" stroke="currentColor" stroke-width="1.2"/>
          </svg>
        </button>
        <button
          class="tab-action-btn"
          @click="splitPane('col')"
          :disabled="!activeTab()"
          aria-label="向下分屏"
          title="向下分屏 (⌘⇧D)"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <rect x="1.5" y="2" width="11" height="10" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
            <path d="M1.5 7h11" stroke="currentColor" stroke-width="1.2"/>
          </svg>
        </button>
        <button
          class="tab-action-btn"
          @click="closePane(activeTab()?.activePaneId)"
          :disabled="!activeTab()"
          aria-label="关闭面板"
          title="关闭面板 (⌘W)"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </div>

    <!-- Terminal Panes -->
    <div class="term-area">
      <div
        v-for="tab in tabs"
        :key="tab.id"
        v-show="tab.id === activeTabId"
        class="tab-panes"
      >
        <PaneNode
          :node="tab.root"
          :active-pane-id="tab.activePaneId"
          @focus="onPaneFocus(tab, $event)"
          @ctx="onPaneContextMenu(tab, $event)"
        />
      </div>

      <!-- 搜索框 -->
      <div v-if="searchOpen" class="search-box">
        <input
          ref="searchInputEl"
          v-model="searchQuery"
          class="search-input"
          placeholder="查找"
          @keydown="onSearchKeydown"
          @input="searchNext"
        />
        <button class="search-btn" @click="searchPrev" aria-label="上一个" title="上一个 (⇧Enter)">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M6 2.5v7M2.5 6L6 2.5 9.5 6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
        <button class="search-btn" @click="searchNext" aria-label="下一个" title="下一个 (Enter)">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M6 2.5v7M2.5 6L6 9.5 9.5 6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
        <button class="search-btn" @click="closeSearch" aria-label="关闭" title="关闭 (Esc)">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </button>
      </div>

      <div v-if="tabs.length === 0" class="term-empty">
        <p>没有打开的终端</p>
        <button class="term-empty-btn" @click="newTerminal">新建终端</button>
      </div>

      <ContextMenu :visible="ctxVisible" :position="ctxPosition" :items="ctxItems" @close="ctxClose" />
    </div>
  </div>
</template>

<style scoped>
.terminal-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: var(--color-surface-0);
}

.tab-bar {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: var(--space-1) var(--space-2);
  background: var(--color-surface-1);
  border-bottom: 1px solid var(--color-border-subtle);
  overflow-x: auto;
  flex-shrink: 0;
}

.tab {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2);
  font-size: 12px;
  color: var(--color-text-secondary);
  cursor: pointer;
  border-radius: var(--radius-sm);
  white-space: nowrap;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.tab:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.tab.active {
  color: var(--color-text-primary);
  background: var(--color-surface-2);
}

.tab.dead .tab-title {
  text-decoration: line-through;
  opacity: 0.6;
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

.tab-add {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  margin-left: var(--space-1);
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  flex-shrink: 0;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.tab-add:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.tab-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

/* 悬浮菜单 */
.menu-wrap {
  position: relative;
  flex-shrink: 0;
  margin-right: var(--space-1);
}

.menu-btn.open {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.menu-dropdown {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  z-index: 30;
  min-width: 120px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  background: var(--color-surface-2);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md, 8px);
  box-shadow: 0 8px 24px oklch(0 0 0 / 0.45);
}

.menu-item {
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

.menu-item:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.tab-action-btn {
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

.tab-action-btn:hover:not(:disabled) {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.tab-action-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.term-area {
  flex: 1;
  min-height: 0;
  position: relative;
  background: #0d1117;
}

.tab-panes {
  position: absolute;
  inset: 0;
}

.search-box {
  position: absolute;
  top: var(--space-2);
  right: var(--space-3);
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 3px;
  background: var(--color-surface-2);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  box-shadow: 0 4px 12px oklch(0 0 0 / 0.4);
}

.search-input {
  width: 180px;
  padding: 2px var(--space-1);
  font-size: 12px;
  font-family: inherit;
  color: var(--color-text-primary);
  background: var(--color-surface-0);
  border: 1px solid var(--color-border-subtle);
  border-radius: var(--radius-sm);
  outline: none;
}

.search-input:focus {
  border-color: var(--color-accent);
}

.search-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 0;
  transition: color var(--duration-fast), background var(--duration-fast);
}

.search-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}

.term-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-3);
  color: var(--color-text-muted);
  font-size: 13px;
}

.term-empty-btn {
  padding: var(--space-1) var(--space-3);
  font-size: 12px;
  border: none;
  border-radius: var(--radius-sm);
  background: var(--color-accent);
  color: var(--color-surface-0);
  cursor: pointer;
}
</style>
