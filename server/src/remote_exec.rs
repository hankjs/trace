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
use tokio::sync::{mpsc, oneshot, Notify};

/// 长轮询单次挂起时长
const POLL_TIMEOUT: Duration = Duration::from_secs(25);
/// 在线判定窗口：该时长内有 poll 即在线
const ONLINE_WINDOW: Duration = Duration::from_secs(60);
/// 远程调用在被代理工具超时之上追加的网络余量
pub const NETWORK_MARGIN: Duration = Duration::from_secs(10);
/// 纯观察类工具：admin 终端页每 3~5s 轮询一次，若计入「最后运行」
/// 会把该字段永久钉在「刚刚」，使它失去意义。
const OBSERVE_ONLY_TOOLS: [&str; 2] = ["terminal_list", "terminal_read"];

/// 该工具调用是否算作一次「真实派发」（用于刷新 last_active_at）
fn counts_as_dispatch(tool: &str) -> bool {
    !OBSERVE_ONLY_TOOLS.contains(&tool)
}

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
    /// client 注册时上报的本机 Agent CLI 能力。
    pub agent_backends: HashMap<String, Vec<String>>,
    /// Agent JSONL 流（request_id → server 消费通道）。
    pub agent_events: HashMap<String, mpsc::Sender<Value>>,
}

/// 已下发到 hank-cli、等待事件与终态的 Agent 任务。
pub struct RemoteAgentRun {
    pub request_id: String,
    pub event_rx: mpsc::Receiver<Value>,
    pub result_rx: oneshot::Receiver<ToolCallResult>,
}

