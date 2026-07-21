import { apiRequest } from "../composables/useSession";

export interface WeixinBindCode {
  code: string;
  expires_at: number; // 毫秒时间戳
}

export interface WeixinBinding {
  id: string;
  ilink_user_id: string; // 已脱敏，如 "ab***"
  created_at: string;
}

export async function createBindCode() {
  return apiRequest<WeixinBindCode>("/api/weixin/bind-code", { method: "POST" });
}

export async function getBinding() {
  return apiRequest<WeixinBinding | null>("/api/weixin/binding");
}

export async function unbind() {
  return apiRequest<null>("/api/weixin/binding", { method: "DELETE" });
}
