use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::{self, Claims};
use crate::config::DEFAULT_MODEL;
use crate::provider_registry;
use crate::response::{self as R};

pub async fn health() -> impl IntoResponse {
    R::ok(serde_json::json!({"status": "ok"}))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub scope: Option<String>, // "admin" or "client"
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let username = body.username.unwrap_or_default();
    let password = body.password.unwrap_or_default();
    let scope = body.scope.unwrap_or_else(|| "client".to_string());

    let user = match state.db.get_user_by_username(&username).await {
        Ok(Some(u)) => u,
        _ => return R::unauthorized("invalid credentials"),
    };

    if !bcrypt::verify(&password, &user.password_hash).unwrap_or(false) {
        return R::unauthorized("invalid credentials");
    }

    // Check scope permission
    if scope == "admin" && !user.can_login_admin {
        return R::forbidden("no admin access");
    }
    if scope == "client" && !user.can_login_client {
        return R::forbidden("no client access");
    }

    match auth::create_token(&state.jwt_secret, &user.id, &user.username, user.can_login_admin, user.can_login_client) {
        Ok(token) => R::ok(serde_json::json!({"token": token, "username": user.username, "can_admin": user.can_login_admin, "can_client": user.can_login_client})),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub work_dir: Option<String>,
    pub environment: Option<String>,
    pub session_type: Option<String>,
    pub metadata: Option<String>,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let provider = match body.provider {
        Some(p) => p,
        None => match provider_registry::default_provider_name(&state.db).await {
            Some(name) => name,
            None => return R::internal_error("No provider configured"),
        },
    };
    let model = match &body.model {
        Some(m) => m.clone(),
        None => {
            match state.db.get_provider_by_name(&provider).await {
                Ok(Some(record)) => provider_registry::resolve_default_model(&record),
                _ => DEFAULT_MODEL.to_string(),
            }
        }
    };

    match state
        .db
        .create_session(&provider, &model, body.work_dir.as_deref(), Some(&claims.sub), body.environment.as_deref(), body.session_type.as_deref(), body.metadata.as_deref())
        .await
    {
        Ok(session) => R::created(serde_json::json!(session)),
        Err(e) => R::internal_error(e),
    }
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.list_sessions_by_user(&claims.sub).await {
        Ok(sessions) => R::ok(serde_json::json!(sessions)),
        Err(e) => R::internal_error(e),
    }
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_session(&id).await {
        Ok(Some(session)) => R::ok(serde_json::json!(session)),
        Ok(None) => R::not_found("session not found"),
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_session(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateSessionRequest {
    pub title: Option<String>,
    pub work_dir: Option<String>,
    pub local_agent: Option<String>,
    pub local_work_dir: Option<String>,
    pub change_id: Option<String>,
    pub metadata: Option<String>,
}

pub async fn update_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSessionRequest>,
) -> impl IntoResponse {
    if let Some(title) = &body.title {
        if let Err(e) = state.db.update_session_title(&id, title).await {
            return R::internal_error(e);
        }
    }
    if body.work_dir.is_some() {
        if let Err(e) = state.db.update_session_work_dir(&id, body.work_dir.as_deref()).await {
            return R::internal_error(e);
        }
    }
    if body.local_agent.is_some() || body.local_work_dir.is_some() {
        if let Err(e) = state.db.update_session_local_agent(&id, body.local_agent.as_deref(), body.local_work_dir.as_deref()).await {
            return R::internal_error(e);
        }
    }
    if let Some(ref change_id) = body.change_id {
        if let Err(e) = state.db.set_session_change_id(&id, change_id).await {
            return R::internal_error(e);
        }
    }
    if let Some(ref metadata) = body.metadata {
        if let Err(e) = state.db.update_session_metadata(&id, metadata).await {
            return R::internal_error(e);
        }
    }
    match state.db.get_session(&id).await {
        Ok(Some(session)) => R::ok(serde_json::json!(session)),
        Ok(None) => R::not_found("session not found"),
        Err(e) => R::internal_error(e),
    }
}

pub async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<GetMessagesQuery>,
) -> impl IntoResponse {
    // If leaf_id provided, return branch messages; otherwise use active_leaf or all
    if let Some(leaf_id) = &query.leaf_id {
        match state.db.get_branch_messages(&id, leaf_id).await {
            Ok(messages) => R::ok(serde_json::json!(messages)),
            Err(e) => R::internal_error(e),
        }
    } else {
        // Try to use active_leaf_id from session
        let session = state.db.get_session(&id).await.ok().flatten();
        if let Some(leaf) = session.and_then(|s| s.active_leaf_id) {
            match state.db.get_branch_messages(&id, &leaf).await {
                Ok(messages) => R::ok(serde_json::json!(messages)),
                Err(e) => R::internal_error(e),
            }
        } else {
            // Fallback: return all messages (legacy behavior for sessions without tree)
            match state.db.get_messages(&id).await {
                Ok(messages) => R::ok(serde_json::json!(messages)),
                Err(e) => R::internal_error(e),
            }
        }
    }
}

#[derive(Deserialize)]
pub struct GetMessagesQuery {
    pub leaf_id: Option<String>,
}

pub async fn get_message_tree(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_message_tree(&id).await {
        Ok(tree) => R::ok(serde_json::json!(tree)),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateActiveLeafRequest {
    pub leaf_id: String,
}

pub async fn update_active_leaf(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateActiveLeafRequest>,
) -> impl IntoResponse {
    match state.db.update_active_leaf(&id, &body.leaf_id).await {
        Ok(()) => R::ok(serde_json::json!({"status": "ok"})),
        Err(e) => R::internal_error(e),
    }
}

// POST /api/sessions/{id}/messages - save a message (for local agent sessions)
#[derive(Deserialize)]
pub struct PostMessageRequest {
    pub role: String,
    pub content: serde_json::Value,
    pub parent_id: Option<String>,
}

pub async fn post_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PostMessageRequest>,
) -> impl IntoResponse {
    // Verify session exists
    let session = match state.db.get_session(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
    };

    let now = chrono::Utc::now();
    // If parent_id not provided, fall back to session's active_leaf_id to prevent broken chains
    let parent_id = body.parent_id.as_deref()
        .or(session.active_leaf_id.as_deref());

    match state.db.save_message(&id, &body.role, &body.content, now, parent_id).await {
        Ok(msg_id) => {
            // Update active_leaf_id to the new message
            let _ = state.db.update_active_leaf(&id, &msg_id).await;
            R::created(serde_json::json!({"id": msg_id}))
        }
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct TruncateMessagesRequest {
    pub keep_count: u32,
}

pub async fn truncate_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<TruncateMessagesRequest>,
) -> impl IntoResponse {
    match state.db.truncate_messages(&id, body.keep_count).await {
        Ok(deleted) => R::ok(serde_json::json!({"deleted": deleted})),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateSettingsRequest {
    pub settings: std::collections::HashMap<String, String>,
}

pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    for (key, value) in &body.settings {
        if let Err(e) = state.db.set_setting(key, value).await {
            return R::internal_error(e);
        }
    }
    R::ok(serde_json::json!({"status": "ok"}))
}

pub async fn list_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let providers = state.db.list_providers_ordered().await.unwrap_or_default();
    let provider_list: Vec<serde_json::Value> = providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            let models: serde_json::Value = serde_json::from_str(&p.models).unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "name": p.name,
                "type": p.provider_type,
                "default_model": p.default_model,
                "models": models,
            })
        })
        .collect();

    let default_provider = providers.iter().find(|p| p.enabled).map(|p| p.name.clone()).unwrap_or_default();

    R::ok(serde_json::json!({
        "providers": provider_list,
        "default_provider": default_provider,
    }))
}

