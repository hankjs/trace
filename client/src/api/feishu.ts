import { apiRequest } from "../composables/useSession";

export interface FeishuBindCode {
  code: string;
  expires_at: number; // 毫秒时间戳
}

export interface FeishuBinding {
  id: string;
  account_id: string;
  open_id: string; // 已脱敏，如 "ou***"
  created_at: string;
}

export async function createFeishuBindCode() {
  return apiRequest<FeishuBindCode>("/api/feishu/bind-code", { method: "POST" });
}

export async function getFeishuBinding() {
  return apiRequest<FeishuBinding | null>("/api/feishu/binding");
}

export async function unbindFeishu() {
  return apiRequest<null>("/api/feishu/binding", { method: "DELETE" });
}