impl UserHub {
    fn notify_handle(&mut self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// 取走该 client 的全部待执行请求；顺带刷新在线时间
    fn drain_for(&mut self, client_id: &str) -> Vec<ToolCallRequest> {
        self.last_polls
            .insert(client_id.to_string(), Instant::now());
        let (mine, rest): (VecDeque<_>, VecDeque<_>) = self
            .pending
            .drain(..)
            .partition(|r| r.client_id == client_id);
        self.pending = rest;
        mine.into_iter().collect()
    }

    /// 按 request_id 从 pending 移除尚未被 poll 取走的请求。
    /// 已被 drain 的请求不在 pending 中，调用无副作用。
    fn remove_pending(&mut self, request_id: &str) -> bool {
        let before = self.pending.len();
        self.pending.retain(|r| r.request_id != request_id);
        self.pending.len() < before
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

    // 入队即视为一次派发；DB 失败不影响本次调用。
    // 观察类工具（admin 页高频轮询）不计入，否则「最后运行」永远是「刚刚」。
    if counts_as_dispatch(tool) {
        let _ = state.db.touch_client_agent_active(client_id).await;
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
        Err(_) => Err(anyhow!(
            "等待 client 执行结果超时（{}s）",
            timeout.as_secs()
        )),
    }
}

/// 下发长时 Agent 任务。与普通工具不同，stdout/stderr 会通过 agent-event 独立流式回传。
pub async fn start_agent_run(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    input: Value,
) -> Result<RemoteAgentRun> {
    if !is_client_online(state, user_id, client_id).await {
        return Err(anyhow!("hank-cli 节点不在线"));
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (result_tx, result_rx) = oneshot::channel();
    let (event_tx, event_rx) = mpsc::channel(256);
    {
        let mut hubs = state.client_hubs.write().await;
        let hub = hubs.entry(user_id.to_string()).or_default();
        hub.pending.push_back(ToolCallRequest {
            request_id: request_id.clone(),
            client_id: client_id.to_string(),
            tool: "agent_run".to_string(),
            input,
        });
        hub.inflight.insert(request_id.clone(), result_tx);
        hub.agent_events.insert(request_id.clone(), event_tx);
        hub.notify.notify_one();
    }
    // 独立入队路径，不走 dispatch_tool_call，需单独刷新最后运行时间
    let _ = state.db.touch_client_agent_active(client_id).await;
    Ok(RemoteAgentRun {
        request_id,
        event_rx,
        result_rx,
    })
}

/// 释放 Agent 任务的 server 侧通道。迟到事件会被 HTTP handler 拒绝，不会串到其他任务。
/// 同时从 pending 移除尚未被 hank-cli poll 取走的请求，避免超时/离线后节点重连执行旧任务。
pub async fn cleanup_agent_run(state: &AppState, user_id: &str, request_id: &str) {
    let mut hubs = state.client_hubs.write().await;
    if let Some(hub) = hubs.get_mut(user_id) {
        hub.remove_pending(request_id);
        hub.inflight.remove(request_id);
        hub.agent_events.remove(request_id);
    }
}

/// 通知 hank-cli 终止指定 Agent 进程组。
pub async fn cancel_agent_run(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    request_id: &str,
) -> Result<()> {
    if !is_client_online(state, user_id, client_id).await {
        return Err(anyhow!("hank-cli 节点不在线，无法下发取消"));
    }
    let result = dispatch_tool_call(
        state,
        user_id,
        client_id,
        "agent_cancel",
        serde_json::json!({ "request_id": request_id }),
        Duration::from_secs(8),
    )
    .await?;
    if result.is_error {
        Err(anyhow!(result.content))
    } else {
        Ok(())
    }
}

/// 指定 client 是否在线（60s 内有 poll）
pub async fn is_client_online(state: &AppState, user_id: &str, client_id: &str) -> bool {
    let hubs = state.client_hubs.read().await;
    hubs.get(user_id)
        .is_some_and(|hub| hub.is_online(client_id))
}

/// 是否可被自动选路派发（接受远程 + 未停用）
fn is_dispatchable(c: &ClientAgent) -> bool {
    c.accept_remote && c.enabled
}

/// 挑选一台在线且接受远程任务的 client：最近 poll 的优先
pub async fn pick_online_client(state: &AppState, user_id: &str) -> Option<ClientAgent> {
    let candidates = state.db.list_client_agents(user_id).await.ok()?;
    let hubs = state.client_hubs.read().await;
    let hub = hubs.get(user_id)?;
    candidates
        .into_iter()
        .filter(is_dispatchable)
        .filter_map(|c| {
            hub.last_polls
                .get(&c.id)
                .filter(|t| t.elapsed() < ONLINE_WINDOW)
                .map(|t| (*t, c))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, c)| c)
}

/// 挑选一台在线、允许远程执行且明确上报了目标 Agent CLI 的 hank-cli 节点。
pub async fn pick_online_agent_client(
    state: &AppState,
    user_id: &str,
    backend: &str,
) -> Option<ClientAgent> {
    let candidates = state.db.list_client_agents(user_id).await.ok()?;
    let hubs = state.client_hubs.read().await;
    let hub = hubs.get(user_id)?;
    candidates
        .into_iter()
        .filter(|client| is_dispatchable(client) && client.work_dir.is_some())
        .filter(|client| {
            hub.agent_backends
                .get(&client.id)
                .is_some_and(|values| values.iter().any(|value| value == backend))
        })
        .filter_map(|client| {
            hub.last_polls
                .get(&client.id)
                .filter(|seen| seen.elapsed() < ONLINE_WINDOW)
                .map(|seen| (*seen, client))
        })
        .max_by_key(|(seen, _)| *seen)
        .map(|(_, client)| client)
}

/// 指定在线节点是否在最近一次 registration/poll 中上报了该 Agent 后端。
pub async fn client_reports_backend(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    backend: &str,
) -> bool {
    let hubs = state.client_hubs.read().await;
    hubs.get(user_id)
        .and_then(|hub| hub.agent_backends.get(client_id))
        .is_some_and(|values| values.iter().any(|value| value == backend))
}

// ─── HTTP 路由（client JWT protected 组）────────────────────────────────────

#[derive(Deserialize)]
pub struct RegistrationRequest {
    pub client_id: String,
    pub hostname: Option<String>,
    pub work_dir: Option<String>,
    pub accept_remote: bool,
    #[serde(default)]
    pub agent_backends: Vec<String>,
}

pub async fn register_client(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<RegistrationRequest>,
) -> impl IntoResponse {
    if body.client_id.trim().is_empty() {
        return R::bad_request("client_id is required");
    }
    let agent_backends = sanitize_agent_backends(body.agent_backends);
    let result = state
        .db
        .upsert_client_agent(
            &body.client_id,
            &claims.sub,
            body.hostname.as_deref(),
            body.work_dir.as_deref(),
            body.accept_remote,
        )
        .await;
    match result {
        Ok(()) => {
            let mut hubs = state.client_hubs.write().await;
            hubs.entry(claims.sub)
                .or_default()
                .agent_backends
                .insert(body.client_id.clone(), agent_backends.clone());
            R::ok(serde_json::json!({
                "client_id": body.client_id,
                "agent_backends": agent_backends,
            }))
        }
        Err(e) => R::internal_error(e),
    }
}

fn sanitize_agent_backends(backends: Vec<String>) -> Vec<String> {
    // 白名单唯一来源是 AgentBackend::ALL，不再各自维护一份清单。
    let mut clean = Vec::new();
    for backend in backends {
        let backend = backend.trim().to_ascii_lowercase();
        if hank_db::AgentBackend::parse(&backend).is_some() && !clean.contains(&backend) {
            clean.push(backend);
        }
    }
    clean
}

// ─── 终端通知上报 ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NotifyRequest {
    pub client_id: String,
    pub term_id: Option<String>,
    pub kind: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
}

/// client 上报终端通知（OSC 9/777 捕获：kimi task complete / approval 等）。
/// 落库即可，后续推微信/其他渠道由消费方决定。
pub async fn post_notification(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<NotifyRequest>,
) -> impl IntoResponse {
    if body.client_id.trim().is_empty() {
        return R::bad_request("client_id is required");
    }
    let kind = body.kind.as_deref().unwrap_or("notification");
    let title = body.title.as_deref().unwrap_or("");
    match state
        .db
        .create_client_notification(
            &claims.sub,
            &body.client_id,
            body.term_id.as_deref(),
            kind,
            title,
            body.body.as_deref(),
        )
        .await
    {
        Ok(id) => {
            tracing::info!(
                client_id = %body.client_id,
                term_id = ?body.term_id,
                title,
                "client terminal notification reported"
            );
            R::ok(serde_json::json!({"id": id}))
        }
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct PollQuery {
    pub client_id: String,
    pub agent_backends: Option<String>,
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

    // 长轮询进入即视为一次在线观测；25s 粒度足够，不必在 loop 内重复写
    let _ = state.db.touch_client_agent_seen(&client_id).await;

    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let (notify, requests) = {
            let mut hubs = state.client_hubs.write().await;
            let hub = hubs.entry(user_id.clone()).or_default();
            if let Some(backends) = query.agent_backends.as_deref() {
                hub.agent_backends.insert(
                    client_id.clone(),
                    sanitize_agent_backends(backends.split(',').map(str::to_string).collect()),
                );
            }
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

#[derive(Deserialize)]
pub struct AgentEventRequest {
    pub request_id: String,
    pub event: Value,
}

/// hank-cli 在本机 Agent 运行期间逐行上报 stdout/stderr。
pub async fn post_agent_event(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AgentEventRequest>,
) -> impl IntoResponse {
    let sender = {
        let hubs = state.client_hubs.read().await;
        hubs.get(&claims.sub)
            .and_then(|hub| hub.agent_events.get(&body.request_id))
            .cloned()
    };
    match sender {
        Some(tx) => match tx.try_send(body.event) {
            Ok(()) => R::ok(serde_json::json!({"request_id": body.request_id})),
            Err(mpsc::error::TrySendError::Full(_)) => R::bad_request("agent event buffer is full"),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                R::bad_request("agent event receiver closed")
            }
        },
        None => R::bad_request(format!("unknown agent request_id: {}", body.request_id)),
    }
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
                "enabled": c.enabled,
                "last_active_at": c.last_active_at,
                "last_seen_at": c.last_seen_at,
                "online": hub.is_online(&c.id),
                "agent_backends": hub.agent_backends.get(&c.id).cloned().unwrap_or_default(),
            })
        })
        .collect();
    R::ok(serde_json::json!({ "clients": list }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(request_id: &str, client_id: &str, tool: &str) -> ToolCallRequest {
        ToolCallRequest {
            request_id: request_id.to_string(),
            client_id: client_id.to_string(),
            tool: tool.to_string(),
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn agent_backends_are_allowlisted_and_deduplicated() {
        assert_eq!(
            sanitize_agent_backends(vec![
                " Codex ".to_string(),
                "codex".to_string(),
                "KIMI".to_string(),
                "shell".to_string(),
            ]),
            vec!["codex", "kimi"]
        );
    }

    #[test]
    fn agent_backend_allowlist_rejects_unknown_values() {
        assert!(sanitize_agent_backends(vec!["bash".into(), "native".into()]).is_empty());
        assert_eq!(
            sanitize_agent_backends(vec!["grok".into(), "claude".into(), "grok".into()]),
            vec!["grok", "claude"]
        );
    }

    #[test]
    fn remove_pending_drops_target_and_keeps_other_clients_and_requests() {
        let mut hub = UserHub::default();
        hub.pending.push_back(req("a1", "cli-a", "agent_run"));
        hub.pending.push_back(req("a2", "cli-a", "shell"));
        hub.pending.push_back(req("b1", "cli-b", "agent_run"));

        assert!(hub.remove_pending("a1"));
        let ids: Vec<_> = hub.pending.iter().map(|r| r.request_id.as_str()).collect();
        assert_eq!(ids, vec!["a2", "b1"]);
    }

    #[test]
    fn remove_pending_is_noop_when_already_drained() {
        let mut hub = UserHub::default();
        hub.pending.push_back(req("keep", "cli-a", "agent_run"));

        // 模拟已被 poll drain：目标不在 pending
        assert!(!hub.remove_pending("gone"));
        assert_eq!(hub.pending.len(), 1);
        assert_eq!(hub.pending[0].request_id, "keep");
    }

    #[test]
    fn cleanup_paths_clear_pending_and_channels() {
        let mut hub = UserHub::default();
        let rid = "run-1";
        hub.pending.push_back(req(rid, "cli-a", "agent_run"));
        hub.pending.push_back(req("other", "cli-b", "shell"));

        let (result_tx, _result_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel::<Value>(1);
        hub.inflight.insert(rid.to_string(), result_tx);
        hub.agent_events.insert(rid.to_string(), event_tx);

        // 与 cleanup_agent_run 相同的同步清理路径（避免构造完整 AppState）
        hub.remove_pending(rid);
        hub.inflight.remove(rid);
        hub.agent_events.remove(rid);

        assert!(!hub.pending.iter().any(|r| r.request_id == rid));
        assert_eq!(hub.pending.len(), 1);
        assert_eq!(hub.pending[0].request_id, "other");
        assert!(!hub.inflight.contains_key(rid));
        assert!(!hub.agent_events.contains_key(rid));
    }

    fn sample_client(accept_remote: bool, enabled: bool) -> ClientAgent {
        ClientAgent {
            id: "c1".into(),
            user_id: "u1".into(),
            hostname: Some("host".into()),
            work_dir: Some("/tmp".into()),
            accept_remote,
            enabled,
            last_active_at: None,
            last_seen_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn disabled_client_is_skipped_by_pick() {
        let c = sample_client(true, false);
        assert!(!is_dispatchable(&c));
    }

    #[test]
    fn enabled_client_passes_pick() {
        assert!(is_dispatchable(&sample_client(true, true)));
        // 两个条件是且关系：不接受远程即使 enabled 也不能自动派发
        assert!(!is_dispatchable(&sample_client(false, true)));
    }

    #[test]
    fn observation_tools_do_not_count_as_dispatch() {
        // admin 终端页 3~5s 一次的轮询不应刷新「最后运行」
        assert!(!counts_as_dispatch("terminal_list"));
        assert!(!counts_as_dispatch("terminal_read"));
    }

    #[test]
    fn real_tool_calls_count_as_dispatch() {
        assert!(counts_as_dispatch("terminal_write"));
        assert!(counts_as_dispatch("terminal_create"));
        assert!(counts_as_dispatch("shell"));
    }
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