#[derive(Deserialize)]
pub struct ListDirQuery {
    pub path: Option<String>,
}

pub async fn list_directory(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListDirQuery>,
) -> impl IntoResponse {
    let dir_path = match &query.path {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")),
    };

    // Validate path is under allowed directories (if configured)
    if !state.config.server.allowed_dirs.is_empty() {
        let canonical = dir_path.canonicalize().unwrap_or_else(|_| dir_path.clone());
        let allowed = state.config.server.allowed_dirs.iter().any(|allowed| {
            let allowed_path = std::path::Path::new(allowed);
            canonical.starts_with(allowed_path)
        });
        if !allowed {
            return R::forbidden("Path not in allowed directories");
        }
    }

    // Use spawn_blocking to avoid blocking the async runtime
    let dir_path_clone = dir_path.clone();
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let entries_result = tokio::task::spawn_blocking(move || {
        match std::fs::read_dir(&dir_path_clone) {
            Ok(rd) => Ok((rd, dir_path_clone, false)),
            Err(_) => {
                // Directory doesn't exist, fall back to home
                std::fs::read_dir(&home).map(|rd| (rd, home, true))
            }
        }
    })
    .await
    .unwrap_or_else(|e| Err(std::io::Error::new(std::io::ErrorKind::Other, e)));

    let (entries, dir_path, redirected) = match entries_result {
        Ok(tuple) => tuple,
        Err(e) => {
            return R::bad_request(e);
        }
    };

    let mut dirs: Vec<serde_json::Value> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            dirs.push(serde_json::json!({ "name": name, "is_dir": true }));
        }
    }
    dirs.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    let parent = dir_path
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    let mut result = serde_json::json!({
        "path": dir_path.to_string_lossy(),
        "parent": parent,
        "entries": dirs,
    });
    if redirected {
        result["redirected"] = serde_json::json!(true);
        result["message"] = serde_json::json!(format!(
            "目录已被移除，已重定向到 {}",
            dir_path.to_string_lossy()
        ));
    }

    R::ok(result)
}

