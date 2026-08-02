use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
    Extension, Json,
};
use code_agent::AgentEvent;
use hank_provider::{CompletionRequest, Message, StreamEvent};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_stream::StreamExt;

use crate::auth::Claims;
use crate::provider_registry;
use crate::response as R;

// ─── Changes ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateChangeRequest {
    pub name: String,
    pub work_dir: Option<String>,
    pub requirement_path: Option<String>,
    pub tasks_path: Option<String>,
}

#[derive(Deserialize)]
pub struct ListChangesQuery {
    pub status: Option<String>,
    pub work_dir: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateChangeRequest {
    pub name: Option<String>,
    pub status: Option<String>,
    pub explore_summary: Option<String>,
}

#[derive(Serialize)]
pub struct ChangeDetail {
    #[serde(flatten)]
    pub change: hank_db::Change,
    pub artifacts: Vec<ArtifactSummary>,
    pub task_counts: TaskCounts,
}

#[derive(Serialize)]
pub struct ArtifactSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub capability: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct TaskCounts {
    pub total: i64,
    pub done: i64,
    pub in_progress: i64,
    pub pending: i64,
}

pub async fn create_change(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Json(body): Json<CreateChangeRequest>,
) -> impl IntoResponse {
    match state
        .db
        .create_change(
            &body.name,
            body.work_dir.as_deref(),
            body.requirement_path.as_deref(),
            body.tasks_path.as_deref(),
        )
        .await
    {
        Ok(change) => R::created(change),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                R::bad_request("change name already exists")
            } else {
                R::internal_error(msg)
            }
        }
    }
}

pub async fn list_changes(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Query(q): Query<ListChangesQuery>,
) -> impl IntoResponse {
    if let Some(ref wd) = q.work_dir {
        match state.db.list_changes_by_work_dir(wd).await {
            Ok(changes) => R::ok(changes),
            Err(e) => R::internal_error(e),
        }
    } else {
        match state.db.list_changes(q.status.as_deref()).await {
            Ok(changes) => R::ok(changes),
            Err(e) => R::internal_error(e),
        }
    }
}

pub async fn get_change(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };

    let artifacts = match state.db.list_artifacts(&id).await {
        Ok(a) => a,
        Err(e) => return R::internal_error(e),
    };

    let (total, done, in_progress, pending) = match state.db.get_change_task_counts(&id).await {
        Ok(c) => c,
        Err(e) => return R::internal_error(e),
    };

    let detail = ChangeDetail {
        change,
        artifacts: artifacts
            .into_iter()
            .map(|a| ArtifactSummary {
                id: a.id,
                artifact_type: a.artifact_type,
                capability: a.capability,
                updated_at: a.updated_at,
            })
            .collect(),
        task_counts: TaskCounts {
            total,
            done,
            in_progress,
            pending,
        },
    };

    R::ok(detail)
}

pub async fn update_change(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
    Json(body): Json<UpdateChangeRequest>,
) -> impl IntoResponse {
    match state.db.get_change(&id).await {
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }
    if let Some(ref summary) = body.explore_summary {
        if let Err(e) = state.db.update_change_explore_summary(&id, summary).await {
            return R::internal_error(e);
        }
    }
    match state
        .db
        .update_change(&id, body.name.as_deref(), body.status.as_deref())
        .await
    {
        Ok(()) => R::no_content(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                R::bad_request("change name already exists")
            } else {
                R::internal_error(msg)
            }
        }
    }
}

