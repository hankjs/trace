//! Admin 终端代理 API：查看/操作任意在线 client 的终端会话。
//! 转发链：admin 前端 → 本模块 → remote_exec::dispatch_tool_call → client。

use crate::remote_exec;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const TERM_TIMEOUT: Duration = Duration::from_secs(15);

/// GET /api/admin/clients — 全部 client agent（含在线状态）
pub async fn list_clients(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_all_client_agents().await {
        Ok(agents) => {
            let mut out = Vec::with_capacity(agents.len());
            for a in agents {
                let online = remote_exec::is_client_online(&state, &a.user_id, &a.id).await;
                out.push(serde_json::json!({
                    "id": a.id,
                    "user_id": a.user_id,
                    "hostname": a.hostname,
                    "work_dir": a.work_dir,
                    "accept_remote": a.accept_remote,
                    "online": online,
                }));
            }
            R::ok(out)
        }
        Err(e) => R::internal_error(e),
    }
}

/// 向指定 client 派发一条 terminal_* 工具调用；Err 为可直接返回的错误响应
async fn dispatch(
    state: &AppState,
    client_id: &str,
    tool: &str,
    input: serde_json::Value,
) -> Result<String, axum::response::Response> {
    let agent = state
        .db
        .get_client_agent_by_id(client_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| R::not_found("client not found"))?;
    if !remote_exec::is_client_online(state, &agent.user_id, &agent.id).await {
        return Err(R::bad_request("client 不在线或未开启远程执行"));
    }
    match remote_exec::dispatch_tool_call(
        state,
        &agent.user_id,
        &agent.id,
        tool,
        input,
        TERM_TIMEOUT,
    )
    .await
    {
        Ok(r) if !r.is_error => Ok(r.content),
        Ok(r) => Err(R::bad_request(r.content)),
        Err(e) => Err(R::bad_request(e.to_string())),
    }
}

/// GET /api/admin/clients/{cid}/terminals — 该 client 的终端会话列表
pub async fn list_terminals(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
) -> impl IntoResponse {
    match dispatch(&state, &cid, "terminal_list", serde_json::json!({})).await {
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

/// GET /api/admin/clients/{cid}/terminals/{tid}/output?lines=N&raw=1 — 终端输出尾部
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
    match dispatch(&state, &cid, "terminal_read", input).await {
        Ok(content) => R::ok(serde_json::json!({ "output": content })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct NotifyQuery {
    limit: Option<u32>,
}

/// GET /api/admin/notifications?limit=N — 当前 admin 用户的终端通知（新到旧）
pub async fn list_notifications(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<crate::auth::Claims>,
    Query(q): Query<NotifyQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).min(500);
    match state.db.list_client_notifications(&claims.sub, limit).await {
        Ok(rows) => R::ok(rows),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct InputBody {
    data: String,
}

/// POST /api/admin/clients/{cid}/terminals/{tid}/input — 向终端发送输入（原样发送，不补回车）
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
    )
    .await
    {
        Ok(_) => R::ok(serde_json::json!({ "sent": true })),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
pub struct EnabledBody {
    enabled: bool,
}

/// POST /api/admin/clients/{cid}/terminals/{tid}/enabled — 停用/启用终端会话
pub async fn terminal_set_enabled(
    State(state): State<Arc<AppState>>,
    Path((cid, tid)): Path<(String, String)>,
    Json(body): Json<EnabledBody>,
) -> impl IntoResponse {
    match dispatch(
        &state,
        &cid,
        "terminal_set_enabled",
        serde_json::json!({ "id": tid, "enabled": body.enabled }),
    )
    .await
    {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => R::ok(v),
            Err(_) => R::ok(serde_json::json!({ "enabled": body.enabled })),
        },
        Err(resp) => resp,
    }
}
