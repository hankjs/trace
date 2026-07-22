//! 微信模块 HTTP 接口：admin（bot 账号管理）+ client（绑定管理）。

use crate::auth::Claims;
use crate::response::{self as R};
use crate::weixin::{api, login, monitor, router};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ─── Admin ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct LoginStartResponse {
    pub login_id: String,
    pub qrcode_url: String,
}

/// POST /api/admin/weixin/login — 发起扫码登录
pub async fn create_login(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match login::start(&state).await {
        Ok((login_id, qrcode_url)) => R::ok(LoginStartResponse {
            login_id,
            qrcode_url,
        }),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/admin/weixin/login/{login_id} — 轮询登录状态（服务端代做一次长轮询）
pub async fn get_login(
    State(state): State<Arc<AppState>>,
    Path(login_id): Path<String>,
) -> impl IntoResponse {
    match login::poll(&state, &login_id).await {
        Ok(status) => R::ok(status),
        Err(e) => R::not_found(e.to_string()),
    }
}

/// GET /api/admin/weixin/accounts
pub async fn list_accounts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_weixin_accounts().await {
        Ok(accounts) => R::ok(accounts),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateAccountRequest {
    pub enabled: bool,
}

/// PATCH /api/admin/weixin/accounts/{id} — 启用/停用账号（启用时重启 monitor）
pub async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAccountRequest>,
) -> impl IntoResponse {
    if let Err(e) = state.db.set_weixin_account_enabled(&id, body.enabled).await {
        return R::internal_error(e);
    }
    if body.enabled {
        match state.db.get_weixin_account(&id).await {
            Ok(Some(account)) => monitor::spawn_monitor(state.clone(), account),
            Ok(None) => return R::not_found("account not found"),
            Err(e) => return R::internal_error(e),
        }
    } else {
        monitor::stop_monitor(&state, &id).await;
    }
    R::no_content()
}

/// DELETE /api/admin/weixin/accounts/{id} — 停 monitor + 删库
pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    monitor::stop_monitor(&state, &id).await;
    match state.db.delete_weixin_account(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/admin/weixin/bindings
pub async fn list_bindings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_weixin_bindings().await {
        Ok(bindings) => R::ok(bindings),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub binding_id: String,
    pub text: String,
}

/// POST /api/admin/weixin/send — 主动给已绑定用户发微信消息。
/// 依赖 binding 里最近一次入站消息刷新的 context_token；没有则无法发送。
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let text = body.text.trim();
    if text.is_empty() {
        return R::bad_request("text is empty");
    }
    let binding = match state.db.get_weixin_binding_by_id(&body.binding_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return R::not_found("binding not found"),
        Err(e) => return R::internal_error(e),
    };
    let context_token = match binding.context_token.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return R::bad_request("该用户还没有给机器人发过消息，无法主动发送"),
    };
    let account = match state.db.get_weixin_account(&binding.account_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return R::not_found("account not found"),
        Err(e) => return R::internal_error(e),
    };
    let client = api::IlinkClient::new();
    match client
        .send_text(&account, &binding.ilink_user_id, &context_token, text)
        .await
    {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

/// DELETE /api/admin/weixin/bindings/{id}
pub async fn delete_binding_admin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_weixin_binding(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// ─── Client ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct BindCodeResponse {
    pub code: String,
    pub expires_at: i64,
}

/// POST /api/weixin/bind-code — 生成 6 位数字绑定码（10 分钟有效）
pub async fn create_bind_code(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    // 同用户旧码不再可用：直接生成新码即可（consume 后旧码自动失效），
    // 这里不做额外清理，DB 中旧码过期后自然不可用。
    let code = random_six_digits();
    let expires_at = router::bind_code_expires_at();
    match state
        .db
        .create_weixin_bind_code(&code, &claims.sub, expires_at)
        .await
    {
        Ok(()) => R::ok(BindCodeResponse { code, expires_at }),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Serialize)]
pub struct BindingResponse {
    pub id: String,
    pub account_id: String,
    /// 脱敏后的微信用户标识
    pub ilink_user_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/weixin/binding — 当前绑定
pub async fn get_binding(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.get_weixin_binding_by_user(&claims.sub).await {
        Ok(Some(b)) => R::ok(BindingResponse {
            id: b.id,
            account_id: b.account_id,
            ilink_user_id: mask(&b.ilink_user_id),
            created_at: b.created_at,
        }),
        Ok(None) => R::not_found("not bound"),
        Err(e) => R::internal_error(e),
    }
}

/// DELETE /api/weixin/binding — 解绑
pub async fn delete_binding(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.get_weixin_binding_by_user(&claims.sub).await {
        Ok(Some(b)) => match state.db.delete_weixin_binding(&b.id).await {
            Ok(()) => R::no_content(),
            Err(e) => R::internal_error(e),
        },
        Ok(None) => R::not_found("not bound"),
        Err(e) => R::internal_error(e),
    }
}

/// 6 位数字码，随机数取自 uuid（不新增依赖）。
fn random_six_digits() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    format!("{:06}", n % 1_000_000)
}

/// 脱敏：保留前 2 位，其余打码。
fn mask(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 2 {
        return "*".repeat(chars.len());
    }
    let prefix: String = chars[..2].iter().collect();
    format!("{prefix}***")
}
