//! Admin 交互单 REST：列表 / 详情 / 手动应答 / 取消。
//!
//! 手动应答必须走 `interaction_flow::answer_and_resume`，与飞书按钮回调共用
//! 抢名额 → 应答 → 派发 → 失败回滚，避免只改库状态导致任务永不执行。

use crate::admin::PaginatedResponse;
use crate::auth::Claims;
use crate::interaction_flow;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;

const STATUSES: &[&str] = &[
    "pending",
    "answered",
    "executing",
    "done",
    "failed",
    "expired",
    "cancelled",
];
const KINDS: &[&str] = &["quant_confirm", "ask_user", "task_gate", "team_gate"];
const CHANNELS: &[&str] = &["feishu", "weixin", "trace_chat"];

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub channel: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct AnswerBody {
    pub answer: String,
}

/// 筛选项白名单校验。空串 / None 表示不限；非法值拒绝。
/// 抽成纯函数便于单测，避免 admin handler 里悄悄吞掉脏参数。
pub fn parse_filter_param<'a>(
    raw: Option<&'a str>,
    allowed: &[&str],
    field: &str,
) -> Result<Option<&'a str>, String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if allowed.contains(&s) {
        Ok(Some(s))
    } else {
        Err(format!("非法 {field}：{s}（允许：{}）", allowed.join("|")))
    }
}

/// GET /api/admin/interactions
pub async fn list_interactions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let status = match parse_filter_param(query.status.as_deref(), STATUSES, "status") {
        Ok(v) => v,
        Err(msg) => return R::bad_request(msg),
    };
    let kind = match parse_filter_param(query.kind.as_deref(), KINDS, "kind") {
        Ok(v) => v,
        Err(msg) => return R::bad_request(msg),
    };
    let channel = match parse_filter_param(query.channel.as_deref(), CHANNELS, "channel") {
        Ok(v) => v,
        Err(msg) => return R::bad_request(msg),
    };
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(30).clamp(1, 200);
    let (data, total) = match state
        .db
        .list_interactions(status, kind, channel, page, per_page)
        .await
    {
        Ok(result) => result,
        Err(e) => return R::internal_error(e),
    };
    R::ok(PaginatedResponse {
        data,
        total: total.max(0) as u64,
        page,
        per_page,
    })
}

/// GET /api/admin/interactions/{id}
pub async fn get_interaction(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_interaction(&id).await {
        Ok(Some(row)) => R::ok(row),
        Ok(None) => R::not_found("交互单不存在"),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/admin/interactions/{id}/answer  body: {"answer":"确认"}
///
/// 必须真派发，不能只改 status。options 白名单校验后走 answer_and_resume。
pub async fn answer_interaction(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AnswerBody>,
) -> impl IntoResponse {
    let answer = body.answer.trim();
    if answer.is_empty() {
        return R::bad_request("answer 不能为空");
    }
    let row = match state.db.get_interaction(&id).await {
        Ok(Some(row)) => row,
        Ok(None) => return R::not_found("交互单不存在"),
        Err(e) => return R::internal_error(e),
    };
    if row.status != "pending" {
        return R::bad_request(format!(
            "交互单当前状态为 {}，仅 pending 可应答",
            row.status
        ));
    }
    let options: Vec<String> = match serde_json::from_str(&row.options) {
        Ok(v) => v,
        Err(e) => return R::internal_error(format!("options 解析失败: {e}")),
    };
    if !options.iter().any(|o| o == answer) {
        return R::bad_request(format!(
            "answer 不在 options 内（允许：{}）",
            options.join(" / ")
        ));
    }
    match interaction_flow::answer_and_resume(&state, &id, answer, &claims.sub, None).await {
        Ok(()) => match state.db.get_interaction(&id).await {
            Ok(Some(updated)) => R::ok(updated),
            Ok(None) => R::ok(row),
            Err(e) => R::internal_error(e),
        },
        Err(e) => R::bad_request(e.message),
    }
}

/// POST /api/admin/interactions/{id}/cancel
pub async fn cancel_interaction(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    let row = match state.db.get_interaction(&id).await {
        Ok(Some(row)) => row,
        Ok(None) => return R::not_found("交互单不存在"),
        Err(e) => return R::internal_error(e),
    };
    if row.status != "pending" {
        return R::bad_request(format!(
            "交互单当前状态为 {}，仅 pending 可取消",
            row.status
        ));
    }
    match state.db.cancel_interaction(&id, &claims.sub).await {
        Ok(true) => match state.db.get_interaction(&id).await {
            Ok(Some(updated)) => R::ok(updated),
            Ok(None) => R::no_content(),
            Err(e) => R::internal_error(e),
        },
        Ok(false) => R::bad_request("取消失败：交互单可能已被应答或状态已变"),
        Err(e) => R::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_filter_param, CHANNELS, KINDS, STATUSES};

    #[test]
    fn filter_none_and_empty_mean_unlimited() {
        assert_eq!(parse_filter_param(None, STATUSES, "status").unwrap(), None);
        assert_eq!(
            parse_filter_param(Some(""), STATUSES, "status").unwrap(),
            None
        );
        assert_eq!(parse_filter_param(Some("  "), KINDS, "kind").unwrap(), None);
    }

    #[test]
    fn filter_allows_whitelist() {
        assert_eq!(
            parse_filter_param(Some("pending"), STATUSES, "status").unwrap(),
            Some("pending")
        );
        assert_eq!(
            parse_filter_param(Some("quant_confirm"), KINDS, "kind").unwrap(),
            Some("quant_confirm")
        );
        assert_eq!(
            parse_filter_param(Some("feishu"), CHANNELS, "channel").unwrap(),
            Some("feishu")
        );
    }

    #[test]
    fn filter_rejects_unknown() {
        let err = parse_filter_param(Some("bogus"), STATUSES, "status").unwrap_err();
        assert!(err.contains("非法 status"));
        assert!(err.contains("bogus"));
        let err = parse_filter_param(Some("slack"), CHANNELS, "channel").unwrap_err();
        assert!(err.contains("非法 channel"));
    }
}
