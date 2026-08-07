//! client JWT 保护的注册 / 长轮询 / 结果回传。

use super::hub::{ToolCallResult, UserHub, ONLINE_WINDOW};
use crate::auth::Claims;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 长轮询单次挂起时长
const POLL_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Deserialize)]
pub struct RegistrationRequest {
    pub client_id: String,
    pub hostname: Option<String>,
    pub work_dir: Option<String>,
    pub accept_remote: bool,
}

/// PUT /api/client/registration
pub async fn register_client(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<RegistrationRequest>,
) -> impl IntoResponse {
    if body.client_id.trim().is_empty() {
        return R::bad_request("client_id is required");
    }
    match state
        .db
        .upsert_client_agent(
            &body.client_id,
            &claims.sub,
            body.hostname.as_deref(),
            body.work_dir.as_deref(),
            body.accept_remote,
        )
        .await
    {
        Ok(()) => R::ok(serde_json::json!({ "client_id": body.client_id })),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct PollQuery {
    pub client_id: String,
}

/// GET /api/client/poll?client_id=
/// 挂起最长 25s；每次调用（含超时）都刷新该 client 在线时间。
pub async fn poll_requests(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<PollQuery>,
) -> impl IntoResponse {
    let user_id = claims.sub;
    let client_id = query.client_id;
    if client_id.trim().is_empty() {
        return R::bad_request("client_id is required");
    }

    let _ = state.db.touch_client_agent_seen(&client_id).await;

    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let (notify, requests) = {
            let mut hubs = state.client_hubs.write().await;
            let hub = hubs.entry(user_id.clone()).or_default();
            let requests = hub.drain_for(&client_id);
            (hub.notify_handle(), requests)
        };
        if !requests.is_empty() {
            return R::ok(serde_json::json!({ "requests": requests }));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return R::ok(serde_json::json!({ "requests": [] }));
        }
        let _ = tokio::time::timeout(remaining, notify.notified()).await;
    }
}

#[derive(Deserialize)]
pub struct ToolResultRequest {
    pub request_id: String,
    pub content: String,
    pub is_error: bool,
}

/// POST /api/client/tool-result
pub async fn post_tool_result(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<ToolResultRequest>,
) -> impl IntoResponse {
    let sender = {
        let mut hubs = state.client_hubs.write().await;
        hubs.get_mut(&claims.sub)
            .and_then(|hub| hub.inflight.remove(&body.request_id))
    };
    match sender {
        Some(tx) => {
            let _ = tx.send(ToolCallResult {
                content: body.content,
                is_error: body.is_error,
            });
            R::ok(serde_json::json!({ "request_id": body.request_id }))
        }
        None => R::bad_request(format!("unknown request_id: {}", body.request_id)),
    }
}

/// GET /api/client/online — 当前用户注册过的 client 及在线状态
pub async fn list_online(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    let clients = match state.db.list_client_agents(&claims.sub).await {
        Ok(c) => c,
        Err(e) => return R::internal_error(e),
    };
    let hubs = state.client_hubs.read().await;
    let empty = UserHub::default();
    let hub = hubs.get(&claims.sub).unwrap_or(&empty);
    let list: Vec<Value> = clients
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "client_id": c.id,
                "hostname": c.hostname,
                "work_dir": c.work_dir,
                "accept_remote": c.accept_remote,
                "enabled": c.enabled,
                "last_active_at": c.last_active_at,
                "last_seen_at": c.last_seen_at,
                "online": hub.is_online(&c.id),
            })
        })
        .collect();
    R::ok(serde_json::json!({ "clients": list }))
}

// 抑制未使用警告：ONLINE_WINDOW 在 is_online 内使用，对外文档用
#[allow(dead_code)]
fn _online_window() -> Duration {
    ONLINE_WINDOW
}
