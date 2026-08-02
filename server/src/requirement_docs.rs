use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::response as R;

// ─── Client endpoints (ExploreAgent uses these) ─────────────────────

#[derive(Deserialize)]
pub struct CreateDocRequest {
    pub change_id: String,
    pub session_id: Option<String>,
    pub name: String,
    pub content: String,
    pub progress_json: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateDocRequest {
    pub content: String,
    pub progress_json: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
}

pub async fn create_doc(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDocRequest>,
) -> axum::response::Response {
    match state
        .db
        .create_requirement_doc(
            &body.change_id,
            body.session_id.as_deref(),
            &body.name,
            &body.content,
            body.progress_json.as_deref(),
        )
        .await
    {
        Ok(doc) => R::created(doc),
        Err(e) => R::internal_error(e),
    }
}

pub async fn update_doc(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDocRequest>,
) -> axum::response::Response {
    let source = body.source.as_deref().unwrap_or("system");
    match state
        .db
        .update_requirement_doc(
            &id,
            &body.content,
            body.progress_json.as_deref(),
            body.status.as_deref(),
            source,
        )
        .await
    {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

pub async fn get_doc_by_change(
    State(state): State<Arc<AppState>>,
    Path(change_id): Path<String>,
) -> axum::response::Response {
    match state.db.get_requirement_doc_by_change(&change_id).await {
        Ok(Some(doc)) => R::ok(doc),
        Ok(None) => R::not_found("doc not found"),
        Err(e) => R::internal_error(e),
    }
}

// ─── Admin endpoints (read-only) ────────────────────────────────────

#[derive(Deserialize)]
pub struct ListDocsQuery {
    pub search: Option<String>,
    pub status: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Serialize)]
pub struct PaginatedDocs {
    pub items: Vec<hank_db::RequirementDoc>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

pub async fn admin_list_docs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListDocsQuery>,
) -> axum::response::Response {
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(20);
    match state
        .db
        .list_requirement_docs(q.search.as_deref(), q.status.as_deref(), page, page_size)
        .await
    {
        Ok((items, total)) => R::ok(PaginatedDocs {
            items,
            total,
            page,
            page_size,
        }),
        Err(e) => R::internal_error(e),
    }
}

pub async fn admin_get_doc(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state.db.get_requirement_doc(&id).await {
        Ok(Some(doc)) => R::ok(doc),
        Ok(None) => R::not_found("doc not found"),
        Err(e) => R::internal_error(e),
    }
}

#[derive(Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<String>,
    pub change_id: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Serialize)]
pub struct PaginatedTasks {
    pub items: Vec<hank_db::ChangeTask>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

pub async fn admin_list_tasks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListTasksQuery>,
) -> axum::response::Response {
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(20);
    match state
        .db
        .list_all_tasks(q.status.as_deref(), q.change_id.as_deref(), page, page_size)
        .await
    {
        Ok((items, total)) => R::ok(PaginatedTasks {
            items,
            total,
            page,
            page_size,
        }),
        Err(e) => R::internal_error(e),
    }
}
