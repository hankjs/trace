use crate::AppState;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    Extension, Json,
};
use code_agent::AgentEvent;
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::Claims;
use crate::response as R;

#[derive(Deserialize)]
pub struct CreateSpecRequest {
    pub capability: String,
    pub title: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_spec(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Json(body): Json<CreateSpecRequest>,
) -> impl IntoResponse {
    let metadata_str = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    match state
        .db
        .create_spec(
            &body.capability,
            &body.title,
            &body.content,
            metadata_str.as_deref(),
        )
        .await
    {
        Ok(spec) => R::created(spec),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                R::bad_request("capability name already exists")
            } else {
                R::internal_error(msg)
            }
        }
    }
}

pub async fn list_specs(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
) -> impl IntoResponse {
    match state.db.list_specs().await {
        Ok(specs) => R::ok(specs),
        Err(e) => R::internal_error(e),
    }
}

pub async fn get_spec(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_spec(&id).await {
        Ok(Some(spec)) => R::ok(spec),
        Ok(None) => R::not_found("spec not found"),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateSpecRequest {
    pub content: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub title: Option<String>,
}

pub async fn update_spec(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateSpecRequest>,
) -> impl IntoResponse {
    // Get current spec to store snapshot
    let spec = match state.db.get_spec(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::not_found("spec not found"),
        Err(e) => return R::internal_error(e),
    };

    // Store current version as snapshot
    if let Err(e) = state
        .db
        .create_spec_version(
            &spec.id,
            spec.version,
            &spec.content,
            spec.metadata.as_deref(),
            None,
        )
        .await
    {
        return R::internal_error(e);
    }

    let metadata_str = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    match state
        .db
        .update_spec(
            &id,
            body.content.as_deref(),
            metadata_str.as_deref(),
            body.title.as_deref(),
        )
        .await
    {
        Ok(()) => {
            let updated = state.db.get_spec(&id).await.ok().flatten();

            // Emit SSE event if called from agent tool
            if let Some(session_id) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
                let new_version = updated
                    .as_ref()
                    .map(|s| s.version)
                    .unwrap_or(spec.version + 1);
                let event = AgentEvent::SpecUpdated {
                    spec_id: id.clone(),
                    capability: spec.capability.clone(),
                    version: new_version,
                };
                let mut buffers = state.event_buffers.write().await;
                if let Some(buf) = buffers.get_mut(session_id) {
                    buf.push(event);
                }
            }

            match updated {
                Some(s) => R::ok(s),
                None => R::no_content(),
            }
        }
        Err(e) => R::internal_error(e),
    }
}

pub async fn list_spec_versions(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Verify spec exists
    match state.db.get_spec(&id).await {
        Ok(None) => return R::not_found("spec not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }
    match state.db.list_spec_versions(&id).await {
        Ok(versions) => R::ok(versions),
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_spec(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_spec(&id).await {
        Ok(None) => return R::not_found("spec not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }
    match state.db.delete_spec(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}
