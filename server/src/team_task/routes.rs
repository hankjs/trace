//! 团队任务 REST：运行时配置 + 看板数据接口。
//!
//! 配置：GET / PATCH `/api/admin/team-task/config`（05a）
//! 看板：GET `/api/team/tasks` 等（本文件后半，路径前缀刻意不同——见 B2.1）

use super::orchestrator;
use super::settings;
use super::{
    is_terminal, ROLE_DEFS, STATUS_CANCELLED, STATUS_DONE, STATUS_FAILED, STATUS_PENDING_CONFIRM,
    STATUS_PENDING_DEV_GATE, STATUS_PENDING_REVIEW_GATE, STATUS_PENDING_TEST_GATE,
};
use crate::admin::PaginatedResponse;
use crate::auth::Claims;
use crate::interactions::parse_filter_param;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 配置（05a，保持不动）
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 看板 REST
// ---------------------------------------------------------------------------

/// 团队任务 9 个合法 status（筛选白名单）。
const TEAM_TASK_STATUSES: &[&str] = &[
    STATUS_PENDING_CONFIRM,
    "running_developer",
    STATUS_PENDING_REVIEW_GATE,
    "running_reviewer",
    STATUS_PENDING_DEV_GATE,
    STATUS_PENDING_TEST_GATE,
    "running_tester",
    STATUS_DONE,
    STATUS_FAILED,
    STATUS_CANCELLED,
];

const DEFAULT_PER_PAGE: u32 = 20;
const MAX_PER_PAGE: u32 = 100;

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<String>,
    pub user_id: Option<String>,
    pub issue_key: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

/// `per_page` 归一：0 / 缺省 → 默认 20，超 100 → 截到 100。
pub(crate) fn normalize_per_page(raw: Option<u32>) -> u32 {
    match raw {
        None | Some(0) => DEFAULT_PER_PAGE,
        Some(n) => n.min(MAX_PER_PAGE),
    }
}

/// 仅 `failed` 可重试。抽成纯函数便于单测。
pub(crate) fn can_retry(status: &str) -> bool {
    status == STATUS_FAILED
}

/// GET /api/team/tasks — 列表，支持 status / user_id / issue_key 筛选 + 分页
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTasksQuery>,
) -> impl IntoResponse {
    let status = match parse_filter_param(query.status.as_deref(), TEAM_TASK_STATUSES, "status") {
        Ok(v) => v,
        Err(msg) => return R::bad_request(msg),
    };
    let user_id = query
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let issue_key = query
        .issue_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let page = query.page.unwrap_or(1).max(1);
    let per_page = normalize_per_page(query.per_page);

    match state
        .db
        .list_team_tasks(status, user_id, issue_key, page, per_page)
        .await
    {
        Ok((rows, total)) => R::ok(PaginatedResponse {
            data: rows,
            total: total as u64,
            page,
            per_page,
        }),
        Err(e) => R::internal_error(e),
    }
}

/// GET /api/team/tasks/{task_no} — 详情：任务 + runs + events
pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(task_no): Path<String>,
) -> impl IntoResponse {
    let task = match state.db.get_team_task_by_no(&task_no).await {
        Ok(Some(t)) => t,
        Ok(None) => return R::not_found(format!("任务不存在: {task_no}")),
        Err(e) => return R::internal_error(e),
    };
    let runs = match state.db.list_team_runs(&task.id).await {
        Ok(r) => r,
        Err(e) => return R::internal_error(e),
    };
    let events = match state.db.list_team_events(&task.id).await {
        Ok(e) => e,
        Err(e) => return R::internal_error(e),
    };
    R::ok(json!({
        "task": task,
        "runs": runs,
        "events": events,
    }))
}

