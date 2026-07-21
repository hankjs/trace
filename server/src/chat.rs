use crate::AppState;
use crate::provider_registry;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
};
use futures::stream::Stream;
use code_agent::{AgentEvent, AgentSession};
use code_tools::{
    ask_user::AskUserTool,
    explore_tools::FinalizeExploreTool,
    file_checksum::new_checksum_store,
    generate_tools::GenerateArtifactsTool,
    git::GitTool,
    read_file::ReadFileTool, search::SearchTool, shell::ShellTool,
    spec_tools::{UpdateArtifactTool, UpdateSpecTool, UpdateTaskStatusTool},
    str_replace::StrReplaceTool,
    list_directory::ListDirectoryTool,
    test_runner::TestRunnerTool,
    web_fetch::WebFetchTool,
    write_file::WriteFileTool, Tool,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, Instrument};

// --- Event Buffer types ---

#[derive(Clone, Debug)]
pub struct EventEntry {
    pub id: u64,
    pub event: AgentEvent,
}

pub struct EventBuffer {
    pub events: Vec<EventEntry>,
    pub next_id: u64,
    pub completed: bool,
    pub tx: broadcast::Sender<EventEntry>,
}

impl EventBuffer {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            events: Vec::new(),
            next_id: 1,
            completed: false,
            tx,
        }
    }

    pub fn push(&mut self, event: AgentEvent) -> EventEntry {
        let id = self.next_id;
        self.next_id += 1;
        let entry = EventEntry { id, event };
        self.events.push(entry.clone());
        let _ = self.tx.send(entry.clone());
        entry
    }
}

// --- Request types ---

