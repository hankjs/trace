use crate::AppState;
use crate::provider_registry;
use crate::response::{self as R};
use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    Json,
};
use code_agent::{AgentEvent, AgentSession};
use code_tools::{
    read_file::ReadFileTool, search::SearchTool, shell::ShellTool, write_file::WriteFileTool, Tool,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// --- Request/Response types ---

#[derive(Deserialize)]
pub struct PaginationQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub search: Option<String>,
    pub session_type: Option<String>,
}

#[derive(Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub data: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

#[derive(Deserialize)]
pub struct PromptTemplateRequest {
    pub name: String,
    pub content: String,
    pub category: Option<String>,
}

#[derive(Deserialize)]
pub struct ReplayRequest {
    pub session_id: String,
    pub prompt_template_id: Option<String>,
    pub system_prompt: Option<String>,
}

// --- Handlers ---

#[derive(Serialize)]
struct SessionWithUser {
    #[serde(flatten)]
    session: hank_db::Session,
    username: Option<String>,
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PaginationQuery>,
) -> impl IntoResponse {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).min(100);

    let all_sessions = match state.db.list_sessions().await {
        Ok(s) => s,
        Err(e) => return R::internal_error(e),
    };

    // Load users for username lookup
    let users = state.db.list_users().await.unwrap_or_default();
    let user_map: std::collections::HashMap<&str, &str> = users
        .iter()
        .map(|u| (u.id.as_str(), u.username.as_str()))
        .collect();

    let filtered: Vec<_> = if let Some(ref search) = query.search {
        let s = search.to_lowercase();
        all_sessions.into_iter().filter(|sess| {
            sess.title.to_lowercase().contains(&s)
                || sess.id.contains(&s)
                || sess.user_id.as_deref()
                    .and_then(|uid| user_map.get(uid))
                    .map(|name| name.to_lowercase().contains(&s))
                    .unwrap_or(false)
        }).collect()
    } else {
        all_sessions
    };

    // Filter by session_type if specified
    let filtered: Vec<_> = match &query.session_type {
        Some(st) if st == "explore" => filtered.into_iter().filter(|s| s.session_type == "explore").collect(),
        Some(st) if st == "!explore" => filtered.into_iter().filter(|s| s.session_type != "explore").collect(),
        Some(st) => filtered.into_iter().filter(|s| s.session_type == *st).collect(),
        None => filtered,
    };

    let total = filtered.len() as u64;
    let start = ((page - 1) * per_page) as usize;
    let data: Vec<SessionWithUser> = filtered.into_iter().skip(start).take(per_page as usize).map(|sess| {
        let username = sess.user_id.as_deref()
            .and_then(|uid| user_map.get(uid))
            .map(|s| s.to_string());
        SessionWithUser { session: sess, username }
    }).collect();

    R::ok(PaginatedResponse { data, total, page, per_page })
}

pub async fn session_replay(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let messages = match state.db.get_messages(&session_id).await {
        Ok(m) => m,
        Err(e) => return R::internal_error(e),
    };
    let metrics = state.db.get_session_metrics(&session_id).await.unwrap_or_default();
    let tool_executions = state.db.get_session_tool_executions(&session_id).await.unwrap_or_default();

    #[derive(Serialize)]
    struct ReplayResponse {
        messages: Vec<hank_db::DbMessage>,
        metrics: Vec<hank_db::AgentMetric>,
        tool_executions: Vec<hank_db::ToolExecution>,
    }

    R::ok(ReplayResponse { messages, metrics, tool_executions })
}

pub async fn session_events(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    // Merge agent_events + local_events (explore events are in local_events)
    let remote_events = match state.db.get_session_events(&session_id).await {
        Ok(events) => events,
        Err(e) => return R::internal_error(e),
    };

    let local_events = match state.db.get_local_events(&session_id).await {
        Ok(events) => events,
        Err(e) => return R::internal_error(e),
    };

    let mut unified: Vec<serde_json::Value> = Vec::new();

    for e in remote_events {
        unified.push(serde_json::json!({
            "id": e.id,
            "session_id": e.session_id,
            "event_type": e.event_type,
            "payload": e.payload,
            "seq": e.seq,
            "source": "remote",
            "created_at": e.created_at,
        }));
    }

    for e in local_events {
        unified.push(serde_json::json!({
            "id": e.id,
            "session_id": e.session_id,
            "event_type": e.event_type,
            "agent_type": e.agent_type,
            "payload": e.payload,
            "source": e.source,
            "created_at": e.created_at,
        }));
    }

    unified.sort_by(|a, b| {
        let ta = a["created_at"].as_str().unwrap_or("");
        let tb = b["created_at"].as_str().unwrap_or("");
        ta.cmp(tb)
    });

    R::ok(serde_json::json!(unified))
}

