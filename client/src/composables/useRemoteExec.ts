import { ref, readonly } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { execToolLocal } from "../agents/ExploreAgent/localTools";
import {
  pollToolRequests,
  postToolResult,
  registerClient,
  type RemoteToolRequest,
} from "../api/remoteExec";

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

// 模块级共享状态（与 useSession 同一模式）
const clientId = loadClientId();
const acceptRemote = ref(localStorage.getItem(ACCEPT_KEY) === "1");
const workDir = ref(localStorage.getItem(WORKDIR_KEY) || "");
const isPolling = ref(false);

let running = false;
let abort: AbortController | null = null;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** shell 单引号转义 */
function shellQuote(s: string): string {
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

/**
 * git 工具 input → git CLI 命令的简单映射。
 * 未覆盖的 operation 返回 null，提示 agent 改用 shell 工具自行拼命令。
 */
function buildGitCommand(input: Record<string, any>): string | null {
  const op = String(input?.operation || "").toLowerCase();
  switch (op) {
    case "status":
      return "git status";
    case "diff":
      if (input.staged || input.cached) return "git diff --staged";
      if (input.path) return `git diff -- ${shellQuote(input.path)}`;
      return "git diff";
    case "log":
      return `git log --oneline -n ${Number(input.n) || 20}`;
    case "branch":
      return "git branch -a";
    case "add": {
      const paths: string[] = Array.isArray(input.paths) ? input.paths : input.path ? [input.path] : [];
      if (paths.length === 0) return "git add -A";
      return `git add ${paths.map(shellQuote).join(" ")}`;
    }
    case "commit":
      return input.message ? `git commit -m ${shellQuote(input.message)}` : null;
    case "checkout":
      return input.target ? `git checkout ${shellQuote(input.target)}` : null;
    case "pull":
      return "git pull";
    case "push":
      return "git push";
    case "fetch":
      return "git fetch";
    default:
      return null;
  }
}

/** 执行一条远程下发的工具调用 */
async function executeRemoteTool(req: RemoteToolRequest): Promise<{ content: string; is_error: boolean }> {
  const dir = workDir.value || ".";
  const input = req.input || {};
  try {
    switch (req.tool) {
      case "shell":
        // 远程下发场景需要完整执行权限，直接走 execToolLocal（不含白名单校验）；
        // ExploreAgent 的 execTool 只读白名单不受影响。
        return await execToolLocal("bash", { command: input.command, timeout_ms: input.timeout_ms }, dir);
      case "read_file":
        return await execToolLocal("read_file", input, dir);
      case "write_file":
        return await execToolLocal("write_file", input, dir);
      case "str_replace":
        return await execToolLocal("edit", input, dir);
      case "search":
        return await execToolLocal("search", input, dir);
      case "list_directory":
        return await execToolLocal("glob", { pattern: input.pattern || "**/*", path: input.path }, dir);
      case "git": {
        const cmd = buildGitCommand(input);
        if (!cmd) {
          return {
            content: `Unsupported git operation: ${input.operation || "(missing)"}. Use the shell tool with an explicit git command instead.`,
            is_error: true,
          };
        }
        return await execToolLocal("bash", { command: cmd }, dir);
      }
      case "terminal_list": {
        const list = await invoke("term_list");
        return { content: JSON.stringify(list), is_error: false };
      }
      case "terminal_read": {
        const lines = Number(input.lines) || 200;
        // 未指定 lines 时按字节兜底，避免返回过大；指定 lines 时读全量再按行截尾
        const text = await invoke<string>("term_read", {
          id: input.id,
          maxBytes: input.lines ? null : 40000,
        });
        const tail = text.split("\n").slice(-lines).join("\n");
        return { content: tail, is_error: false };
      }
      case "terminal_write": {
        await invoke("term_write", { id: input.id, data: String(input.data ?? "") });
        return { content: "ok", is_error: false };
      }
      default:
        return { content: `Unknown tool: ${req.tool}`, is_error: true };
    }
  } catch (e: any) {
    return { content: `Remote exec error: ${e?.message || e}`, is_error: true };
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
    // 正常返回（含空 requests）立即进入下一轮
    backoffMs = 1000;
    if (result.requests.length > 0) {
      await Promise.all(
        result.requests.map(async (req) => {
          const r = await executeRemoteTool(req);
          await postToolResult({ request_id: req.request_id, content: r.content, is_error: r.is_error });
        })
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

/** 上报当前注册信息到 server */
async function syncRegistration() {
  const result = await registerClient({
    client_id: clientId,
    work_dir: workDir.value || undefined,
    accept_remote: acceptRemote.value,
  });
  if (!result.ok) console.warn("[RemoteExec] registration failed:", result.msg);
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

export function useRemoteExec() {
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
