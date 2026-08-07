//! handy 连接配置的 client 级 REST：用户管理自己的 handy 对接。
//!
//! 与飞书/微信的 admin 级账号管理不同：handy 是用户自己的对端，
//! 凭证（base_url + hnk_ token + webhook_secret）按用户存 handy_accounts 表。
//! 安全口径：GET 掩码 token/secret 不回显；PUT 空串 = 保留旧值。

use crate::auth::Claims;
use crate::handy::client::HandyApi;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// GET 出参：token/webhook_secret 掩码（已配置返回固定掩码串，未配置空串）。
/// webhook_url 是算好的回推地址，用户在 handy 建 token 时直接照抄。
#[derive(Debug, Serialize)]
pub struct AccountView {
    pub base_url: String,
    pub token: String,
    pub webhook_secret: String,
    pub enabled: bool,
    pub webhook_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// 掩码：不回显任何明文片段。已配置 → 固定掩码；未配置 → 空串
/// （前端据此区分「已存凭证」与「未填」）。
fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        "********".to_string()
    }
}

/// base_url 规范化：去首尾空白与尾部斜杠，必须 http(s)。
/// 入库与建 client 前都过这道，避免双斜杠与裸主机名。
fn normalize_base_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err("base_url 不能为空".to_string());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("base_url 必须以 http:// 或 https:// 开头".to_string());
    }
    Ok(url.to_string())
}

/// handy 侧配置 token 时要填的回推地址（纯拼接，单测用）。
fn build_webhook_url(base: &str, user_id: &str) -> String {
    format!(
        "{}/api/channels/handy/{}/webhook",
        base.trim_end_matches('/'),
        user_id
    )
}

/// 算 webhook_url 的 base：优先 config 里显式配置的对外地址（admin_base_url，
/// admin SPA 就由本服务挂在 /admin 下，它就是本服务的对外基址）；
/// 未配置时退回请求 Host（本地 dev 兜底，反代后可能是内网地址，仅作展示）。
fn webhook_base_url(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if let Some(base) = state
        .config
        .server
        .admin_base_url
        .as_ref()
        .filter(|u| !u.trim().is_empty())
    {
        return Some(base.trim().trim_end_matches('/').to_string());
    }
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))?
        .to_str()
        .ok()?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    Some(format!("{proto}://{host}"))
}

