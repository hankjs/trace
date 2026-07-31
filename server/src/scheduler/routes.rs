//! 定时任务 admin HTTP 接口：列表/启停/执行记录/手动触发。
//!
//! 镜像 quant 的 /api/admin/jobs（app/api/admin.py）：手动触发 202 立即返回，
//! 执行在后台进行，前端轮询列表看状态。

use crate::auth::Claims;
use crate::response::{self as R};
use crate::scheduler::{self, TRIGGER_MANUAL};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RunsQuery {
    pub limit: Option<u32>,
}

/// GET /api/admin/jobs — 任务列表：调度信息 + 启停状态 + 最近系统/手动执行
pub async fn list_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let latest = state.db.latest_job_runs().await.unwrap_or_default();
    // (job_id, trigger) → 最新一条
    let mut latest_map: HashMap<(String, String), hank_db::JobRun> = HashMap::new();
    for r in latest {
        latest_map.insert((r.job_id.clone(), r.trigger.clone()), r);
    }
    let states = state.db.list_job_states().await.unwrap_or_default();
    let enabled_map: HashMap<String, bool> = states.into_iter().collect();
    let next_runs = state.scheduler.next_runs.read().await.clone();
    let scheduler_running = state.config.server.scheduler_enabled;

    let jobs: Vec<Value> = scheduler::JOB_DEFS
        .iter()
        .map(|j| {
            json!({
                "id": j.id,
                "name": j.name,
                "description": j.description,
                "schedule": j.schedule_label,
                "enabled": enabled_map.get(j.id).copied().unwrap_or(true),
                "next_run_time": if scheduler_running {
                    next_runs.get(j.id).map(|t| t.to_rfc3339())
                } else {
                    None
                },
                "last_system_run": latest_map.get(&(j.id.to_string(), "system".to_string())),
                "manual_run": latest_map.get(&(j.id.to_string(), "manual".to_string())),
            })
        })
        .collect();

    R::ok(json!({
        "scheduler_running": scheduler_running,
        "jobs": jobs,
    }))
}

#[derive(Deserialize)]
pub struct UpdateJobRequest {
    pub enabled: bool,
}

/// PATCH /api/admin/jobs/{id} — 启停任务
pub async fn update_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateJobRequest>,
) -> impl IntoResponse {
    if scheduler::job_def(&id).is_none() {
        return R::not_found(format!("未知任务: {id}"));
    }
    match state.db.set_job_enabled(&id, body.enabled).await {
        Ok(()) => R::no_content(),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/admin/jobs/{id}/runs?limit=20 — 执行历史（新到旧）
pub async fn job_runs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<RunsQuery>,
) -> impl IntoResponse {
    if scheduler::job_def(&id).is_none() {
        return R::not_found(format!("未知任务: {id}"));
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    match state.db.recent_job_runs(&id, limit).await {
        Ok(runs) => R::ok(runs),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/admin/jobs/{id}/run — 手动触发（202 立即返回，后台执行）
pub async fn run_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    let Some(job) = scheduler::job_def(&id) else {
        return R::not_found(format!("未知任务: {id}"));
    };
    let running = state
        .scheduler
        .locks
        .get(&id)
        .map(|l| l.load(std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(false);
    if running {
        return R::conflict("该任务已有执行进行中");
    }
    let operator = claims.sub.clone();
    tokio::spawn(async move {
        if let Err(e) = scheduler::execute_job(state, job, TRIGGER_MANUAL, Some(&operator)).await {
            tracing::warn!("scheduler: 手动触发 {id} 失败: {e}");
        }
    });
    R::ok(json!({ "status": "started" }))
}