// POST /api/sessions/{id}/local-events - batch upload local ACP execution events
#[derive(Deserialize)]
pub struct LocalEventInput {
    pub event_type: String,
    pub agent_type: String,
    pub payload: serde_json::Value,
}

pub async fn post_local_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Vec<LocalEventInput>>,
) -> impl IntoResponse {
    // Verify session exists
    match state.db.get_session(&id).await {
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }

    let events: Vec<hank_db::LocalEvent> = body
        .into_iter()
        .map(|e| hank_db::LocalEvent {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: id.clone(),
            event_type: e.event_type,
            agent_type: e.agent_type,
            payload: e.payload.to_string(),
            source: "local".to_string(),
            created_at: chrono::Utc::now(),
        })
        .collect();

    match state.db.insert_local_events(&events).await {
        Ok(()) => R::created(serde_json::json!({"count": events.len()})),
        Err(e) => R::internal_error(e),
    }
}

// GET /api/sessions/{id}/events - returns both remote and local events with source marker
pub async fn get_session_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Verify session exists
    match state.db.get_session(&id).await {
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }

    let remote_events = match state.db.get_session_events(&id).await {
        Ok(events) => events,
        Err(e) => return R::internal_error(e),
    };

    let local_events = match state.db.get_local_events(&id).await {
        Ok(events) => events,
        Err(e) => return R::internal_error(e),
    };

    // Merge into a unified list sorted by created_at
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

/// GET /api/sessions/{id}/transcript — FR-SESSION-3: 导出用户可见会话记录（不含隐藏 prompt、内部推理、secret）
pub async fn get_session_transcript(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let session = match state.db.get_session(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
    };

    let messages = if let Some(ref leaf) = session.active_leaf_id {
        state.db.get_branch_messages(&id, leaf).await.unwrap_or_default()
    } else {
        state.db.get_messages(&id).await.unwrap_or_default()
    };

    // 仅保留用户可见角色，不含内部系统消息
    let transcript: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| serde_json::json!({
            "role": m.role,
            "content": m.content,
            "created_at": m.created_at,
        }))
        .collect();

    R::ok(serde_json::json!({
        "session_id": id,
        "transcript": transcript,
        "message_count": transcript.len(),
    }))
}

// GET /api/templates?category=requirement — client-accessible template listing
#[derive(Deserialize)]
pub struct TemplateQuery {
    pub category: Option<String>,
}

pub async fn list_templates(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TemplateQuery>,
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