/// POST /api/team/tasks/{task_no}/cancel — 走编排器，终态幂等
pub async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(task_no): Path<String>,
) -> impl IntoResponse {
    let task = match state.db.get_team_task_by_no(&task_no).await {
        Ok(Some(t)) => t,
        Ok(None) => return R::not_found(format!("任务不存在: {task_no}")),
        Err(e) => return R::internal_error(e),
    };

    // 终态再取消：decide_next 判 Ignore，返回 200 + 当前状态即可
    if is_terminal(&task.status) {
        return R::ok(json!({
            "task_no": task.task_no,
            "status": task.status,
            "message": "任务已是终态，无需取消",
        }));
    }

    if let Err(e) = orchestrator::advance(
        &state,
        &task.id,
        orchestrator::Trigger::Cancelled {
            operator: claims.username.clone(),
        },
    )
    .await
    {
        return R::internal_error(e);
    }

    // 重读最新状态
    match state.db.get_team_task(&task.id).await {
        Ok(Some(t)) => R::ok(json!({
            "task_no": t.task_no,
            "status": t.status,
        })),
        Ok(None) => R::ok(json!({
            "task_no": task_no,
            "status": STATUS_CANCELLED,
        })),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/team/tasks/{task_no}/retry — 仅 failed 可用，从当前角色 round+1 重派
pub async fn retry_task(
    State(state): State<Arc<AppState>>,
    Path(task_no): Path<String>,
) -> impl IntoResponse {
    let task = match state.db.get_team_task_by_no(&task_no).await {
        Ok(Some(t)) => t,
        Ok(None) => return R::not_found(format!("任务不存在: {task_no}")),
        Err(e) => return R::internal_error(e),
    };

    if !can_retry(&task.status) {
        return R::bad_request(format!("仅 failed 状态可重试，当前 status={}", task.status));
    }

    match orchestrator::retry_from_current_role(&state, &task.id).await {
        Ok(true) => match state.db.get_team_task(&task.id).await {
            Ok(Some(t)) => R::ok(json!({
                "task_no": t.task_no,
                "status": t.status,
                "current_role": t.current_role,
                "message": "已从当前角色重试",
            })),
            Ok(None) => R::ok(json!({ "task_no": task_no, "message": "已派发" })),
            Err(e) => R::internal_error(e),
        },
        Ok(false) => {
            // 已有在途派发：409 Conflict
            (
                axum::http::StatusCode::CONFLICT,
                Json(json!({
                    "code": 409,
                    "msg": "已有在途派发，请稍后重试",
                    "data": null,
                })),
            )
                .into_response()
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("仅 failed") || msg.contains("没有可重试") {
                R::bad_request(msg)
            } else {
                R::internal_error(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_whitelist_accepts_known_rejects_bogus() {
        assert_eq!(
            parse_filter_param(None, TEAM_TASK_STATUSES, "status").unwrap(),
            None
        );
        assert_eq!(
            parse_filter_param(Some(""), TEAM_TASK_STATUSES, "status").unwrap(),
            None
        );
        assert_eq!(
            parse_filter_param(Some("  "), TEAM_TASK_STATUSES, "status").unwrap(),
            None
        );
        assert_eq!(
            parse_filter_param(Some("failed"), TEAM_TASK_STATUSES, "status").unwrap(),
            Some("failed")
        );
        assert_eq!(
            parse_filter_param(Some("running_developer"), TEAM_TASK_STATUSES, "status").unwrap(),
            Some("running_developer")
        );
        let err = parse_filter_param(Some("bogus"), TEAM_TASK_STATUSES, "status").unwrap_err();
        assert!(err.contains("非法 status"), "{err}");
    }

    #[test]
    fn per_page_normalization() {
        assert_eq!(normalize_per_page(None), 20);
        assert_eq!(normalize_per_page(Some(0)), 20);
        assert_eq!(normalize_per_page(Some(1)), 1);
        assert_eq!(normalize_per_page(Some(50)), 50);
        assert_eq!(normalize_per_page(Some(100)), 100);
        assert_eq!(normalize_per_page(Some(101)), 100);
        assert_eq!(normalize_per_page(Some(9999)), 100);
    }

    #[test]
    fn can_retry_only_failed() {
        assert!(can_retry("failed"));
        for s in [
            "pending_confirm",
            "running_developer",
            "pending_review_gate",
            "running_reviewer",
            "pending_dev_gate",
            "pending_test_gate",
            "running_tester",
            "done",
            "cancelled",
        ] {
            assert!(!can_retry(s), "should reject {s}");
        }
    }
}
