//! 团队任务运行时配置的 admin REST。
//!
//! GET / PATCH `/api/admin/team-task/config`。

use super::settings;
use super::ROLE_DEFS;
use crate::auth::Claims;
use crate::response::{self as R};
use crate::AppState;
use axum::{extract::State, response::IntoResponse, Extension, Json};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// GET /api/admin/team-task/config — 当前生效配置 + 元信息
pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (config, source) = settings::effective_with_source(&state).await;
    R::ok(json!({
        "config": config,
        "source": source,
        "role_options": role_options(),
        "gate_options": gate_options(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateConfigBody {
    pub task_gate_enabled: Option<bool>,
    pub enabled: Option<bool>,
    pub roles: Option<Vec<String>>,
    pub gates: Option<Vec<String>>,
    pub max_dev_rounds: Option<i32>,
    pub dashboard_base_url: Option<Option<String>>,
}

/// PATCH /api/admin/team-task/config — 改配置。校验失败回 400。
///
/// 校验从启动时改成写入时——点保存立刻看到错误，而不是重启后起不来。
pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<UpdateConfigBody>,
) -> impl IntoResponse {
    let mut next = settings::effective(&state).await;

    if let Some(v) = body.task_gate_enabled {
        next.task_gate_enabled = v;
    }
    if let Some(v) = body.enabled {
        next.enabled = v;
    }
    if let Some(v) = body.roles {
        next.roles = v;
    }
    if let Some(v) = body.gates {
        next.gates = v;
    }
    if let Some(v) = body.max_dev_rounds {
        next.max_dev_rounds = v;
    }
    if let Some(v) = body.dashboard_base_url {
        next.dashboard_base_url = v;
    }
    next.updated_by = Some(claims.username.clone());

    if let Err(msg) = settings::validate(&next) {
        return R::bad_request(msg);
    }

    match state.db.save_team_task_settings(&next).await {
        Ok(()) => R::ok(json!({
            "config": next,
            "source": "db",
            "role_options": role_options(),
            "gate_options": gate_options(),
        })),
        Err(e) => R::internal_error(e),
    }
}

fn role_options() -> Vec<serde_json::Value> {
    ROLE_DEFS
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "label": r.label,
            })
        })
        .collect()
}

fn gate_options() -> Vec<serde_json::Value> {
    // 文案与 GateBoundary 对齐；前端不要硬编码
    vec![
        json!({
            "id": "dev_start",
            "label": "开发前（分析后确认是否开始修）",
        }),
        json!({
            "id": "review_start",
            "label": "进入评审前",
        }),
        json!({
            "id": "dev_restart",
            "label": "评审打回后重新开发前",
        }),
        json!({
            "id": "test_start",
            "label": "进入测试前",
        }),
    ]
}
