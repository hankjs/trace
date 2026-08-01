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

// --- Agent CLI 凭据管理（codex / claude） ---
//
// 部署环境的第三方 API 端点用完或轮换时，过去只能登服务器改 /opt/hank/agent-cli.env
// 再重启 systemd。这组端点把配置放进库里，cli_agent 每轮任务读一次，改完即时生效。
// 与 /api/admin/providers 不同，这里的 GET 绝不回传 api_key，只回传是否已设置。

/// 后端凭据配置的对外表示。api_key 只以布尔形式暴露，避免明文凭据经浏览器流转。
#[derive(Serialize)]
pub struct AgentCliConfigView {
    pub backend: String,
    pub auth_kind: String,
    /// 库里是否已存有凭据；前端据此显示「已配置，留空则不修改」。
    pub api_key_set: bool,
    pub base_url: String,
    pub model: String,
    pub extra_env: serde_json::Value,
    pub enabled: bool,
    pub updated_at: Option<String>,
    pub updated_by: String,
    /// 当前真正生效的来源：db / env / provider，都没有时为 null。
    /// 让 admin 能看出「我存了配置但还没启用，实际仍在用服务器上的环境文件」。
    pub effective_source: Option<crate::cli_agent::AuthSource>,
    /// 该后端允许的 auth_kind 与附加环境变量键，供前端渲染选项。
    pub auth_kind_options: Vec<String>,
    pub extra_env_keys: Vec<String>,
}

/// GET /api/admin/agent-cli-config — 返回两个后端的配置与实际生效来源
pub async fn list_agent_cli_configs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stored = match state.db.list_agent_cli_configs().await {
        Ok(rows) => rows,
        Err(e) => return R::internal_error(e),
    };
    let mut views = Vec::new();
    for backend in ["claude", "codex"] {
        let Some((auth_kinds, extra_keys)) = crate::cli_agent::backend_env_whitelist(backend)
        else {
            continue;
        };
        let record = stored.iter().find(|row| row.backend == backend);
        let effective_source = crate::cli_agent::effective_auth_source(&state, backend).await;
        views.push(AgentCliConfigView {
            backend: backend.to_string(),
            auth_kind: record
                .map(|row| row.auth_kind.clone())
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| auth_kinds[0].to_string()),
            api_key_set: record.is_some_and(|row| !row.api_key.trim().is_empty()),
            base_url: record.map(|row| row.base_url.clone()).unwrap_or_default(),
            model: record.map(|row| row.model.clone()).unwrap_or_default(),
            extra_env: record
                .and_then(|row| serde_json::from_str(&row.extra_env).ok())
                .unwrap_or_else(|| serde_json::json!({})),
            enabled: record.is_some_and(|row| row.enabled),
            updated_at: record.map(|row| row.updated_at.to_rfc3339()),
            updated_by: record.map(|row| row.updated_by.clone()).unwrap_or_default(),
            effective_source,
            auth_kind_options: auth_kinds.iter().map(|key| key.to_string()).collect(),
            extra_env_keys: extra_keys.iter().map(|key| key.to_string()).collect(),
        });
    }
    R::ok(views)
}

