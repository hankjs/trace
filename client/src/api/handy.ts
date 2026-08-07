import { apiRequest } from "../composables/useSession";

export interface HandyAccount {
  base_url: string;
  token: string; // 掩码：已配置为 "********"，未配置为空串
  webhook_secret: string; // 同上
  enabled: boolean;
  webhook_url: string; // handy 侧建凭证时要填的回推地址（服务端算好）
  created_at: string;
  updated_at: string;
}

export interface PutHandyAccountBody {
  base_url: string;
  token?: string; // 空串 = 保留旧值（首次保存必填）
  webhook_secret?: string; // 同上
  enabled?: boolean;
}

export interface HandyTestResult {
  ok: boolean;
  token_name?: string;
  webhook_configured?: boolean;
  error?: string;
}

export async function getHandyAccount() {
  return apiRequest<HandyAccount>("/api/handy/account");
}

export async function putHandyAccount(body: PutHandyAccountBody) {
  return apiRequest<HandyAccount>("/api/handy/account", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function testHandyAccount(body: { base_url?: string; token?: string } = {}) {
  return apiRequest<HandyTestResult>("/api/handy/account/test", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}
