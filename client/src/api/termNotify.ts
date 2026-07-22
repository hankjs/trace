import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { listen } from "@tauri-apps/api/event";
import { apiRequest } from "../composables/useSession";

/**
 * 终端通知捕获后的统一处理：
 * 1. macOS 系统通知（懒申请权限）
 * 2. 上报 server（落库，后续是否推微信等由消费方决定）
 *
 * 相同 term + body 5 秒内去重，防止 spinner/重绘重复触发。
 */
const recent = new Map<string, number>();
const DEDUPE_MS = 5000;

export interface TermNotifyPayload {
  clientId: string;
  termId: string;
  title: string;
  body: string;
  /** notification(默认) / bell(响铃) / command(命令完成) */
  kind?: string;
}

export function reportTermNotification(p: TermNotifyPayload) {
  const key = `${p.termId}:${p.body}`;
  const now = Date.now();
  if (now - (recent.get(key) || 0) < DEDUPE_MS) return;
  recent.set(key, now);

  // 1. 系统通知
  (async () => {
    try {
      let granted = await isPermissionGranted();
      if (!granted) {
        granted = (await requestPermission()) === "granted";
      }
      if (granted) {
        sendNotification({ title: p.title, body: p.body });
      }
    } catch (e) {
      console.warn("[TermNotify] system notification failed:", e);
    }
  })();

  // 2. 上报 server（失败静默，不影响本地通知）
  apiRequest("/api/client/notify", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      client_id: p.clientId,
      term_id: p.termId,
      kind: p.kind || "notification",
      title: p.title,
      body: p.body,
    }),
  }).catch((e) => console.warn("[TermNotify] report to server failed:", e));
}

/** Rust 侧 term-notify 事件负载（terminal.rs reader 线程统一捕获） */
interface TermNotifyEvent {
  id: string;
  kind: string;
  title: string;
  body: string;
}

let listening = false;

/**
 * 注册全局 term-notify 监听：OSC 9/777/133/BEL 由 Rust reader 线程统一捕获，
 * 与视图是否附着无关（无头终端如微信托管的 kimi CLI 也能上报）。
 * 幂等：重复调用只注册一次。
 */
export function initTermNotifyListener(getClientId: () => string) {
  if (listening) return;
  listening = true;
  listen<TermNotifyEvent>("term-notify", (e) => {
    reportTermNotification({
      clientId: getClientId(),
      termId: e.payload.id,
      title: e.payload.title,
      body: e.payload.body,
      kind: e.payload.kind,
    });
  }).catch((e) => console.warn("[TermNotify] listen term-notify failed:", e));
}
