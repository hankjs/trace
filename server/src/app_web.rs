//! App 产品 API：用户作用域的远程终端代理 + WebRTC 信令。
//!
//! 普通 JWT（client scope）即可；每个 client 必须归属当前用户。
//! 转发链：app 前端 → 本模块 → remote_term::dispatch_tool_call → 桌面 client。

use crate::auth::Claims;
use crate::remote_term;
use crate::response::{self as R};
use crate::turn;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const TERM_TIMEOUT: Duration = Duration::from_secs(15);
const RTC_SIGNAL_TIMEOUT: Duration = Duration::from_secs(20);

/// 校验 client 归属当前用户；不存在或不属于自己一律 404。
async fn owned_agent(
    state: &AppState,
    user_id: &str,
    client_id: &str,
) -> Result<hank_db::ClientAgent, axum::response::Response> {
    let agent = state
        .db
        .get_client_agent_by_id(client_id)
        .await
        .map_err(R::internal_error)?
        .ok_or_else(|| R::not_found("client not found"))?;
    if agent.user_id != user_id {
        return Err(R::not_found("client not found"));
    }
    Ok(agent)
}

/// 向归属当前用户的 client 派发工具调用。
async fn dispatch(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    tool: &str,
    input: serde_json::Value,
    timeout: Duration,
) -> Result<String, axum::response::Response> {
    let agent = owned_agent(state, user_id, client_id).await?;
    if !remote_term::is_client_online(state, &agent.user_id, &agent.id).await {
        return Err(R::bad_request("节点不在线或未开启远程终端"));
    }
    match remote_term::dispatch_tool_call(
        state,
        &agent.user_id,
        &agent.id,
        tool,
        input,
        timeout,
    )
    .await
    {
        Ok(r) if !r.is_error => Ok(r.content),
        Ok(r) => Err(R::bad_request(r.content)),
        Err(e) => Err(R::bad_request(e.to_string())),
    }
}

/// GET /api/app/clients — 当前用户的 client 列表（含在线状态）
pub async fn list_clients(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    let clients = match state.db.list_client_agents(&claims.sub).await {
        Ok(c) => c,
        Err(e) => return R::internal_error(e),
    };
    let mut out = Vec::with_capacity(clients.len());
    for c in clients {
        let online = remote_term::is_client_online(&state, &claims.sub, &c.id).await;
        out.push(serde_json::json!({
            "id": c.id,
            "hostname": c.hostname,
            "work_dir": c.work_dir,
            "accept_remote": c.accept_remote,
            "enabled": c.enabled,
            "last_active_at": c.last_active_at,
            "last_seen_at": c.last_seen_at,
            "online": online,
        }));
    }
    R::ok(serde_json::json!({ "clients": out }))
}

#[derive(Deserialize)]
pub struct ClientEnabledBody {
    enabled: bool,
}

/// POST /api/app/clients/{cid}/enabled
pub async fn set_client_enabled(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(cid): Path<String>,
    Json(body): Json<ClientEnabledBody>,
) -> impl IntoResponse {
    if let Err(resp) = owned_agent(&state, &claims.sub, &cid).await {
        return resp;
    }
    match state.db.set_client_agent_enabled(&cid, body.enabled).await {
        Ok(()) => R::ok(serde_json::json!({ "id": cid, "enabled": body.enabled })),
        Err(e) => R::internal_error(e),
    }
}