#[derive(Deserialize)]
pub struct ChatRequest {
    pub content: String,
    pub images: Option<Vec<ImagePayload>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    pub apply_change_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ImagePayload {
    pub media_type: String,
    pub data: String,
}

// --- Chat turn core (decoupled from HTTP) ---

/// Options for a single chat turn, supplied by the caller (HTTP handler,
/// WeChat bot, ...).
pub struct ChatTurnOpts {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    pub apply_change_id: Option<String>,
    /// JWT carried by spec-family tools when they call back into this server.
    pub auth_token: String,
}

/// Handle returned by [`run_chat_turn`]. The receiver is subscribed to the
/// session's EventBuffer BEFORE the agent task starts, so the caller sees
/// every event from the beginning. The stream ends when the buffer's
/// broadcast channel closes after the turn completes.
pub struct ChatTurnHandle {
    pub event_rx: broadcast::Receiver<EventEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatTurnError {
    #[error("No enabled providers available")]
    NoProviders,
}

/// Extract the plain text of a content-block list (used for checkpoint
/// labels, session titles and ask_user tool_result payloads).
fn text_from_blocks(blocks: &[hank_provider::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            hank_provider::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Run one full chat turn for a session, independent of any HTTP request.
///
/// Persists agent events/metrics, updates the session's EventBuffer (the
/// resume endpoint depends on it), and registers the cancellation token in
/// `state.active_tasks` exactly like the HTTP chat endpoint does.
pub async fn run_chat_turn(
    state: &Arc<AppState>,
    session_id: &str,
    content: Vec<hank_provider::ContentBlock>,
    opts: ChatTurnOpts,
) -> Result<ChatTurnHandle, ChatTurnError> {
    let session_id = session_id.to_string();

    // Resolve providers with fallback from DB
    let fallback_list = match opts.provider.as_deref() {
        Some(name) => provider_registry::resolve_with_fallback(&state.db, name).await,
        None => {
            let all = state.db.list_providers_ordered().await.unwrap_or_default();
            all.into_iter()
                .filter(|r| r.enabled)
                .map(|r| {
                    let p = provider_registry::build_provider_from_record(&r);
                    (r, p)
                })
                .collect()
        }
    };
    if fallback_list.is_empty() {
        return Err(ChatTurnError::NoProviders);
    }

    // Determine model from the first (preferred) provider record
    let first_record = &fallback_list[0].0;
    let model = match &opts.model {
        Some(m) => provider_registry::resolve_model(first_record, m),
        None => provider_registry::resolve_default_model(first_record),
    };

    let session_record = state.db.get_session(&session_id).await.ok().flatten();
    let work_dir = session_record.as_ref().and_then(|s| s.work_dir.clone());
    let work_dir_for_checkpoint = work_dir.clone();
    let work_dir_for_agent = work_dir.clone();
    let session_change_id = session_record.as_ref().and_then(|s| s.change_id.clone());
    let session_type = session_record.as_ref().map(|s| s.session_type.clone()).unwrap_or_else(|| "chat".to_string());
    // 远程执行：会话绑定的桌面 client（None = server 本地执行）
    let exec_client_id = session_record.as_ref().and_then(|s| s.exec_client_id.clone());
    let session_user_id = session_record.as_ref().and_then(|s| s.user_id.clone());

    // Check if this session has a pending ask_user state
    let pending_ask_user = session_record.as_ref().and_then(|s| s.pending_ask_user.clone());

    let parent_id_for_new_msg = match opts.parent_id.as_deref() {
        Some("root") => None,
        Some(id) => Some(id.to_string()),
        None => session_record.as_ref().and_then(|s| s.active_leaf_id.clone()),
    };

    let tools: Vec<Arc<dyn Tool>> = {
        let base_url = format!("http://127.0.0.1:{}", state.config.server.port);
        let token = opts.auth_token.clone();
        let checksum_store = new_checksum_store();
        // fs/shell 类工具：绑定 exec_client_id 的会话改用远程代理工具，
        // 在桌面 client 本地执行；test_runner 远程会话不提供（可用 shell 跑测试）
        let mut t: Vec<Arc<dyn Tool>> = match (&exec_client_id, &session_user_id) {
            (Some(client_id), Some(user_id)) => {
                let mut remote = crate::remote_tools::remote_tool_set(
                    state.clone(),
                    user_id,
                    client_id,
                    work_dir.clone(),
                );
                remote.push(Arc::new(WebFetchTool::new()));
                remote
            }
            _ => vec![
                Arc::new(ShellTool::new(work_dir.clone())),
                Arc::new(ReadFileTool::with_checksum_store(work_dir.clone(), checksum_store.clone())),
                Arc::new(WriteFileTool::with_checksum_store(work_dir.clone(), checksum_store.clone())),
                Arc::new(StrReplaceTool::with_checksum_store(work_dir.clone(), checksum_store.clone())),
                Arc::new(ListDirectoryTool::new(work_dir.clone())),
                Arc::new(SearchTool::new(work_dir.clone())),
                Arc::new(GitTool::new(work_dir.clone())),
                Arc::new(WebFetchTool::new()),
                Arc::new(TestRunnerTool::new(work_dir.clone())),
            ],
        };
        t.push(Arc::new(UpdateSpecTool::new(base_url.clone(), token.clone(), session_id.clone())));
        t.push(Arc::new(UpdateTaskStatusTool::new(base_url.clone(), token.clone(), session_id.clone())));
        t.push(Arc::new(UpdateArtifactTool::new(base_url.clone(), token.clone(), session_id.clone())));
        t.push(Arc::new(AskUserTool::new()));
        // Add explore/generate tools if session is bound to a change or is explore type
        if let Some(ref cid) = session_change_id {
            t.push(Arc::new(FinalizeExploreTool::new(base_url.clone(), token.clone(), cid.clone(), session_id.clone())));
            t.push(Arc::new(GenerateArtifactsTool::new(base_url.clone(), token.clone(), cid.clone())));
        } else if session_type == "explore" {
            // Explore session without a change yet — finalize_explore will create the change
            t.push(Arc::new(FinalizeExploreTool::new(base_url.clone(), token.clone(), String::new(), session_id.clone())));
        }
        t
    };

    // Initialize event buffer for this session
    {
        let mut buffers = state.event_buffers.write().await;
        buffers.insert(session_id.clone(), EventBuffer::new());
    }

    // Subscribe to the buffer's broadcast BEFORE spawning the task
    let rx = {
        let buffers = state.event_buffers.read().await;
        buffers.get(&session_id).unwrap().tx.subscribe()
    };

    // Set up internal channel for agent -> buffer forwarding
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let db = state.db.clone();
    let sid = session_id.clone();
    let content_text = text_from_blocks(&content);
    let apply_change_id = opts.apply_change_id.clone();

    // If pending_ask_user, the user's reply becomes a tool_result
    let user_content: Vec<hank_provider::ContentBlock> = if let Some(ref pending_json) = pending_ask_user {
        // Parse pending state to get tool_use_id
        let pending: serde_json::Value = serde_json::from_str(pending_json).unwrap_or_default();
        let tool_use_id = pending["tool_use_id"].as_str().unwrap_or_default().to_string();
        // Clear pending state
        let _ = state.db.clear_session_pending_ask_user(&session_id).await;
        vec![hank_provider::ContentBlock::ToolResult {
            tool_use_id,
            content: content_text.clone(),
            is_error: false,
        }]
    } else {
        content
    };
    let is_first_message = {
        let msgs = if let Some(ref leaf) = parent_id_for_new_msg {
            state.db.get_branch_messages(&session_id, leaf).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        msgs.is_empty()
    };
    let parent_for_chain = parent_id_for_new_msg.clone();

    // Build history for the session
    let history: Vec<hank_provider::Message> = if let Some(ref leaf) = parent_id_for_new_msg {
        state.db.get_branch_messages(&session_id, leaf).await.unwrap_or_default()
    } else {
        Vec::new()
    }
    .iter()
    .filter_map(|m| {
        let content: Vec<hank_provider::ContentBlock> =
            serde_json::from_str(&m.content).ok()?;
        let role = match m.role.as_str() {
            "user" => hank_provider::Role::User,
            "assistant" => hank_provider::Role::Assistant,
            _ => return None,
        };
        Some(hank_provider::Message { role, content })
    })
    .collect();
    let history_len = history.len();

    // ─── Checkpoint: 在 agent 执行前创建快照 ─────────────────────────────
    if let Some(ref wd) = work_dir_for_checkpoint {
        let cp_state = state.clone();
        let cp_session_id = session_id.clone();
        let cp_message_id = parent_id_for_new_msg.clone().unwrap_or_default();
        let cp_work_dir = wd.clone();
        let cp_label: String = content_text.chars().take(40).collect();
        tokio::spawn(async move {
            if let Err(e) = crate::checkpoints::create_checkpoint_for_turn(
                &cp_state, &cp_session_id, &cp_message_id, &cp_work_dir, &cp_label,
            ).await {
                tracing::warn!(session_id = %cp_session_id, "checkpoint creation failed: {e:#}");
            }
        });
    }

    let cancel_token = CancellationToken::new();
    {
        let mut tasks = state.active_tasks.write().await;
        tasks.insert(session_id.clone(), cancel_token.clone());
    }
    let state_for_cleanup = state.clone();
    let sid_for_cleanup = session_id.clone();

    // Forwarder task: reads from agent mpsc, writes to EventBuffer + persists metrics
    let state_fwd = state.clone();
    let sid_fwd = session_id.clone();
    let db_fwd = state.db.clone();
    let sid_fwd2 = session_id.clone();
    let fwd_span = tracing::info_span!("chat_fwd", session_id = %session_id);
    tokio::spawn(async move {
        let mut seq: u64 = 0;
        while let Some(event) = event_rx.recv().await {
            seq += 1;

            // Persist event to agent_events table
            let event_type = extract_event_type(&event);
            if let Ok(payload) = serde_json::to_string(&event) {
                let _ = db_fwd.save_agent_event(&sid_fwd2, event_type, &payload, seq).await;
            }

            // Keep existing metric/tool persistence for backward compatibility
            match &event {
                AgentEvent::Metrics { input_tokens, output_tokens, latency_ms, model, provider, phase: _ } => {
                    let _ = db_fwd.save_agent_metric(
                        &sid_fwd2, None, *input_tokens, *output_tokens, *latency_ms, model, provider,
                    ).await;
                }
                AgentEvent::ToolMetrics { tool_name, duration_ms, is_error } => {
                    let _ = db_fwd.save_tool_execution(
                        &sid_fwd2, None, tool_name, *duration_ms, *is_error,
                    ).await;
                }
                AgentEvent::AskUser { question, options, tool_use_id } => {
                    // Persist pending ask_user state to session
                    let pending = serde_json::json!({
                        "tool_use_id": tool_use_id,
                        "question": question,
                        "options": options,
                    });
                    let _ = db_fwd.set_session_pending_ask_user(&sid_fwd2, &pending.to_string()).await;
                }
                _ => {}
            }
            let mut buffers = state_fwd.event_buffers.write().await;
            if let Some(buf) = buffers.get_mut(&sid_fwd) {
                buf.push(event);
            }
        }
    }.instrument(fwd_span));

    // Agent task with fallback loop
    let state_for_buffer2 = state.clone();
    let sid_for_buffer2 = session_id.clone();
    let agent_span = tracing::info_span!("chat_agent", session_id = %session_id);
    tokio::spawn(async move {
        let max_attempts = fallback_list.len().min(3);
        let mut last_error = String::new();

        // Business prompts are assembled by the client. The server keeps a
        // neutral fallback prompt while it forwards requests and records logs.
        let system_prompt = "You are a helpful AI assistant. Follow the user's message exactly.".to_string();

        // If apply_change_id provided, bind session to change
        if let Some(ref apply_cid) = apply_change_id {
            let _ = db.set_session_change_id(&sid, apply_cid).await;
        }

        for attempt in 0..max_attempts {
            let (ref record, ref provider) = fallback_list[attempt];
            let current_model = if attempt == 0 {
                model.clone()
            } else {
                provider_registry::resolve_default_model(record)
            };

            // Emit fallback event if not first attempt
            if attempt > 0 {
                let prev_name = &fallback_list[attempt - 1].0.name;
                let _ = event_tx.send(AgentEvent::ProviderFallback {
                    from: prev_name.clone(),
                    to: record.name.clone(),
                    reason: last_error.clone(),
                }).await;
            }

            let mut session = AgentSession::new(
                provider.clone(),
                tools.clone(),
                current_model,
                system_prompt.clone(),
            );
            // 接入权限模型：workspace-write 模式 + work_dir 作为可写根（FR-PERM-1/4）
            if let Some(ref wd) = work_dir_for_agent {
                session = session.with_permission(
                    code_tools::PermissionMode::WorkspaceWrite,
                    wd.clone(),
                );
            }
            // 分层注入运行时 + 环境上下文（FR-CTX-1/2）。
            // base 沿用客户端组装的 system_prompt（业务提示词由客户端负责）。
            {
                let runtime = code_agent::RuntimeContext {
                    permission_mode: "workspace-write".to_string(),
                    approval_policy: "auto".to_string(),
                    writable_roots: work_dir_for_agent.clone().into_iter().collect(),
                    network_policy: "restricted".to_string(),
                    tools: tools
                        .iter()
                        .map(|t| code_agent::ToolInfo {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            risk: format!("{:?}", t.risk_level()),
                        })
                        .collect(),
                    skills: vec![],
                };
                let env = code_agent::EnvironmentContext {
                    cwd: work_dir_for_agent.clone(),
                    shell: "/bin/sh".to_string(),
                    current_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                    timezone: "UTC".to_string(),
                    repo_root: work_dir_for_agent.clone(),
                    sandbox_mode: "workspace-write".to_string(),
                    network_policy: "restricted".to_string(),
                };
                // 远程执行会话：向 agent 说明工作目录在用户本地桌面机器上
                let mut project_segments: Vec<code_agent::PromptSegment> = vec![];
                if exec_client_id.is_some() {
                    project_segments.push(code_agent::PromptSegment::Dynamic(
                        "执行环境说明：工作目录在用户的本地桌面机器上，文件与 shell 类工具\
                        通过远程执行通道在该机器上运行，所有路径均为该机器的本地路径。"
                            .to_string(),
                    ));
                }
                let (_assembled, named) = code_agent::build_layered_prompt(
                    Some(&system_prompt),
                    Some(&runtime),
                    Some(&env),
                    &project_segments,
                );
                session = session.with_layered_prompt(named);
            }
            session.set_messages(history.clone());

            match session.run(user_content.clone(), event_tx.clone(), cancel_token.clone()).await {
                Ok(()) => {
                    // Success — save messages
                    let new_messages: Vec<_> = session.messages().iter().skip(history_len).collect();
                    if !new_messages.is_empty() {
                        let base_time = chrono::Utc::now();
                        let mut prev_id = parent_for_chain;
                        for (i, msg) in new_messages.iter().enumerate() {
                            let role = match msg.role {
                                hank_provider::Role::User => "user",
                                hank_provider::Role::Assistant => "assistant",
                            };
                            let content_val = serde_json::to_value(&msg.content).unwrap_or_default();
                            let ts = base_time + chrono::Duration::microseconds(i as i64);
                            match db.save_message(&sid, role, &content_val, ts, prev_id.as_deref()).await {
                                Ok(new_id) => prev_id = Some(new_id),
                                Err(_) => break,
                            }
                        }
                        if let Some(ref leaf) = prev_id {
                            let _ = db.update_active_leaf(&sid, leaf).await;
                        }
                        let _ = db.touch_session(&sid).await;
                    }

                    if is_first_message {
                        let title: String = content_text.chars().take(50).collect();
                        let _ = db.update_session_title(&sid, &title).await;
                    }
                    break;
                }
                Err(e) => {
                    last_error = format!("{e:#}");
                    let is_retryable = is_retryable_error(&last_error);

                    if !is_retryable || attempt == max_attempts - 1 {
                        // Non-retryable or last attempt — emit error
                        error!(session_id = %sid, provider = %record.name, "Agent error: {e:#}");
                        let _ = event_tx.send(AgentEvent::Error { message: format!("{e:#}") }).await;

                        let error_content = serde_json::json!([{"type": "error", "text": format!("{e:#}")}]);
                        let ts = chrono::Utc::now();
                        if let Ok(err_id) = db.save_message(&sid, "assistant", &error_content, ts, parent_for_chain.as_deref()).await {
                            let _ = db.update_active_leaf(&sid, &err_id).await;
                        }
                        let _ = db.touch_session(&sid).await;
                        break;
                    }
                    // Retryable — continue to next provider
                    tracing::warn!(provider = %record.name, "Provider failed, trying fallback: {}", last_error);
                }
            }
        }

        // Drop event_tx so forwarder finishes
        drop(event_tx);

        // Remove token from active tasks
        {
            let mut tasks = state_for_cleanup.active_tasks.write().await;
            tasks.remove(&sid_for_cleanup);
        }

        // Mark buffer as completed
        {
            let mut buffers = state_for_buffer2.event_buffers.write().await;
            if let Some(buf) = buffers.get_mut(&sid_for_buffer2) {
                buf.completed = true;
            }
        }
    }.instrument(agent_span));

    Ok(ChatTurnHandle { event_rx: rx })
}

// PLACEHOLDER_CHAT_HANDLER

pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    // spec 类工具回调 server 自身时使用请求携带的 JWT
    let auth_token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();

    let mut blocks = vec![hank_provider::ContentBlock::Text { text: body.content }];
    if let Some(images) = body.images {
        for img in images {
            blocks.push(hank_provider::ContentBlock::Image {
                source: hank_provider::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: img.media_type,
                    data: img.data,
                },
            });
        }
    }

    let opts = ChatTurnOpts {
        provider: body.provider,
        model: body.model,
        parent_id: body.parent_id,
        apply_change_id: body.apply_change_id,
        auth_token,
    };

    match run_chat_turn(&state, &session_id, blocks, opts).await {
        Ok(handle) => {
            // Build SSE stream from broadcast receiver + heartbeat
            let stream = make_sse_stream(handle.event_rx);
            Sse::new(stream).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            e.to_string(),
        )
            .into_response(),
    }
}

/// Check if an error is retryable (network, rate limit, 5xx, auth issues).
fn is_retryable_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("overloaded")
}

// PLACEHOLDER_MAKE_SSE

fn make_sse_stream(
    mut rx: broadcast::Receiver<EventEntry>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(entry) => {
                            let json = serde_json::to_string(&entry.event).unwrap_or_default();
                            yield Ok(Event::default().data(json).id(entry.id.to_string()));

                            // If this was TurnComplete, end the stream
                            if matches!(entry.event, AgentEvent::TurnComplete) {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("SSE client lagged by {n} events");
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
                _ = heartbeat_interval.tick() => {
                    yield Ok(Event::default().event("heartbeat").data("{}"));
                }
            }
        }
    }
}

// --- Resume handler ---

#[derive(Deserialize)]
pub struct ResumeQuery {
    pub last_event_id: u64,
}

pub async fn resume_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<ResumeQuery>,
) -> impl IntoResponse {
    let last_id = query.last_event_id;

    // Get missed events and optionally subscribe for live events
    let (missed, rx, completed) = {
        let buffers = state.event_buffers.read().await;
        match buffers.get(&session_id) {
            Some(buf) => {
                let missed: Vec<EventEntry> = buf
                    .events
                    .iter()
                    .filter(|e| e.id > last_id)
                    .cloned()
                    .collect();
                let rx = if !buf.completed {
                    Some(buf.tx.subscribe())
                } else {
                    None
                };
                (missed, rx, buf.completed)
            }
            None => {
                return (StatusCode::NOT_FOUND, "No event buffer for session").into_response();
            }
        }
    };

    let stream = make_resume_stream(missed, rx, completed, last_id);
    Sse::new(stream).into_response()
}

