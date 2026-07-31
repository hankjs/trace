//! 飞书模块 HTTP 接口：admin（应用账号与绑定管理）+ client（绑定管理）。
//!
//! 与 weixin/routes.rs 同一模式：账号凭证入 DB，启停跟随 monitor；
//! 用户绑定走一次性 6 位绑定码（admin/client 生成 → 飞书里发给 bot）。

use crate::auth::Claims;
use crate::feishu::{api::FeishuApi, monitor};
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;

// ─── Admin ─────────────────────────────────────────────────────────

/// GET /api/admin/feishu/accounts
pub async fn list_accounts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_feishu_accounts().await {
        Ok(accounts) => R::ok(accounts),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateAccountRequest {
    pub name: Option<String>,
    pub app_id: String,
    pub app_secret: String,
}

/// POST /api/admin/feishu/accounts — 添加自建应用（先验证凭证再落库），成功后启动长连接
pub async fn create_account(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateAccountRequest>,
) -> impl IntoResponse {
    let app_id = body.app_id.trim();
    let app_secret = body.app_secret.trim();
    if app_id.is_empty() || app_secret.is_empty() {
        return R::bad_request("app_id / app_secret 不能为空");
    }
    // 先拿一次 tenant_access_token 验证凭证有效，避免存下错误凭证后 monitor 空转
    if let Err(e) = FeishuApi::verify_credentials(app_id, app_secret).await {
        return R::bad_request(format!("飞书凭证校验失败：{e:#}"));
    }
    let name = body.name.unwrap_or_default();
    let id = match state.db.create_feishu_account(&name, app_id, app_secret).await {
        Ok(id) => id,
        Err(e) => return R::internal_error(e),
    };
    match state.db.get_feishu_account(&id).await {
        Ok(Some(account)) => monitor::spawn_monitor(state.clone(), account),
        Ok(None) => return R::not_found("account not found after create"),
        Err(e) => return R::internal_error(e),
    }
    R::ok(serde_json::json!({ "id": id }))
}

#[derive(Deserialize)]
pub struct UpdateAccountRequest {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    /// 换凭证（轮转 app_secret）；提供时重启长连接
    pub app_secret: Option<String>,
}

/// PATCH /api/admin/feishu/accounts/{id} — 启用/停用/改名/换 secret（按需重启 monitor）
pub async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAccountRequest>,
) -> impl IntoResponse {
    let account = match state.db.get_feishu_account(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return R::not_found("account not found"),
        Err(e) => return R::internal_error(e),
    };

    // 改名/换 secret（name 缺省保持原值）
    if body.name.is_some() || body.app_secret.is_some() {
        let name = body.name.clone().unwrap_or_else(|| account.name.clone());
        if let Err(e) = state
            .db
            .update_feishu_account(&id, &name, body.app_secret.as_deref())
            .await
        {
            return R::internal_error(e);
        }
    }
    if let Some(enabled) = body.enabled {
        if let Err(e) = state.db.set_feishu_account_enabled(&id, enabled).await {
            return R::internal_error(e);
        }
    }

    // secret 变更或 enabled 变更都统一按最新状态重启/停止 monitor
    let latest = match state.db.get_feishu_account(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return R::not_found("account not found"),
        Err(e) => return R::internal_error(e),
    };
    if latest.enabled {
        monitor::spawn_monitor(state.clone(), latest);
    } else {
        monitor::stop_monitor(&state, &id).await;
    }
    R::no_content()
}

/// DELETE /api/admin/feishu/accounts/{id} — 停 monitor + 删库（绑定/话题映射级联删除）
pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    monitor::stop_monitor(&state, &id).await;
    match state.db.delete_feishu_account(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/admin/feishu/bindings
pub async fn list_bindings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_feishu_bindings().await {
        Ok(bindings) => R::ok(bindings),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct AdminCreateBindCodeRequest {
    pub user_id: String,
}

/// POST /api/admin/feishu/bind-code — 管理员为指定 Trace 用户生成绑定码。
pub async fn create_bind_code_admin(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AdminCreateBindCodeRequest>,
) -> impl IntoResponse {
    let user_id = body.user_id.trim();
    if user_id.is_empty() {
        return R::bad_request("user_id 不能为空");
    }
    match state.db.get_user_by_id(user_id).await {
        Ok(Some(_)) => issue_bind_code(&state, user_id).await,
        Ok(None) => R::not_found("user not found"),
        Err(e) => R::internal_error(e),
    }
}

/// DELETE /api/admin/feishu/bindings/{id}
pub async fn delete_binding_admin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_feishu_binding(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub binding_id: String,
    pub text: String,
}

/// POST /api/admin/feishu/send — 主动给已绑定用户发飞书单聊消息。
/// quant 巡检/任务完成等主动推送场景也走这个通道（open_id 来自绑定记录）。
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let text = body.text.trim();
    if text.is_empty() {
        return R::bad_request("text is empty");
    }
    let binding = match state.db.get_feishu_binding_by_id(&body.binding_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return R::not_found("binding not found"),
        Err(e) => return R::internal_error(e),
    };
    let account = match state.db.get_feishu_account(&binding.account_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return R::not_found("account not found"),
        Err(e) => return R::internal_error(e),
    };
    if !account.enabled {
        return R::bad_request("该应用已停用");
    }
    let api = FeishuApi::new(&account);
    match api.send_text("open_id", &binding.open_id, text).await {
        Ok(_) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// ─── Client ────────────────────────────────────────────────────────

/// 绑定码有效期（10 分钟）
fn bind_code_expires_at() -> i64 {
    chrono::Utc::now().timestamp_millis() + 10 * 60 * 1000
}

/// POST /api/feishu/bind-code — 生成 6 位数字绑定码（10 分钟有效）
pub async fn create_bind_code(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    issue_bind_code(&state, &claims.sub).await
}

async fn issue_bind_code(state: &Arc<AppState>, user_id: &str) -> axum::response::Response {
    let code = random_six_digits();
    let expires_at = bind_code_expires_at();
    match state
        .db
        .create_feishu_bind_code(&code, user_id, expires_at)
        .await
    {
        Ok(()) => R::ok(serde_json::json!({ "code": code, "expires_at": expires_at })),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/feishu/binding — 当前绑定
pub async fn get_binding(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.get_feishu_binding_by_user(&claims.sub).await {
        Ok(Some(b)) => R::ok(serde_json::json!({
            "id": b.id,
            "account_id": b.account_id,
            "open_id": mask(&b.open_id),
            "created_at": b.created_at,
        })),
        Ok(None) => R::not_found("not bound"),
        Err(e) => R::internal_error(e),
    }
}

/// DELETE /api/feishu/binding — 解绑
pub async fn delete_binding(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.get_feishu_binding_by_user(&claims.sub).await {
        Ok(Some(b)) => match state.db.delete_feishu_binding(&b.id).await {
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