fn to_view(
    state: &AppState,
    headers: &HeaderMap,
    account: &hank_db::HandyAccount,
) -> AccountView {
    let webhook_url = webhook_base_url(state, headers)
        .map(|base| build_webhook_url(&base, &account.user_id))
        .unwrap_or_default();
    AccountView {
        base_url: account.base_url.clone(),
        token: mask_secret(&account.token),
        webhook_secret: mask_secret(&account.webhook_secret),
        enabled: account.enabled,
        webhook_url,
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

/// GET /api/handy/account — 当前用户的 handy 连接配置（掩码版）。
pub async fn get_account(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match state.db.get_handy_account(&claims.sub).await {
        Ok(Some(account)) => R::ok(to_view(&state, &headers, &account)),
        Ok(None) => R::not_found("handy 未配置"),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutAccountBody {
    pub base_url: String,
    /// 空串 = 保留旧值（首次保存时必填）
    #[serde(default)]
    pub token: String,
    /// 空串 = 保留旧值（首次保存时必填）
    #[serde(default)]
    pub webhook_secret: String,
    /// None = 保留旧值（首次保存默认启用）
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// PUT /api/handy/account — 保存当前用户的 handy 连接配置（upsert）。
pub async fn put_account(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
    Json(body): Json<PutAccountBody>,
) -> impl IntoResponse {
    let base_url = match normalize_base_url(&body.base_url) {
        Ok(u) => u,
        Err(msg) => return R::bad_request(msg),
    };
    let existing = match state.db.get_handy_account(&claims.sub).await {
        Ok(v) => v,
        Err(e) => return R::internal_error(e),
    };
    // 空串 = 保留旧值；首次保存时 token/secret 必填
    let token = if body.token.is_empty() {
        match existing.as_ref().map(|a| a.token.clone()) {
            Some(t) => t,
            None => return R::bad_request("首次保存必须填写 token"),
        }
    } else {
        body.token.trim().to_string()
    };
    let webhook_secret = if body.webhook_secret.is_empty() {
        match existing.as_ref().map(|a| a.webhook_secret.clone()) {
            Some(s) => s,
            None => return R::bad_request("首次保存必须填写 webhook_secret"),
        }
    } else {
        body.webhook_secret.trim().to_string()
    };
    let enabled = body
        .enabled
        .or_else(|| existing.as_ref().map(|a| a.enabled))
        .unwrap_or(true);
    if let Err(e) = state
        .db
        .upsert_handy_account(&claims.sub, &base_url, &token, &webhook_secret, enabled)
        .await
    {
        return R::internal_error(e);
    }
    match state.db.get_handy_account(&claims.sub).await {
        Ok(Some(account)) => R::ok(to_view(&state, &headers, &account)),
        Ok(None) => R::internal_error("handy 配置保存后读取失败"),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TestAccountBody {
    /// 空 = 用已存的 base_url（待保存的新值优先）
    #[serde(default)]
    pub base_url: String,
    /// 空 = 用已存的 token
    #[serde(default)]
    pub token: String,
}

/// POST /api/handy/account/test — 用待保存（或已存）凭证调 handy whoami 自检。
/// 连通结果走 200 + {ok:bool} 返回，业务失败不当 HTTP 错误（前端直接展示原因）。
pub async fn test_account(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<TestAccountBody>,
) -> impl IntoResponse {
    let existing = match state.db.get_handy_account(&claims.sub).await {
        Ok(v) => v,
        Err(e) => return R::internal_error(e),
    };
    let raw_base = if body.base_url.is_empty() {
        existing.as_ref().map(|a| a.base_url.clone()).unwrap_or_default()
    } else {
        body.base_url.clone()
    };
    let base_url = match normalize_base_url(&raw_base) {
        Ok(u) => u,
        Err(msg) => return R::bad_request(msg),
    };
    let token = if body.token.is_empty() {
        match existing.as_ref().map(|a| a.token.clone()) {
            Some(t) => t,
            None => return R::bad_request("没有可用的 token（请先保存或在请求里带上）"),
        }
    } else {
        body.token.trim().to_string()
    };

    let api = HandyApi::new(&base_url, &token);
    match api.whoami().await {
        Ok(data) => R::ok(serde_json::json!({
            "ok": true,
            "token_name": data["token_name"].as_str().unwrap_or(""),
            "webhook_configured": data["webhook_configured"].as_bool().unwrap_or(false),
        })),
        Err(e) => R::ok(serde_json::json!({
            "ok": false,
            "error": format!("{e:#}"),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_never_echoes_plaintext() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("hnk_abcdef123456"), "********");
        // 掩码不含原文任何片段
        assert!(!mask_secret("hnk_abcdef123456").contains("hnk"));
    }

    #[test]
    fn normalize_base_url_trims_and_validates() {
        assert_eq!(
            normalize_base_url(" https://handy.example.com/ ").unwrap(),
            "https://handy.example.com"
        );
        assert_eq!(
            normalize_base_url("http://127.0.0.1:8300//").unwrap(),
            "http://127.0.0.1:8300"
        );
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("   ").is_err());
        assert!(normalize_base_url("handy.example.com").is_err());
        assert!(normalize_base_url("ftp://x").is_err());
    }

    #[test]
    fn webhook_url_is_per_user_and_trims_base() {
        assert_eq!(
            build_webhook_url("https://trace.example.com/", "u-1"),
            "https://trace.example.com/api/channels/handy/u-1/webhook"
        );
        assert!(!build_webhook_url("https://x/", "u").contains("//api"));
    }
}