#[derive(Deserialize)]
pub struct UpdateAgentCliConfigRequest {
    pub auth_kind: Option<String>,
    /// 留空或不传表示保留库里已有的凭据，只改端点/模型时不必重新粘贴 key。
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub extra_env: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

/// PUT /api/admin/agent-cli-config/{backend} — 写入某后端的凭据配置
pub async fn update_agent_cli_config(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<crate::auth::Claims>,
    Path(backend): Path<String>,
    Json(body): Json<UpdateAgentCliConfigRequest>,
) -> impl IntoResponse {
    let Some((auth_kinds, extra_keys)) = crate::cli_agent::backend_env_whitelist(&backend) else {
        return R::bad_request(format!("不支持的外部 Agent 后端: {backend}"));
    };

    let auth_kind = body
        .auth_kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .unwrap_or(auth_kinds[0]);
    if !auth_kinds.contains(&auth_kind) {
        return R::bad_request(format!(
            "{backend} 的凭据变量名只能是 {}",
            auth_kinds.join(" / ")
        ));
    }

    // extra_env 只接受白名单键的字符串值，其他键直接拒绝而不是静默丢弃，
    // 避免 admin 以为配上了却没生效。
    let mut extra_env = serde_json::Map::new();
    if let Some(serde_json::Value::Object(map)) = body.extra_env {
        for (key, value) in map {
            if !extra_keys.contains(&key.as_str()) {
                return R::bad_request(format!("{backend} 不允许配置环境变量 {key}"));
            }
            let Some(text) = value.as_str() else {
                return R::bad_request(format!("环境变量 {key} 的值必须是字符串"));
            };
            let text = text.trim();
            // 控制字符会破坏子进程环境，且可能被用来伪造日志行。
            if text.contains(['\n', '\r', '\0']) {
                return R::bad_request(format!("环境变量 {key} 的值包含非法控制字符"));
            }
            if !text.is_empty() {
                extra_env.insert(key, serde_json::Value::String(text.to_string()));
            }
        }
    }

    let base_url = body.base_url.as_deref().map(str::trim).unwrap_or_default();
    if !base_url.is_empty() && !base_url.starts_with("https://") {
        // 凭据会随请求发往该端点，明文 HTTP 会让 key 暴露在链路上。
        return R::bad_request("base URL 必须是 https:// 开头");
    }
    let model = body.model.as_deref().map(str::trim).unwrap_or_default();
    let api_key = body.api_key.as_deref().map(str::trim).unwrap_or_default();
    let enabled = body.enabled.unwrap_or(false);

    // 启用就必须真的有凭据可用，否则会静默退回环境文件，看起来「配好了」其实没生效。
    if enabled && api_key.is_empty() {
        let stored_key = state
            .db
            .get_agent_cli_config(&backend)
            .await
            .ok()
            .flatten()
            .is_some_and(|row| !row.api_key.trim().is_empty());
        if !stored_key {
            return R::bad_request("启用前需要先填写凭据");
        }
    }

    let extra_env_json = serde_json::Value::Object(extra_env).to_string();
    if let Err(e) = state
        .db
        .upsert_agent_cli_config(
            &backend,
            auth_kind,
            Some(api_key),
            base_url,
            model,
            &extra_env_json,
            enabled,
            &claims.username,
        )
        .await
    {
        return R::internal_error(e);
    }
    tracing::info!(
        backend = %backend,
        enabled,
        operator = %claims.username,
        "更新外部 Agent CLI 凭据配置"
    );
    R::ok(serde_json::json!({"status": "ok"}))
}

/// POST /api/admin/agent-cli-config/{backend}/test — 用库里的配置向端点发一次最小请求
///
/// 换第三方中转后最想知道的是「这个 key 和端点到底能不能用」。等飞书任务跑失败再看日志
/// 反馈太慢，这里直接探一次。不落库、不改配置，只回报连通性。
pub async fn test_agent_cli_config(
    State(state): State<Arc<AppState>>,
    Path(backend): Path<String>,
) -> impl IntoResponse {
    if crate::cli_agent::backend_env_whitelist(&backend).is_none() {
        return R::bad_request(format!("不支持的外部 Agent 后端: {backend}"));
    }
    let config = match state.db.get_agent_cli_config(&backend).await {
        Ok(Some(row)) if !row.api_key.trim().is_empty() => row,
        Ok(_) => return R::bad_request("还没有配置凭据"),
        Err(e) => return R::internal_error(e),
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(e) => return R::internal_error(e),
    };
    let api_key = config.api_key.trim();
    let base_url = config.base_url.trim().trim_end_matches('/');

    // 两个后端的探测都用「1 token 的最小对话」，比拉模型列表更接近真实调用路径：
    // 中转常见的失败是端点通但不支持某协议，只有真正发一次推理请求才能暴露。
    let result = if backend == "claude" {
        let base = if base_url.is_empty() {
            "https://api.anthropic.com"
        } else {
            base_url
        };
        let model = if config.model.trim().is_empty() {
            "claude-sonnet-4-20250514"
        } else {
            config.model.trim()
        };
        client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
            }))
            .send()
            .await
    } else {
        let base = if base_url.is_empty() {
            "https://api.openai.com/v1"
        } else {
            base_url
        };
        let model = if config.model.trim().is_empty() {
            "gpt-5"
        } else {
            config.model.trim()
        };
        // Codex 0.146 只走 Responses API，探测也必须用同一个协议，
        // 否则「Chat Completions 能通但 Responses 不支持」的中转会被误判为可用。
        client
            .post(format!("{base}/responses"))
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": model,
                "input": "ping",
                "max_output_tokens": 16,
            }))
            .send()
            .await
    };

    match result {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                R::ok(serde_json::json!({
                    "ok": true,
                    "status": status.as_u16(),
                    "message": "端点可用",
                }))
            } else {
                // 回传响应体片段帮助定位（模型名错、余额不足、协议不支持），
                // 截断避免把上游的长 HTML 错误页整页塞进 admin。上游有时会在错误里
                // 回显收到的 key，先脱敏再返回。
                let body = response.text().await.unwrap_or_default();
                let body = body.replace(api_key, "[redacted]");
                let detail: String = body.chars().take(400).collect();
                R::ok(serde_json::json!({
                    "ok": false,
                    "status": status.as_u16(),
                    "message": format!("端点返回 {status}"),
                    "detail": detail,
                }))
            }
        }
        Err(e) => R::ok(serde_json::json!({
            "ok": false,
            "message": format!("请求失败: {e}"),
        })),
    }
}

/// DELETE /api/admin/agent-cli-config/{backend} — 彻底清掉库里的凭据行
///
/// 停用（enabled=false）只是不再使用，凭据仍留在库里；轮换掉泄露的 key 时需要真正删除。
/// 删除后该后端自动回退到服务器上的 agent-cli.env。
pub async fn delete_agent_cli_config(
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<crate::auth::Claims>,
    Path(backend): Path<String>,
) -> impl IntoResponse {
    if crate::cli_agent::backend_env_whitelist(&backend).is_none() {
        return R::bad_request(format!("不支持的外部 Agent 后端: {backend}"));
    }
    if let Err(e) = state.db.delete_agent_cli_config(&backend).await {
        return R::internal_error(e);
    }
    tracing::info!(
        backend = %backend,
        operator = %claims.username,
        "删除外部 Agent CLI 凭据配置"
    );
    R::no_content()
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
