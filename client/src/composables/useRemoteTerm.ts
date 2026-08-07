/**
 * 桌面 client 远程终端通道：注册 + 长轮询 + 只处理 terminal_* / rtc_signal。
 * 故意不恢复 shell/文件类远程执行，防止 agent 远程执行面回潮。
 *
 * 轮询生命周期用 loopGen 防竞态：旧 pollLoop 的 finally 不得清掉新 loop 的 isPolling。
 */
import { ref, readonly, computed } from "vue";
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
/** 最近一次注册/轮询错误（成功轮询会清空） */
const lastError = ref("");
/** 最近一次成功 poll 的时间戳（ms），0 = 从未 */
const lastPollAt = ref(0);

/** 单调递增：stop / 重启时 bump，旧 loop 发现 gen 不匹配即退出且不改共享状态 */
let loopGen = 0;
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

function isCurrent(gen: number): boolean {
  return gen === loopGen;
}

/** 长轮询主循环：错误指数退避 1s→30s，401 停止当前 loop */
async function pollLoop(gen: number) {
  let backoffMs = 1000;
  while (isCurrent(gen)) {
    const controller = new AbortController();
    abort = controller;
    const result = await pollToolRequests(clientId, controller.signal);
    if (!isCurrent(gen)) break;

    if (result.kind === "unauthorized") {
      lastError.value = "登录已失效（401），远程终端轮询已停止";
      // 作废本 loop；authFetch 已 clearAuth，App 会跟着 stop
      if (isCurrent(gen)) {
        loopGen++;
        isPolling.value = false;
        abort = null;
      }
      break;
    }

    if (result.kind === "error") {
      // abort 触发的中断不当错误展示
      const aborted =
        controller.signal.aborted ||
        /abort/i.test(result.message || "");
      if (!aborted) {
        lastError.value = result.message || "轮询失败";
        console.warn("[RemoteTerm] poll error:", result.message, result.status);
      }
      await sleep(backoffMs);
      if (!isCurrent(gen)) break;
      backoffMs = Math.min(backoffMs * 2, 30000);
      continue;
    }

    // 成功一轮（含空 requests）
    backoffMs = 1000;
    lastPollAt.value = Date.now();
    if (lastError.value) lastError.value = "";

    if (result.requests.length > 0) {
      await Promise.all(
        result.requests.map(async (req) => {
          const r = await executeRemoteTool(req);
          if (!isCurrent(gen)) return;
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

/**
 * 启动（或重启）长轮询。
 * 始终 bump gen：保证任意旧 loop 的 finally 不会清掉新状态。
 */
function startPolling() {
  if (!acceptRemote.value) return;

  // 停掉旧 loop（若有）
  loopGen++;
  abort?.abort();
  const gen = loopGen;
  abort = null;
  isPolling.value = true;

  pollLoop(gen).finally(() => {
    // 只有自己仍是当前 gen 时才清 isPolling
    if (gen === loopGen) {
      isPolling.value = false;
      abort = null;
    }
  });
}

function stopPolling() {
  loopGen++;
  abort?.abort();
  abort = null;
  isPolling.value = false;
}

async function syncRegistration(): Promise<{ ok: boolean; msg?: string }> {
  try {
    const result = await registerClient({
      client_id: clientId,
      hostname: await hostname(),
      work_dir: workDir.value || undefined,
      accept_remote: acceptRemote.value,
    });
    if (!result.ok) {
      const msg = result.msg || "注册失败";
      console.warn("[RemoteTerm] registration failed:", msg);
      lastError.value = msg;
      return { ok: false, msg };
    }
    return { ok: true };
  } catch (e: any) {
    // apiRequest 已吞网络错误；此处兜底未预期抛错
    const msg = e?.message || String(e) || "注册异常";
    console.warn("[RemoteTerm] registration threw:", msg);
    lastError.value = msg;
    return { ok: false, msg };
  }
}

/** 登录后调用：若 accept_remote 开启则注册并启动轮询 */
async function startIfEnabled() {
  if (!acceptRemote.value) return;
  const reg = await syncRegistration();
  if (!reg.ok) {
    // 保留开关意图，但标错误；仍尝试 poll（server 偶发抖动时下次退避可恢复）
    // 注册失败时不启动 poll 更干净——没有 DB 行 admin 也看不到
    return;
  }
  startPolling();
}

export type SetAcceptRemoteResult = { ok: boolean; message: string };

/**
 * 开关远程终端。
 * - 开启：先写意图 → 注册 → 成功才 startPolling；注册失败回滚开关
 * - 关闭：stop + 注册 accept_remote=false
 */
async function setAcceptRemote(v: boolean): Promise<SetAcceptRemoteResult> {
  if (!v) {
    acceptRemote.value = false;
    localStorage.setItem(ACCEPT_KEY, "0");
    stopPolling();
    lastError.value = "";
    const reg = await syncRegistration();
    if (!reg.ok) {
      // 本地已关轮询；远端注册失败只提示
      return {
        ok: true,
        message: `已关闭本地轮询（同步 server 失败：${reg.msg}）`,
      };
    }
    return { ok: true, message: "已关闭远程终端" };
  }

  // 开启：先落本地意图
  acceptRemote.value = true;
  localStorage.setItem(ACCEPT_KEY, "1");
  lastError.value = "";
  const reg = await syncRegistration();
  if (!reg.ok) {
    // 回滚：避免「开关开着但 server 无登记」
    acceptRemote.value = false;
    localStorage.setItem(ACCEPT_KEY, "0");
    stopPolling();
    return {
      ok: false,
      message: `开启失败：${reg.msg || "无法注册到 server"}`,
    };
  }
  startPolling();
  return { ok: true, message: "已开启，正在长轮询" };
}

async function setWorkDir(dir: string) {
  workDir.value = dir;
  localStorage.setItem(WORKDIR_KEY, dir);
  if (acceptRemote.value) await syncRegistration();
}

/** 设置页展示用的状态摘要 */
const statusText = computed(() => {
  if (!acceptRemote.value) return "已关闭";
  if (isPolling.value) {
    if (lastPollAt.value > 0) {
      const sec = Math.max(0, Math.floor((Date.now() - lastPollAt.value) / 1000));
      if (sec < 60) return "在线轮询中";
      return `在线轮询中（上次成功 ${sec}s 前）`;
    }
    return "在线轮询中（等待首次响应…）";
  }
  if (lastError.value) return "已开启但轮询已停止";
  return "已开启（轮询未运行）";
});

export function useRemoteTerm() {
  return {
    clientId,
    acceptRemote: readonly(acceptRemote),
    workDir: readonly(workDir),
    isPolling: readonly(isPolling),
    lastError: readonly(lastError),
    lastPollAt: readonly(lastPollAt),
    statusText,
    startIfEnabled,
    startPolling,
    stopPolling,
    syncRegistration,
    setAcceptRemote,
    setWorkDir,
  };
}