fn make_resume_stream(
    missed: Vec<EventEntry>,
    rx: Option<broadcast::Receiver<EventEntry>>,
    completed: bool,
    last_id: u64,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        // First replay missed events
        for entry in &missed {
            let json = serde_json::to_string(&entry.event).unwrap_or_default();
            yield Ok(Event::default().data(json).id(entry.id.to_string()));
        }

        // If session already completed and TurnComplete wasn't in missed, send it
        if completed {
            let has_turn_complete = missed.iter().any(|e| matches!(e.event, AgentEvent::TurnComplete));
            if !has_turn_complete {
                let json = serde_json::to_string(&AgentEvent::TurnComplete).unwrap_or_default();
                yield Ok(Event::default().data(json).id("end".to_string()));
            }
            return;
        }

        // Subscribe to live events
        if let Some(mut rx) = rx {
            let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));
            // Determine the highest ID we've already sent
            let mut max_sent = missed.last().map(|e| e.id).unwrap_or(last_id);

            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(entry) => {
                                if entry.id <= max_sent {
                                    continue; // already sent during replay
                                }
                                max_sent = entry.id;
                                let json = serde_json::to_string(&entry.event).unwrap_or_default();
                                yield Ok(Event::default().data(json).id(entry.id.to_string()));
                                if matches!(entry.event, AgentEvent::TurnComplete) {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    _ = heartbeat_interval.tick() => {
                        yield Ok(Event::default().event("heartbeat").data("{}"));
                    }
                }
            }
        }
    }
}