/// DELETE /api/app/clients/{cid} — 删除节点登记（在线 client 下次注册会再出现）
pub async fn delete_client(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(cid): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = owned_agent(&state, &claims.sub, &cid).await {
        return resp;
    }
    match state.db.delete_client_agent(&claims.sub, &cid).await {
        Ok(true) => {
            // 清内存 hub 在线观测，避免幽灵 online
            let mut hubs = state.client_hubs.write().await;
            if let Some(hub) = hubs.get_mut(&claims.sub) {
                hub.last_polls.remove(&cid);
                hub.pending.retain(|r| r.client_id != cid);
            }
            R::ok(serde_json::json!({ "id": cid, "deleted": true }))
        }
        Ok(false) => R::not_found("client not found"),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/app/clients/{cid}/terminals
pub async fn list_terminals(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(cid): Path<String>,
) -> impl IntoResponse {
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_list",
        serde_json::json!({}),
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => R::ok(v),
            Err(_) => R::ok(serde_json::json!([])),
        },
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct CreateTerminalBody {
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// POST /api/app/clients/{cid}/terminals
pub async fn create_terminal(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(cid): Path<String>,
    Json(body): Json<CreateTerminalBody>,
) -> impl IntoResponse {
    let mut input = serde_json::json!({
        "cols": body.cols.unwrap_or(120),
        "rows": body.rows.unwrap_or(30),
    });
    if let Some(cwd) = body.cwd.filter(|s| !s.trim().is_empty()) {
        input["cwd"] = serde_json::Value::String(cwd);
    }
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_create",
        input,
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => R::ok(serde_json::json!({ "terminal": v })),
            Err(_) => R::ok(serde_json::json!({ "terminal": content })),
        },
        Err(resp) => resp,
    }
}

/// DELETE /api/app/clients/{cid}/terminals/{tid}
pub async fn close_terminal(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((cid, tid)): Path<(String, String)>,
) -> impl IntoResponse {
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_close",
        serde_json::json!({ "id": tid }),
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(_) => R::ok(serde_json::json!({ "closed": true })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct OutputQuery {
    lines: Option<usize>,
    raw: Option<bool>,
}

/// GET /api/app/clients/{cid}/terminals/{tid}/output?lines=N&raw=1
pub async fn terminal_output(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((cid, tid)): Path<(String, String)>,
    Query(q): Query<OutputQuery>,
) -> impl IntoResponse {
    let input = if q.raw.unwrap_or(false) {
        serde_json::json!({ "id": tid, "raw": true })
    } else {
        serde_json::json!({ "id": tid, "lines": q.lines.unwrap_or(200) })
    };
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_read",
        input,
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(content) => R::ok(serde_json::json!({ "output": content })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct InputBody {
    data: String,
}

/// POST /api/app/clients/{cid}/terminals/{tid}/input
pub async fn terminal_input(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((cid, tid)): Path<(String, String)>,
    Json(body): Json<InputBody>,
) -> impl IntoResponse {
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_write",
        serde_json::json!({ "id": tid, "data": body.data }),
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(_) => R::ok(serde_json::json!({ "sent": true })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct ResizeBody {
    cols: u16,
    rows: u16,
}

/// POST /api/app/clients/{cid}/terminals/{tid}/resize
pub async fn terminal_resize(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((cid, tid)): Path<(String, String)>,
    Json(body): Json<ResizeBody>,
) -> impl IntoResponse {
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "terminal_resize",
        serde_json::json!({ "id": tid, "cols": body.cols, "rows": body.rows }),
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(_) => R::ok(serde_json::json!({ "resized": true })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct RtcOfferBody {
    sdp: String,
}

/// POST /api/app/clients/{cid}/rtc/offer — WebRTC 信令（non-trickle SDP）
pub async fn rtc_offer(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(cid): Path<String>,
    Json(body): Json<RtcOfferBody>,
) -> impl IntoResponse {
    if body.sdp.trim().is_empty() {
        return R::bad_request("sdp 不能为空");
    }
    match dispatch(
        &state,
        &claims.sub,
        &cid,
        "rtc_signal",
        serde_json::json!({ "sdp": body.sdp }),
        RTC_SIGNAL_TIMEOUT,
    )
    .await
    {
        Ok(answer_sdp) => R::ok(serde_json::json!({
            "type": "answer",
            "sdp": answer_sdp,
        })),
        Err(resp) => resp,
    }
}

/// GET /api/app/rtc/ice — ICE/TURN 凭据
pub async fn rtc_ice(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    R::ok(turn::ice_servers(&state.config.turn, &claims.sub))
}