pub async fn delete_change(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };
    if change.status != "draft" {
        return R::bad_request("only draft changes can be deleted");
    }
    match state.db.delete_change(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

pub async fn archive_change(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };

    // Check all tasks are done
    let (total, done, _, _) = match state.db.get_change_task_counts(&id).await {
        Ok(c) => c,
        Err(e) => return R::internal_error(e),
    };
    if total > 0 && done < total {
        return R::bad_request("all tasks must be completed before archiving");
    }

    // Merge spec artifacts into main specs
    let artifacts = match state.db.list_artifacts(&id).await {
        Ok(a) => a,
        Err(e) => return R::internal_error(e),
    };

    for artifact in &artifacts {
        if artifact.artifact_type != "spec" {
            continue;
        }
        let capability = match &artifact.capability {
            Some(c) => c.as_str(),
            None => continue,
        };

        // Find or create main spec
        let spec = match state.db.get_spec_by_capability(capability).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                // Create new spec from artifact
                match state
                    .db
                    .create_spec(
                        capability,
                        capability,
                        &artifact.content,
                        artifact.metadata.as_deref(),
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(e) => return R::internal_error(e),
                };
                continue;
            }
            Err(e) => return R::internal_error(e),
        };

        // Store snapshot of current spec
        if let Err(e) = state
            .db
            .create_spec_version(
                &spec.id,
                spec.version,
                &spec.content,
                spec.metadata.as_deref(),
                Some(&change.id),
            )
            .await
        {
            return R::internal_error(e);
        }

        // Merge: use LLM to intelligently merge, fallback to append
        let merged = match llm_merge_specs(&state, &spec.content, &artifact.content, capability)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("LLM merge failed for {capability}, falling back to append: {e:#}");
                format!("{}\n\n{}", spec.content, artifact.content)
            }
        };
        if let Err(e) = state
            .db
            .update_spec(&spec.id, Some(&merged), None, None)
            .await
        {
            return R::internal_error(e);
        }
    }

    // Set status to archived
    if let Err(e) = state.db.update_change(&id, None, Some("archived")).await {
        return R::internal_error(e);
    }

    R::no_content()
}

/// 使用 LLM 智能合并两段 spec 内容
async fn llm_merge_specs(
    state: &AppState,
    existing: &str,
    new_content: &str,
    capability: &str,
) -> anyhow::Result<String> {
    let (record, provider) = provider_registry::resolve_default(&state.db)
        .await
        .ok_or_else(|| anyhow::anyhow!("No provider available for spec merge"))?;
    let model = provider_registry::resolve_default_model(&record);

    let prompt = format!(
        "你是一个技术文档合并助手。请将以下两段 Spec 文档合并为一个完整、无重复、结构清晰的文档。\n\n\
        ## 现有 Spec: {capability}\n\n{existing}\n\n\
        ## 新增内容\n\n{new_content}\n\n\
        请直接输出合并后的完整文档，不要添加额外解释。"
    );

    let req = CompletionRequest {
        model,
        system: None,
        messages: vec![Message {
            role: hank_provider::Role::User,
            content: vec![hank_provider::ContentBlock::Text { text: prompt }],
        }],
        tools: vec![],
        max_tokens: 4096,
    };

    let mut stream = provider.stream(req).await?;
    let mut result = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::TextDelta(text)) => result.push_str(&text),
            Ok(StreamEvent::MessageEnd { .. }) => break,
            Err(e) => anyhow::bail!("Stream error: {e}"),
            _ => {}
        }
    }

    if result.is_empty() {
        anyhow::bail!("LLM returned empty response");
    }

    Ok(result)
}

// ─── Artifacts ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateArtifactRequest {
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub capability: Option<String>,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateArtifactRequest {
    pub content: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn list_artifacts(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(change_id): Path<String>,
) -> impl IntoResponse {
    match state.db.list_artifacts(&change_id).await {
        Ok(artifacts) => R::ok(artifacts),
        Err(e) => R::internal_error(e),
    }
}

pub async fn create_artifact(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(change_id): Path<String>,
    Json(body): Json<CreateArtifactRequest>,
) -> impl IntoResponse {
    let metadata_str = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    match state
        .db
        .create_artifact(
            &change_id,
            &body.artifact_type,
            body.capability.as_deref(),
            &body.content,
            metadata_str.as_deref(),
            body.status.as_deref(),
        )
        .await
    {
        Ok(artifact) => R::created(artifact),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                R::bad_request("artifact already exists for this type+capability")
            } else {
                R::internal_error(msg)
            }
        }
    }
}

