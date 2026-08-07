//! Admin 终端代理 API：查看/操作任意在线 client 的终端会话 + WebRTC 信令。
//! 转发链：admin 前端 → 本模块 → remote_term::dispatch_tool_call → client。

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

/// GET /api/admin/clients — 全部 client agent（含在线状态）
pub async fn list_clients(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_all_client_agents().await {
        Ok(agents) => {
            let mut out = Vec::with_capacity(agents.len());
            for a in agents {
                let online = remote_term::is_client_online(&state, &a.user_id, &a.id).await;
                out.push(serde_json::json!({
                    "id": a.id,
                    "user_id": a.user_id,
                    "hostname": a.hostname,
                    "work_dir": a.work_dir,
                    "accept_remote": a.accept_remote,
                    "enabled": a.enabled,
                    "last_active_at": a.last_active_at,
                    "last_seen_at": a.last_seen_at,
                    "online": online,
                }));
            }
            R::ok(out)
        }
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct ClientEnabledBody {
    enabled: bool,
}

/// POST /api/admin/clients/{cid}/enabled
pub async fn set_client_enabled(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
    Json(body): Json<ClientEnabledBody>,
) -> impl IntoResponse {
    match state.db.get_client_agent_by_id(&cid).await {
        Ok(Some(_)) => {}
        Ok(None) => return R::not_found("client not found"),
        Err(e) => return R::internal_error(e),
    }
    match state.db.set_client_agent_enabled(&cid, body.enabled).await {
        Ok(()) => R::ok(serde_json::json!({ "id": cid, "enabled": body.enabled })),
        Err(e) => R::internal_error(e),
    }
}

/// 向指定 client 派发工具调用；Err 为可直接返回的错误响应
async fn dispatch(
    state: &AppState,
    client_id: &str,
    tool: &str,
    input: serde_json::Value,
    timeout: Duration,
) -> Result<String, axum::response::Response> {
    let agent = state
        .db
        .get_client_agent_by_id(client_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| R::not_found("client not found"))?;
    if !remote_term::is_client_online(state, &agent.user_id, &agent.id).await {
        return Err(R::bad_request("client 不在线或未开启远程终端"));
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

/// GET /api/admin/clients/{cid}/terminals
pub async fn list_terminals(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
) -> impl IntoResponse {
    match dispatch(
        &state,
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
pub struct OutputQuery {
    lines: Option<usize>,
    raw: Option<bool>,
}

/// GET /api/admin/clients/{cid}/terminals/{tid}/output?lines=N&raw=1
pub async fn terminal_output(
    State(state): State<Arc<AppState>>,
    Path((cid, tid)): Path<(String, String)>,
    Query(q): Query<OutputQuery>,
) -> impl IntoResponse {
    let input = if q.raw.unwrap_or(false) {
        serde_json::json!({ "id": tid, "raw": true })
    } else {
        serde_json::json!({ "id": tid, "lines": q.lines.unwrap_or(200) })
    };
    match dispatch(&state, &cid, "terminal_read", input, TERM_TIMEOUT).await {
        Ok(content) => R::ok(serde_json::json!({ "output": content })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct InputBody {
    data: String,
}

/// POST /api/admin/clients/{cid}/terminals/{tid}/input
pub async fn terminal_input(
    State(state): State<Arc<AppState>>,
    Path((cid, tid)): Path<(String, String)>,
    Json(body): Json<InputBody>,
) -> impl IntoResponse {
    match dispatch(
        &state,
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
pub struct RtcOfferBody {
    sdp: String,
}

/// POST /api/admin/clients/{cid}/rtc/offer — WebRTC 信令（non-trickle SDP）
pub async fn rtc_offer(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
    Json(body): Json<RtcOfferBody>,
) -> impl IntoResponse {
    if body.sdp.trim().is_empty() {
        return R::bad_request("sdp 不能为空");
    }
    match dispatch(
        &state,
        &cid,
        "rtc_signal",
        serde_json::json!({ "sdp": body.sdp }),
        RTC_SIGNAL_TIMEOUT,
    )
    .await
    {
        // client 回 answer SDP 字符串；浏览器 setRemoteDescription 需要 {type,sdp}
        Ok(answer_sdp) => R::ok(serde_json::json!({
            "type": "answer",
            "sdp": answer_sdp,
        })),
        Err(resp) => resp,
    }
}

/// GET /api/admin/rtc/ice — ICE/TURN 凭据（admin JWT）
pub async fn rtc_ice(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    R::ok(turn::ice_servers(&state.config.turn, &claims.sub))
}
