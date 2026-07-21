//! 桌面 client 远程执行通道：client 长轮询取工具调用，POST 回传结果。
//!
//! agent loop 仍在 server 跑；绑定 exec_client_id 的会话，其 fs/shell 类工具
//! 通过本模块的内存队列下发到在线 client，在 client 本地 work_dir 执行。
//!
//! 在线判定：client 每次 poll 都会刷新对应 client_id 的 last_poll_at，
//! 60s 内有过 poll 即视为在线。

use crate::auth::Claims;
use crate::response::{self as R};
use crate::AppState;
use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use hank_db::ClientAgent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

/// 长轮询单次挂起时长
const POLL_TIMEOUT: Duration = Duration::from_secs(25);
/// 在线判定窗口：该时长内有 poll 即在线
const ONLINE_WINDOW: Duration = Duration::from_secs(60);
/// 远程调用在被代理工具超时之上追加的网络余量
pub const NETWORK_MARGIN: Duration = Duration::from_secs(10);

/// 一条待 client 执行的工具调用
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRequest {
    pub request_id: String,
    /// 目标 client（仅 server 内部路由用，不下发给 client）
    #[serde(skip)]
    pub client_id: String,
    pub tool: String,
    pub input: Value,
}

/// client 回传的工具执行结果
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub content: String,
    pub is_error: bool,
}

/// 单个用户的 client 执行通道（挂 AppState.client_hubs）
#[derive(Default)]
pub struct UserHub {
    /// 待 client 取走的工具调用队列
    pub pending: VecDeque<ToolCallRequest>,
    /// pending 入队时唤醒长轮询
    pub notify: Arc<Notify>,
    /// 已下发、等待结果的调用（request_id → 回传通道）
    pub inflight: HashMap<String, oneshot::Sender<ToolCallResult>>,
    /// 每个 client 最近一次 poll 的时间（在线判定依据）
    pub last_polls: HashMap<String, Instant>,
}

impl UserHub {
    fn notify_handle(&mut self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// 取走该 client 的全部待执行请求；顺带刷新在线时间
    fn drain_for(&mut self, client_id: &str) -> Vec<ToolCallRequest> {
        self.last_polls.insert(client_id.to_string(), Instant::now());
        let (mine, rest): (VecDeque<_>, VecDeque<_>) =
            self.pending.drain(..).partition(|r| r.client_id == client_id);
        self.pending = rest;
        mine.into_iter().collect()
    }

    fn is_online(&self, client_id: &str) -> bool {
        self.last_polls
            .get(client_id)
            .is_some_and(|t| t.elapsed() < ONLINE_WINDOW)
    }
}

// ─── 供其他模块使用的 pub API ───────────────────────────────────────────────

/// 入队一条工具调用并等待 client 回传结果。超时/断线返回 Err。
pub async fn dispatch_tool_call(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    tool: &str,
    input: Value,
    timeout: Duration,
) -> Result<ToolCallResult> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut hubs = state.client_hubs.write().await;
        let hub = hubs.entry(user_id.to_string()).or_default();
        hub.pending.push_back(ToolCallRequest {
            request_id: request_id.clone(),
            client_id: client_id.to_string(),
            tool: tool.to_string(),
            input,
        });
        hub.inflight.insert(request_id.clone(), tx);
        hub.notify.notify_one();
    }

    let result = tokio::time::timeout(timeout, rx).await;

    // 无论结果如何都清理 inflight，迟到的结果会走"未知 request_id"分支
    {
        let mut hubs = state.client_hubs.write().await;
        if let Some(hub) = hubs.get_mut(user_id) {
            hub.inflight.remove(&request_id);
        }
    }

    match result {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => Err(anyhow!("client 连接中断")),
        Err(_) => Err(anyhow!("等待 client 执行结果超时（{}s）", timeout.as_secs())),
    }
}

/// 指定 client 是否在线（60s 内有 poll）
pub async fn is_client_online(state: &AppState, user_id: &str, client_id: &str) -> bool {
    let hubs = state.client_hubs.read().await;
    hubs.get(user_id)
        .is_some_and(|hub| hub.is_online(client_id))
}

/// 挑选一台在线且接受远程任务的 client：最近 poll 的优先
pub async fn pick_online_client(state: &AppState, user_id: &str) -> Option<ClientAgent> {
    let candidates = state.db.list_client_agents(user_id).await.ok()?;
    let hubs = state.client_hubs.read().await;
    let hub = hubs.get(user_id)?;
    candidates
        .into_iter()
        .filter(|c| c.accept_remote)
        .filter_map(|c| {
            hub.last_polls
                .get(&c.id)
                .filter(|t| t.elapsed() < ONLINE_WINDOW)
                .map(|t| (*t, c))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, c)| c)
}

// ─── HTTP 路由（client JWT protected 组）────────────────────────────────────

#[derive(Deserialize)]
pub struct RegistrationRequest {
    pub client_id: String,
    pub hostname: Option<String>,
    pub work_dir: Option<String>,
    pub accept_remote: bool,
}

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
        Ok(()) => R::ok(serde_json::json!({"client_id": body.client_id})),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct PollQuery {
    pub client_id: String,
}

/// 长轮询：挂起最长 25s 等待该 client 的待执行请求，超时返回空列表。
/// 每次调用（含超时返回）都刷新该 client 的在线时间。
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
        // notify_one 会存一个 permit，dispatch 与等待之间的竞争不会丢唤醒
        let _ = tokio::time::timeout(remaining, notify.notified()).await;
    }
}

#[derive(Deserialize)]
pub struct ToolResultRequest {
    pub request_id: String,
    pub content: String,
    pub is_error: bool,
}

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
            R::ok(serde_json::json!({"request_id": body.request_id}))
        }
        None => R::bad_request(format!("unknown request_id: {}", body.request_id)),
    }
}

/// 当前用户注册过的 client 列表及各自在线状态
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
                "online": hub.is_online(&c.id),
            })
        })
        .collect();
    R::ok(serde_json::json!({ "clients": list }))
}

#[derive(Deserialize)]
pub struct SetExecClientRequest {
    /// null = 切回 server 本地执行
    pub exec_client_id: Option<String>,
}

/// 切换会话的执行位置（server 本地 / 指定桌面 client）
pub async fn set_session_exec_client(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(session_id): Path<String>,
    Json(body): Json<SetExecClientRequest>,
) -> impl IntoResponse {
    let session = match state.db.get_session(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
    };
    if session.user_id.as_deref() != Some(claims.sub.as_str()) {
        return R::forbidden("session does not belong to current user");
    }

    let (exec_client_id, work_dir) = match body.exec_client_id {
        Some(ref cid) => {
            let client = match state.db.get_client_agent(&claims.sub, cid).await {
                Ok(Some(c)) => c,
                Ok(None) => return R::bad_request("client not registered"),
                Err(e) => return R::internal_error(e),
            };
            (Some(client.id), client.work_dir)
        }
        None => (None, None),
    };

    match state
        .db
        .set_session_exec_client(&session_id, exec_client_id.as_deref(), work_dir.as_deref())
        .await
    {
        Ok(()) => R::ok(serde_json::json!({
            "session_id": session_id,
            "exec_client_id": exec_client_id,
            "work_dir": work_dir,
        })),
        Err(e) => R::internal_error(e),
    }
}