pub async fn get_artifact(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((_change_id, artifact_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.db.get_artifact(&artifact_id).await {
        Ok(Some(artifact)) => R::ok(artifact),
        Ok(None) => R::not_found("artifact not found"),
        Err(e) => R::internal_error(e),
    }
}

pub async fn update_artifact(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((change_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateArtifactRequest>,
) -> impl IntoResponse {
    let metadata_str = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    match state
        .db
        .update_artifact(
            &artifact_id,
            body.content.as_deref(),
            metadata_str.as_deref(),
            None,
        )
        .await
    {
        Ok(()) => {
            // Emit SSE event if called from agent tool
            if let Some(session_id) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
                // Get artifact type for the event
                let artifact_type = state
                    .db
                    .get_artifact(&artifact_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|a| a.artifact_type)
                    .unwrap_or_default();
                let event = AgentEvent::ArtifactUpdated {
                    artifact_id: artifact_id.clone(),
                    change_id: change_id.clone(),
                    artifact_type,
                };
                let mut buffers = state.event_buffers.write().await;
                if let Some(buf) = buffers.get_mut(session_id) {
                    buf.push(event);
                }
            }
            R::no_content()
        }
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_artifact(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((_change_id, artifact_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.db.delete_artifact(&artifact_id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// ─── Tasks ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TaskInput {
    pub group_name: String,
    pub group_order: i32,
    pub task_order: i32,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct BatchCreateTasksRequest {
    pub tasks: Vec<TaskInput>,
}

#[derive(Deserialize)]
pub struct UpdateTaskRequest {
    pub status: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct TaskGroup {
    pub group_name: String,
    pub group_order: i32,
    pub tasks: Vec<hank_db::ChangeTask>,
    pub counts: TaskCounts,
}

pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(change_id): Path<String>,
) -> impl IntoResponse {
    let tasks = match state.db.list_tasks(&change_id).await {
        Ok(t) => t,
        Err(e) => return R::internal_error(e),
    };

    // Group by group_name
    let mut groups: Vec<TaskGroup> = Vec::new();
    for task in tasks {
        if let Some(group) = groups.iter_mut().find(|g| g.group_name == task.group_name) {
            group.tasks.push(task);
        } else {
            let group_order = task.group_order;
            let group_name = task.group_name.clone();
            groups.push(TaskGroup {
                group_name,
                group_order,
                tasks: vec![task],
                counts: TaskCounts {
                    total: 0,
                    done: 0,
                    in_progress: 0,
                    pending: 0,
                },
            });
        }
    }

    // Calculate counts per group
    for group in &mut groups {
        group.counts.total = group.tasks.len() as i64;
        group.counts.done = group.tasks.iter().filter(|t| t.status == "done").count() as i64;
        group.counts.in_progress = group
            .tasks
            .iter()
            .filter(|t| t.status == "in_progress")
            .count() as i64;
        group.counts.pending = group.counts.total - group.counts.done - group.counts.in_progress;
    }

    R::ok(groups)
}

pub async fn batch_create_tasks(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(change_id): Path<String>,
    Json(body): Json<BatchCreateTasksRequest>,
) -> impl IntoResponse {
    let tasks: Vec<(String, i32, i32, String, Option<String>)> = body
        .tasks
        .into_iter()
        .map(|t| {
            (
                t.group_name,
                t.group_order,
                t.task_order,
                t.title,
                t.description,
            )
        })
        .collect();
    match state.db.batch_create_tasks(&change_id, &tasks).await {
        Ok(created) => R::created(created),
        Err(e) => R::internal_error(e),
    }
}

pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((change_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateTaskRequest>,
) -> impl IntoResponse {
    match state
        .db
        .update_task(
            &task_id,
            body.status.as_deref(),
            body.title.as_deref(),
            body.description.as_deref(),
            None,
        )
        .await
    {
        Ok(()) => {
            // Emit SSE event if called from agent tool
            if let Some(session_id) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
                if let Some(status) = &body.status {
                    let event = AgentEvent::TaskUpdated {
                        task_id: task_id.clone(),
                        change_id: change_id.clone(),
                        status: status.clone(),
                    };
                    let mut buffers = state.event_buffers.write().await;
                    if let Some(buf) = buffers.get_mut(session_id) {
                        buf.push(event);
                    }
                }
            }
            R::no_content()
        }
        Err(e) => R::internal_error(e),
    }
}

pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((_change_id, task_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.db.delete_task(&task_id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

// ─── Context ─────────────────────────────────────────────────────────

pub async fn get_change_context(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };

    let artifacts = match state.db.list_artifacts(&id).await {
        Ok(a) => a,
        Err(e) => return R::internal_error(e),
    };

    let tasks = match state.db.list_tasks(&id).await {
        Ok(t) => t,
        Err(e) => return R::internal_error(e),
    };

    // Assemble context markdown
    let mut ctx = format!("# Change: {}\n\n", change.name);

    // Specs
    let spec_artifacts: Vec<_> = artifacts
        .iter()
        .filter(|a| a.artifact_type == "spec")
        .collect();
    if !spec_artifacts.is_empty() {
        ctx.push_str("## Specs\n\n");
        for spec in spec_artifacts {
            if let Some(cap) = &spec.capability {
                ctx.push_str(&format!("### {}\n\n", cap));
            }
            ctx.push_str(&spec.content);
            ctx.push_str("\n\n");
        }
    }

    // Tasks
    if !tasks.is_empty() {
        ctx.push_str("## Tasks\n\n");
        let mut current_group = String::new();
        for task in &tasks {
            if task.group_name != current_group {
                current_group = task.group_name.clone();
                ctx.push_str(&format!("### {}\n\n", current_group));
            }
            let marker = if task.status == "done" { "x" } else { " " };
            ctx.push_str(&format!("- [{}] {}\n", marker, task.title));
        }
    }

    R::ok(serde_json::json!({ "context": ctx }))
}

// ─── Explore ─────────────────────────────────────────────────────────

pub async fn start_explore(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Verify change exists
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };

    // Check if a session already exists for this change
    if let Ok(Some(session)) = state.db.get_session_by_change_id(&id).await {
        return R::ok(serde_json::json!({ "session_id": session.id }));
    }

    // Create a dedicated session for explore
    let work_dir = change.work_dir.as_deref();
    let session = match state
        .db
        .create_session(
            "",
            "",
            work_dir,
            Some(&claims.sub),
            None,
            Some("explore"),
            None,
        )
        .await
    {
        Ok(s) => s,
        Err(e) => return R::internal_error(e),
    };

    // Bind session to change
    if let Err(e) = state.db.set_session_change_id(&session.id, &id).await {
        return R::internal_error(e);
    }

    R::ok(serde_json::json!({ "session_id": session.id }))
}

// ─── Generate ────────────────────────────────────────────────────────

pub async fn start_generate(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Verify change exists and explore is complete
    let change = match state.db.get_change(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
    };

    if change.explore_summary.is_none() {
        return R::bad_request("explore phase must be completed before generating artifacts");
    }

    // Return the session_id so client can send a chat message to trigger generation
    let session = match state.db.get_session_by_change_id(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::bad_request("no session found for this change"),
        Err(e) => return R::internal_error(e),
    };

    R::ok(serde_json::json!({ "session_id": session.id, "change_id": id }))
}

// ─── Confirm Artifacts ───────────────────────────────────────────────

pub async fn confirm_artifacts(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_change(&id).await {
        Ok(None) => return R::not_found("change not found"),
        Err(e) => return R::internal_error(e),
        _ => {}
    }

    match state.db.confirm_artifacts(&id).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}