pub async fn metrics_overview(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_metrics_overview().await {
        Ok(overview) => R::ok(overview),
        Err(e) => R::internal_error(e),
    }
}

pub async fn metrics_by_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let metrics = state.db.get_session_metrics(&session_id).await.unwrap_or_default();
    let tool_executions = state.db.get_session_tool_executions(&session_id).await.unwrap_or_default();

    #[derive(Serialize)]
    struct SessionMetrics {
        metrics: Vec<hank_db::AgentMetric>,
        tool_executions: Vec<hank_db::ToolExecution>,
    }

    R::ok(SessionMetrics { metrics, tool_executions })
}

pub async fn create_prompt_template(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PromptTemplateRequest>,
) -> impl IntoResponse {
    match state.db.save_prompt_template(&body.name, &body.content, body.category.as_deref()).await {
        Ok(id) => R::created(serde_json::json!({"id": id})),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct TemplateListQuery {
    pub category: Option<String>,
}

pub async fn list_prompt_templates(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TemplateListQuery>,
) -> impl IntoResponse {
    let result = if let Some(ref cat) = query.category {
        state.db.get_templates_by_category(cat).await
    } else {
        state.db.list_prompt_templates().await
    };
    match result {
        Ok(templates) => R::ok(templates),
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_prompt_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_prompt_template(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

pub async fn replay_with_prompt(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplayRequest>,
) -> impl IntoResponse {
    // Load original session messages (user messages only)
    let all_messages = match state.db.get_messages(&body.session_id).await {
        Ok(m) => m,
        Err(e) => return R::internal_error(e),
    };

    let user_messages: Vec<String> = all_messages.iter()
        .filter(|m| m.role == "user")
        .filter_map(|m| {
            let blocks: Vec<serde_json::Value> = serde_json::from_str(&m.content).ok()?;
            blocks.iter().find_map(|b| b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
        })
        .collect();

    if user_messages.is_empty() {
        return R::bad_request("No user messages found in session");
    }

    // Determine system prompt
    let system_prompt = if let Some(ref prompt) = body.system_prompt {
        prompt.clone()
    } else if let Some(ref template_id) = body.prompt_template_id {
        match state.db.get_prompt_template(template_id).await {
            Ok(Some(t)) => t.content,
            _ => return R::bad_request("Template not found"),
        }
    } else {
        "You are a helpful AI assistant.".to_string()
    };

    // Get default provider from DB
    let (record, provider) = match provider_registry::resolve_default(&state.db).await {
        Some(p) => p,
        None => return R::internal_error("No provider available"),
    };

    let model = provider_registry::resolve_default_model(&record);

    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ShellTool::new(None)),
        Arc::new(ReadFileTool::new(None)),
        Arc::new(WriteFileTool::new(None)),
        Arc::new(SearchTool::new(None)),
    ];

    let mut session = AgentSession::new(provider, tools, model, system_prompt);
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let cancel = CancellationToken::new();

    // Spawn agent task that replays all user messages sequentially
    tokio::spawn(async move {
        for msg in user_messages {
            let content = vec![hank_provider::ContentBlock::Text { text: msg }];
            if let Err(e) = session.run(content, event_tx.clone(), cancel.clone()).await {
                let _ = event_tx.send(AgentEvent::Error { message: format!("{e:#}") }).await;
                break;
            }
        }
    });

    // Stream results as SSE
    let stream = async_stream::stream! {
        while let Some(event) = event_rx.recv().await {
            let json = serde_json::to_string(&crate::chat::event_for_stream(&event)).unwrap_or_default();
            yield Ok::<_, Infallible>(Event::default().data(json));
        }
    };

    Sse::new(stream).into_response()
}

// --- User Management ---

pub async fn list_users(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_users().await {
        Ok(users) => R::ok(users),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub can_login_admin: Option<bool>,
    pub can_login_client: Option<bool>,
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let can_admin = body.can_login_admin.unwrap_or(false);
    let can_client = body.can_login_client.unwrap_or(true);
    match state.db.create_user(&body.username, &body.password, can_admin, can_client).await {
        Ok(user) => R::created(serde_json::json!({"id": user.id, "username": user.username})),
        Err(e) => R::bad_request(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub can_login_admin: Option<bool>,
    pub can_login_client: Option<bool>,
    pub password: Option<String>,
}

pub async fn update_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    if let (Some(can_admin), Some(can_client)) = (body.can_login_admin, body.can_login_client) {
        if let Err(e) = state.db.update_user_permissions(&id, can_admin, can_client).await {
            return R::internal_error(e);
        }
    } else if let Some(can_admin) = body.can_login_admin {
        // Fetch current to preserve other field
        if let Err(e) = state.db.update_user_permissions(&id, can_admin, true).await {
            return R::internal_error(e);
        }
    } else if let Some(can_client) = body.can_login_client {
        if let Err(e) = state.db.update_user_permissions(&id, true, can_client).await {
            return R::internal_error(e);
        }
    }

    if let Some(ref password) = body.password {
        if let Err(e) = state.db.update_user_password(&id, password).await {
            return R::internal_error(e);
        }
    }

    R::ok(serde_json::json!({"status": "ok"}))
}

pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_user(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// --- Provider Management ---

pub async fn list_providers(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_providers_ordered().await {
        Ok(providers) => R::ok(providers),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub provider_type: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub models: Option<serde_json::Value>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

pub async fn create_provider(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProviderRequest>,
) -> impl IntoResponse {
    let models_json = body.models
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());

    match state.db.create_provider(
        &body.name,
        &body.provider_type,
        &body.api_key,
        body.base_url.as_deref().unwrap_or(""),
        body.default_model.as_deref().unwrap_or(""),
        &models_json,
        body.priority.unwrap_or(0),
        body.enabled.unwrap_or(true),
    ).await {
        Ok(record) => R::created(serde_json::json!(record)),
        Err(e) => R::bad_request(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateProviderRequest {
    pub name: String,
    pub provider_type: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub models: Option<serde_json::Value>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

pub async fn update_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProviderRequest>,
) -> impl IntoResponse {
    let models_json = body.models
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());

    match state.db.update_provider(
        &id,
        &body.name,
        &body.provider_type,
        &body.api_key,
        body.base_url.as_deref().unwrap_or(""),
        body.default_model.as_deref().unwrap_or(""),
        &models_json,
        body.priority.unwrap_or(0),
        body.enabled.unwrap_or(true),
    ).await {
        Ok(()) => R::ok(serde_json::json!({"status": "ok"})),
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_provider(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// --- Image Provider Management ---

pub async fn list_image_providers(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_image_providers_ordered().await {
        Ok(providers) => R::ok(providers),
        Err(e) => R::internal_error(e),
    }
}

pub async fn create_image_provider(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProviderRequest>,
) -> impl IntoResponse {
    let models_json = body.models
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    match state.db.create_image_provider(
        &body.name, &body.provider_type, &body.api_key,
        body.base_url.as_deref().unwrap_or(""),
        body.default_model.as_deref().unwrap_or(""),
        &models_json, body.priority.unwrap_or(0), body.enabled.unwrap_or(true),
    ).await {
        Ok(record) => R::created(serde_json::json!(record)),
        Err(e) => R::bad_request(e),
    }
}

pub async fn update_image_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProviderRequest>,
) -> impl IntoResponse {
    let models_json = body.models
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    match state.db.update_image_provider(
        &id, &body.name, &body.provider_type, &body.api_key,
        body.base_url.as_deref().unwrap_or(""),
        body.default_model.as_deref().unwrap_or(""),
        &models_json, body.priority.unwrap_or(0), body.enabled.unwrap_or(true),
    ).await {
        Ok(()) => R::ok(serde_json::json!({"status": "ok"})),
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_image_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_image_provider(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// --- AI Generate ---

#[derive(Deserialize)]
pub struct ChatGenerateRequest {
    pub prompt: String,
    pub context: Option<String>,
}

pub async fn chat_generate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatGenerateRequest>,
) -> impl IntoResponse {
    use futures::StreamExt;
    use hank_provider::{CompletionRequest, ContentBlock, Message, Role, StreamEvent};

    let (record, provider) = match provider_registry::resolve_default(&state.db).await {
        Some(p) => p,
        None => return R::internal_error("No provider available"),
    };

    let model = provider_registry::resolve_default_model(&record);

    let mut user_text = body.prompt.clone();
    if let Some(ctx) = &body.context {
        user_text = format!("{}\n\n---\nContext:\n{}", user_text, ctx);
    }

    let req = CompletionRequest {
        model,
        system: Some("根据用户提示生成文本，直接输出结果，不要添加额外解释。".to_string()),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: user_text }],
        }],
        tools: vec![],
        max_tokens: 4096,
    };

    let event_stream = match provider.stream(req).await {
        Ok(s) => s,
        Err(e) => return R::internal_error(e),
    };

    let sse_stream = event_stream.map(|result| {
        match result {
            Ok(StreamEvent::TextDelta(text)) => {
                let json = serde_json::json!({"type": "text_delta", "text": text});
                Ok::<_, Infallible>(Event::default().data(json.to_string()))
            }
            Ok(StreamEvent::MessageEnd { .. }) => {
                let json = serde_json::json!({"type": "done"});
                Ok(Event::default().data(json.to_string()))
            }
            Ok(_) => Ok(Event::default().comment("")),
            Err(e) => {
                let json = serde_json::json!({"type": "error", "message": e.to_string()});
                Ok(Event::default().data(json.to_string()))
            }
        }
    });

    Sse::new(sse_stream).into_response()
}
