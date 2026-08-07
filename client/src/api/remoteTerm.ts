import { apiRequest, authFetch } from "../composables/useSession";

/** 远程下发的工具调用请求 */
export interface RemoteToolRequest {
  request_id: string;
  tool: string;
  input: Record<string, any>;
}

/** 在线 client 信息（GET /api/client/online） */
export interface OnlineClient {
  client_id: string;
  hostname?: string | null;
  work_dir?: string | null;
  accept_remote: boolean;
  online: boolean;
}

export interface RegisterClientBody {
  client_id: string;
  hostname?: string;
  work_dir?: string;
  accept_remote: boolean;
}

/** 长轮询结果：区分正常返回 / 401 / 其他错误，供退避策略使用 */
export type PollResult =
  | { kind: "ok"; requests: RemoteToolRequest[] }
  | { kind: "unauthorized" }
  | { kind: "error"; status?: number; message: string };

/** 注册/更新本机 client */
export async function registerClient(body: RegisterClientBody) {
  return apiRequest<{ client_id: string }>("/api/client/registration", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** 长轮询拉取待执行的工具调用（server 挂起最长 25s，空 requests 属正常） */
export async function pollToolRequests(
  clientId: string,
  signal?: AbortSignal,
): Promise<PollResult> {
  try {
    const res = await authFetch(
      `/api/client/poll?client_id=${encodeURIComponent(clientId)}`,
      { signal },
    );
    if (res.status === 401) return { kind: "unauthorized" };
    const json = await res.json().catch(() => null);
    if (!res.ok || !json || json.code !== 0) {
      return {
        kind: "error",
        status: res.status,
        message: json?.msg || `poll failed: ${res.status}`,
      };
    }
    const requests = (json.data?.requests ?? []) as RemoteToolRequest[];
    return { kind: "ok", requests };
  } catch (e: any) {
    return { kind: "error", message: e?.message || String(e) };
  }
}

/** 回传工具执行结果 */
export async function postToolResult(body: {
  request_id: string;
  content: string;
  is_error: boolean;
}) {
  return apiRequest<null>("/api/client/tool-result", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** 查询本用户的 client 列表及在线状态 */
export async function getOnlineClients() {
  return apiRequest<{ clients: OnlineClient[] }>("/api/client/online");
}
