/**
 * 终端远程占用状态：app 连上某 pane 时 client 显示「远程中」遮罩，
 * 并停止本地 fit→PTY 抢尺寸，改用 app 传来的 cols/rows。
 *
 * 来源：
 * - 中转：terminal_read / write / resize 心跳（TTL 过期释放）
 * - RTC：Rust `term-remote` 事件（sticky，对端关闭才释放）
 */
import { reactive, readonly, computed } from "vue";

export interface RemoteTermState {
  /** 最近一次远程活动时间（ms） */
  lastSeen: number;
  cols?: number;
  rows?: number;
  /** RTC 附着：不走 TTL，等 active=false */
  sticky: boolean;
}

/** termId → 占用状态 */
const remoteTerms = reactive<Record<string, RemoteTermState>>({});

/** 中转通道无活动后释放占用的宽限（relay 3s 拉一次，10s 足够） */
const RELAY_TTL_MS = 10_000;
const SWEEP_MS = 2_000;

let sweepTimer: ReturnType<typeof setInterval> | null = null;
let unlistenRemote: (() => void) | null = null;
let listenersStarted = false;

function ensureSweep() {
  if (sweepTimer) return;
  sweepTimer = setInterval(() => {
    const now = Date.now();
    for (const id of Object.keys(remoteTerms)) {
      const s = remoteTerms[id];
      if (!s) continue;
      if (s.sticky) continue;
      if (now - s.lastSeen > RELAY_TTL_MS) {
        delete remoteTerms[id];
      }
    }
    if (Object.keys(remoteTerms).length === 0 && sweepTimer) {
      clearInterval(sweepTimer);
      sweepTimer = null;
    }
  }, SWEEP_MS);
}

/** 标记 term 被远程占用（中转心跳 / 显式 resize） */
export function markRemoteControl(
  id: string,
  opts?: { cols?: number; rows?: number; sticky?: boolean },
) {
  if (!id) return;
  const prev = remoteTerms[id];
  remoteTerms[id] = {
    lastSeen: Date.now(),
    cols: opts?.cols ?? prev?.cols,
    rows: opts?.rows ?? prev?.rows,
    sticky: opts?.sticky ?? prev?.sticky ?? false,
  };
  ensureSweep();
}

/** 释放远程占用（RTC 关闭 / 会话退出） */
export function clearRemoteControl(id: string) {
  if (!id) return;
  delete remoteTerms[id];
}

export function isRemoteControlled(id: string): boolean {
  return !!remoteTerms[id];
}

export function getRemoteSize(
  id: string,
): { cols: number; rows: number } | null {
  const s = remoteTerms[id];
  if (!s || !s.cols || !s.rows) return null;
  return { cols: s.cols, rows: s.rows };
}

/**
 * 启动 Rust `term-remote` 事件监听（幂等）。
 * 在远程轮询开启时调用即可。
 */
export async function startRemoteControlListeners() {
  if (listenersStarted) return;
  listenersStarted = true;
  try {
    const { listen } = await import("@tauri-apps/api/event");
    unlistenRemote = await listen<{
      id: string;
      active: boolean;
      cols?: number | null;
      rows?: number | null;
    }>("term-remote", (e) => {
      const { id, active, cols, rows } = e.payload || {};
      if (!id) return;
      if (!active) {
        clearRemoteControl(id);
        return;
      }
      markRemoteControl(id, {
        sticky: true,
        cols: cols ?? undefined,
        rows: rows ?? undefined,
      });
    });
  } catch {
    // 非 Tauri 环境（纯 web 开发）忽略
    listenersStarted = false;
  }
}

export function stopRemoteControlListeners() {
  unlistenRemote?.();
  unlistenRemote = null;
  listenersStarted = false;
}

export function useRemoteControl() {
  const controlledIds = computed(() => Object.keys(remoteTerms));
  return {
    remoteTerms: readonly(remoteTerms),
    controlledIds,
    markRemoteControl,
    clearRemoteControl,
    isRemoteControlled,
    getRemoteSize,
    startRemoteControlListeners,
    stopRemoteControlListeners,
  };
}