// --- Stop handler ---

pub async fn stop_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let tasks = state.active_tasks.read().await;
    if let Some(token) = tasks.get(&session_id) {
        token.cancel();
        StatusCode::OK
    } else {
        StatusCode::OK
    }
}

fn extract_event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::TextDelta { .. } => "text_delta",
        AgentEvent::ToolStart { .. } => "tool_start",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::TurnComplete => "turn_complete",
        AgentEvent::Error { .. } => "error",
        AgentEvent::Thinking { .. } => "thinking",
        AgentEvent::WorkerSpawned { .. } => "worker_spawned",
        AgentEvent::WorkerCompleted { .. } => "worker_completed",
        AgentEvent::Verification { .. } => "verification",
        AgentEvent::LoopDetected { .. } => "loop_detected",
        AgentEvent::TokenWarning { .. } => "token_warning",
        AgentEvent::CompressionTriggered { .. } => "compression_triggered",
        AgentEvent::Metrics { .. } => "metrics",
        AgentEvent::ToolMetrics { .. } => "tool_metrics",
        AgentEvent::ProviderFallback { .. } => "provider_fallback",
        AgentEvent::SpecUpdated { .. } => "spec_updated",
        AgentEvent::TaskUpdated { .. } => "task_updated",
        AgentEvent::ArtifactUpdated { .. } => "artifact_updated",
        AgentEvent::AskUser { .. } => "ask_user",
        AgentEvent::ExploreComplete { .. } => "explore_complete",
        AgentEvent::GenerateComplete { .. } => "generate_complete",
        AgentEvent::LlmRequest { .. } => "llm_request",
        AgentEvent::ToolOutputDelta { .. } => "tool_output_delta",
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::RunCancelled { .. } => "run_cancelled",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::FileChanged { .. } => "file_changed",
        AgentEvent::PermissionRequested { .. } => "permission_requested",
        AgentEvent::PermissionDenied { .. } => "permission_denied",
        AgentEvent::ContextAssembled { .. } => "context_assembled",
        AgentEvent::PlanUpdated { .. } => "plan_updated",
        AgentEvent::VerificationStarted { .. } => "verification_started",
        AgentEvent::VerificationCompleted { .. } => "verification_completed",
    }
}
