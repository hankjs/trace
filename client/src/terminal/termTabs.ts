/**
 * 终端 tab 的模块级状态：全局 tab 栏（GlobalTabBar）与终端页（TerminalView）共享。
 * 状态提升到模块级后，切换到其他页面再回来不会丢失分屏布局；
 * xterm 前端实例仍由 TerminalView 管理（离开时销毁，回来时按 PTY scrollback 回放重建）。
 */
import { ref, reactive, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { allLeaves, type LayoutNode } from "./layout";

export interface TermInfo {
  id: string;
  shell: string;
  cwd: string;
  foreground_cmd: string;
  alive: boolean;
  created_at: string;
}

export interface TermTab {
  id: string;
  title: string;
  /** 用户重命名后的固定标题（不再被前台进程名覆盖） */
  customTitle?: string;
  pinned?: boolean;
  alive: boolean;
  root: LayoutNode;
  activePaneId: string;
}

export const termTabs = ref<TermTab[]>([]);
export const activeTermTabId = ref("");

/** pane id -> 实时信息（标题栏/tab 标题用，5s 轮询更新） */
export const paneInfos = reactive<Record<string, { foreground_cmd: string; cwd: string; alive: boolean }>>({});

/** pin 的 tab 排在前面（稳定排序） */
export const sortedTermTabs = computed(() => {
  return termTabs.value
    .map((t, i) => ({ t, i }))
    .sort((a, b) => Number(b.t.pinned ?? false) - Number(a.t.pinned ?? false) || a.i - b.i)
    .map(({ t }) => t);
});

/**
 * 由 TerminalView 注入的实现，供全局 tab 栏在任何页面调用。
 * TerminalView 未挂载时 create/close/activate 可能为 null（首次进入尚未打开过终端页）。
 */
export const termActions: {
  create: (() => Promise<void>) | null;
  close: ((id: string) => void) | null;
  activate: ((id: string) => Promise<void>) | null;
} = {
  create: null,
  close: null,
  activate: null,
};

// ---------- 标题/存活状态轮询（全局，离开终端页也要更新 tab 标题） ----------

export async function refreshTermTitles() {
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
    for (const tab of termTabs.value) {
      const leaves = allLeaves(tab.root);
      const infos = leaves.map((id) => byId.get(id)).filter((i): i is TermInfo => !!i);
      const activeInfo = byId.get(tab.activePaneId);
      if (activeInfo && !tab.customTitle) tab.title = activeInfo.foreground_cmd;
      tab.alive = infos.some((i) => i.alive);
    }
  } catch {
    /* 忽略轮询错误 */
  }
}

let pollStarted = false;

/** 启动全局标题轮询（幂等） */
export function ensureTitlePolling() {
  if (pollStarted) return;
  pollStarted = true;
  refreshTermTitles();
  setInterval(refreshTermTitles, 5000);
}
