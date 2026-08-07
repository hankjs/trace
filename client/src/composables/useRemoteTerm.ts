/**
 * 桌面 client 远程终端通道：注册 + 长轮询 + 只处理 terminal_* / rtc_signal。
 * 故意不恢复 shell/文件类远程执行，防止 agent 远程执行面回潮。
 */
import { ref, readonly } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  pollToolRequests,
  postToolResult,
  registerClient,
  type RemoteToolRequest,
} from "../api/remoteTerm";

const CLIENT_ID_KEY = "hank_remote_client_id";
const ACCEPT_KEY = "hank_accept_remote";
const WORKDIR_KEY = "hank_remote_work_dir";

function loadClientId(): string {
  let id = localStorage.getItem(CLIENT_ID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(CLIENT_ID_KEY, id);
  }
  return id;
}

const clientId = loadClientId();
const acceptRemote = ref(localStorage.getItem(ACCEPT_KEY) === "1");
const workDir = ref(localStorage.getItem(WORKDIR_KEY) || "");
const isPolling = ref(false);

let running = false;
let abort: AbortController | null = null;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 工具白名单：只允许终端与 RTC 信令 */
const ALLOWED_TOOLS = new Set([
  "terminal_list",
  "terminal_read",
  "terminal_write",
  "terminal_create",
  "terminal_close",
  "terminal_resize",
  "rtc_signal",
]);

async function hostname(): Promise<string | undefined> {
  try {
    // 浏览器/WebView 无可靠 hostname API；用 userAgent 片段兜底展示
    const ua = navigator.userAgent;
    if (ua.includes("Mac")) return "macOS";
    if (ua.includes("Windows")) return "Windows";
    if (ua.includes("Linux")) return "Linux";
    return undefined;
  } catch {
    return undefined;
  }
}

/** 执行一条远程下发的工具调用（仅终端 / RTC） */
async function executeRemoteTool(
  req: RemoteToolRequest,
): Promise<{ content: string; is_error: boolean }> {
  if (!ALLOWED_TOOLS.has(req.tool)) {
    return {
      content: `拒绝执行未授权工具: ${req.tool}（远程终端通道仅允许 terminal_* / rtc_signal）`,
      is_error: true,
    };
  }
  const input = req.input || {};
  try {
    switch (req.tool) {
      case "terminal_list": {
        const list = await invoke("term_list");
        return { content: JSON.stringify(list), is_error: false };
      }
      case "terminal_read": {
        if (input.raw) {
          const { serializeScreen } = await import("../terminal/screenRegistry");
          const snap = serializeScreen(input.id);
          if (snap !== null) return { content: snap, is_error: false };
          const text = await invoke<string>("term_read", {
            id: input.id,
            maxBytes: input.maxBytes ?? 65536,
            raw: true,
          });
          return { content: text, is_error: false };
        }
        const lines = Number(input.lines) || 200;
        const text = await invoke<string>("term_read", {
          id: input.id,
          maxBytes: input.lines ? null : 40000,
        });
        const tail = text.split("\n").slice(-lines).join("\n");
        return { content: tail, is_error: false };
      }
      case "terminal_write": {
        await invoke("term_write", {
          id: input.id,
          data: String(input.data ?? ""),
        });
        return { content: "ok", is_error: false };
      }
      case "terminal_create": {
        const info = await invoke("term_create", {
          cols: Number(input.cols) || 120,
          rows: Number(input.rows) || 30,
          cwd: input.cwd ?? null,
        });
        return { content: JSON.stringify(info), is_error: false };
      }
      case "terminal_close": {
        await invoke("term_close", { id: String(input.id ?? "") });
        return { content: "ok", is_error: false };
      }
      case "terminal_resize": {
        await invoke("term_resize", {
          id: String(input.id ?? ""),
          cols: Number(input.cols) || 80,
          rows: Number(input.rows) || 24,
        });
        return { content: "ok", is_error: false };
      }
      case "rtc_signal": {
        // offer SDP → answer SDP；数据面在 Rust 后台任务里跑
        const answer = await invoke<string>("rtc_accept_offer", {
          offerSdp: String(input.sdp ?? ""),
        });
        return { content: answer, is_error: false };
      }
      default:
        return { content: `Unknown tool: ${req.tool}`, is_error: true };
    }
  } catch (e: any) {
    return { content: `Remote term error: ${e?.message || e}`, is_error: true };
  }
}

/** 长轮询主循环：错误指数退避 1s→30s，401 停止 */
async function pollLoop() {
  let backoffMs = 1000;
  while (running) {
    abort = new AbortController();
    const result = await pollToolRequests(clientId, abort.signal);
    if (!running) break;
    if (result.kind === "unauthorized") {
      stopPolling();
      break;
    }
    if (result.kind === "error") {
      await sleep(backoffMs);
      backoffMs = Math.min(backoffMs * 2, 30000);
      continue;
    }
    backoffMs = 1000;
    if (result.requests.length > 0) {
      await Promise.all(
        result.requests.map(async (req) => {
          const r = await executeRemoteTool(req);
          await postToolResult({
            request_id: req.request_id,
            content: r.content,
            is_error: r.is_error,
          });
        }),
      );
    }
  }
}

function startPolling() {
  if (running || !acceptRemote.value) return;
  running = true;
  isPolling.value = true;
  pollLoop().finally(() => {
    running = false;
    isPolling.value = false;
  });
}

function stopPolling() {
  running = false;
  abort?.abort();
  abort = null;
}

async function syncRegistration() {
  const result = await registerClient({
    client_id: clientId,
    hostname: await hostname(),
    work_dir: workDir.value || undefined,
    accept_remote: acceptRemote.value,
  });
  if (!result.ok) console.warn("[RemoteTerm] registration failed:", result.msg);
  return result.ok;
}

/** 登录后调用：若 accept_remote 开启则注册并启动轮询 */
async function startIfEnabled() {
  if (!acceptRemote.value) return;
  await syncRegistration();
  startPolling();
}

async function setAcceptRemote(v: boolean) {
  acceptRemote.value = v;
  localStorage.setItem(ACCEPT_KEY, v ? "1" : "0");
  await syncRegistration();
  if (v) startPolling();
  else stopPolling();
}

async function setWorkDir(dir: string) {
  workDir.value = dir;
  localStorage.setItem(WORKDIR_KEY, dir);
  if (acceptRemote.value) await syncRegistration();
}

export function useRemoteTerm() {
  return {
    clientId,
    acceptRemote: readonly(acceptRemote),
    workDir: readonly(workDir),
    isPolling: readonly(isPolling),
    startIfEnabled,
    startPolling,
    stopPolling,
    syncRegistration,
    setAcceptRemote,
    setWorkDir,
  };
}
