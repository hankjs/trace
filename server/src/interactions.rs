//! Admin 交互单 REST：列表 / 详情 / 手动应答 / 取消。
//!
//! 手动应答必须走 `interaction_flow::answer_and_resume`，与飞书按钮回调共用
//! 抢名额 → 应答 → 派发 → 失败回滚，避免只改库状态导致任务永不执行。
//!
//! 文件后半是 client 级交互单端点（protected 组，不需 admin）：
//! 第三方系统通过 client API 驱动 trace 时查询 / 应答自己会话的交互单。
//! 与 admin 端点的差别只在归属校验（会话必须属于当前 JWT 用户）与返回字段口径。

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
use chrono::{DateTime, Utc};
use hank_db::AgentInteraction;
use serde::{Deserialize, Serialize};
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

/// answer 白名单校验（纯函数，admin 与 client 端点共用）。
///
/// 多问题单：options 列存扁平 token 全集（["1A","1B","2A",…]），合法答案是
/// 组合串（如 "1A 2B"）而非枚举——逐一枚举会组合爆炸。有 resume_ref.questions
/// 时跳过白名单校验，交给 answer_and_resume / 下游 parse。
pub fn validate_answer(
    options: &[String],
    resume_ref: Option<&str>,
    answer: &str,
) -> Result<(), String> {
    let is_multi = resume_ref
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|v| v.get("questions").cloned())
        .and_then(|q| q.as_array().map(|a| !a.is_empty()))
        .unwrap_or(false);
    if !is_multi && !options.iter().any(|o| o == answer) {
        return Err(format!(
            "answer 不在 options 内（允许：{}）",
            options.join(" / ")
        ));
    }
    Ok(())
}

/// 会话归属校验：session 存在且属于当前用户；不属于一律按 404 处理，不暴露存在性。
pub fn session_owned_by(session: &hank_db::Session, user_id: &str) -> bool {
    session.user_id.as_deref() == Some(user_id)
}

/// client 级端点的交互单视图。字段口径参照 admin 详情，但 options 解析成数组、
/// question / questions 从 resume_ref 取出（与 SSE interaction_created 事件同形）。
#[derive(Debug, Serialize)]
pub struct SessionInteraction {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub question: String,
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<serde_json::Value>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

fn to_session_interaction(row: &AgentInteraction) -> SessionInteraction {
    let resume = row
        .resume_ref
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let question = resume
        .as_ref()
        .and_then(|v| v.get("question"))
        .and_then(|q| q.as_str())
        .unwrap_or(&row.title)
        .to_string();
    let questions = resume
        .as_ref()
        .and_then(|v| v.get("questions"))
        .and_then(|q| q.as_array().cloned())
        .unwrap_or_default();
    let options = serde_json::from_str(&row.options).unwrap_or_default();
    SessionInteraction {
        id: row.id.clone(),
        kind: row.kind.clone(),
        status: row.status.clone(),
        question,
        options,
        questions,
        expires_at: row.expires_at,
        created_at: row.created_at,
    }
}

#[derive(Debug, Deserialize)]
pub struct SessionInteractionQuery {
    pub status: Option<String>,
}

/// GET /api/sessions/{id}/interactions?status=pending
///
/// 默认只返回 pending；status 走与 admin 列表相同的白名单。
/// 会话不存在或不属于当前用户一律 404。
pub async fn list_session_interactions(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionInteractionQuery>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    let status = match parse_filter_param(query.status.as_deref(), STATUSES, "status") {
        Ok(v) => v.or(Some("pending")),
        Err(msg) => return R::bad_request(msg),
    };
    match state.db.get_session(&session_id).await {
        Ok(Some(session)) if session_owned_by(&session, &claims.sub) => {}
        Ok(_) => return R::not_found("会话不存在"),
        Err(e) => return R::internal_error(e),
    }
    match state
        .db
        .list_interactions_by_session(&session_id, status)
        .await
    {
        Ok(rows) => R::ok(rows.iter().map(to_session_interaction).collect::<Vec<_>>()),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/sessions/{id}/interactions/{iid}/answer  body: {"answer":"确认"}
///
/// 与 admin 应答同一口径：options 白名单校验后走 answer_and_resume 真派发；
/// 操作者标识用当前 JWT 用户 id。会话 / 交互单归属不满足一律 404。
pub async fn answer_session_interaction(
    State(state): State<Arc<AppState>>,
    Path((session_id, interaction_id)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<AnswerBody>,
) -> impl IntoResponse {
    let answer = body.answer.trim();
    if answer.is_empty() {
        return R::bad_request("answer 不能为空");
    }
    match state.db.get_session(&session_id).await {
        Ok(Some(session)) if session_owned_by(&session, &claims.sub) => {}
        Ok(_) => return R::not_found("会话不存在"),
        Err(e) => return R::internal_error(e),
    }
    let row = match state.db.get_interaction(&interaction_id).await {
        Ok(Some(row)) if row.session_id == session_id => row,
        Ok(_) => return R::not_found("交互单不存在"),
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
    if let Err(msg) = validate_answer(&options, row.resume_ref.as_deref(), answer) {
        return R::bad_request(msg);
    }
    match interaction_flow::answer_and_resume(&state, &interaction_id, answer, &claims.sub, None)
        .await
    {
        Ok(()) => match state.db.get_interaction(&interaction_id).await {
            Ok(Some(updated)) => R::ok(to_session_interaction(&updated)),
            Ok(None) => R::ok(to_session_interaction(&row)),
            Err(e) => R::internal_error(e),
        },
        Err(e) => R::bad_request(e.message),
    }
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
    if let Err(msg) = validate_answer(&options, row.resume_ref.as_deref(), answer) {
        return R::bad_request(msg);
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
        Ok(true) => {
            // 尽力而为：卡片改灰失败不影响取消结果（库状态是权威）。
            if let Err(e) =
                interaction_flow::close_interaction_card(&state, &id, None, "已取消", "管理员")
                    .await
            {
                tracing::warn!(interaction_id = %id, "取消后改写飞书卡片失败: {e:#}");
            }
            match state.db.get_interaction(&id).await {
                Ok(Some(updated)) => R::ok(updated),
                Ok(None) => R::no_content(),
                Err(e) => R::internal_error(e),
            }
        }
        Ok(false) => R::bad_request("取消失败：交互单可能已被应答或状态已变"),
        Err(e) => R::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_filter_param, session_owned_by, validate_answer, CHANNELS, KINDS, STATUSES};

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

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn validate_answer_accepts_whitelisted_option() {
        let options = strs(&["确认", "否"]);
        assert!(validate_answer(&options, None, "确认").is_ok());
    }

    #[test]
    fn validate_answer_rejects_unknown_option() {
        let options = strs(&["确认", "否"]);
        let err = validate_answer(&options, None, "随便").unwrap_err();
        assert!(err.contains("answer 不在 options 内"), "{err}");
        assert!(err.contains("确认 / 否"), "{err}");
    }

    #[test]
    fn validate_answer_multi_question_skips_whitelist() {
        // 多问题单：options 是扁平 token 全集，合法答案是组合串，跳过白名单
        let options = strs(&["1A", "1B", "2A", "2B"]);
        let resume_ref = r#"{"tool_use_id":"t1","questions":[{"id":"1"},{"id":"2"}]}"#;
        assert!(validate_answer(&options, Some(resume_ref), "1A 2B").is_ok());
        // questions 为空数组不算多问题，仍走白名单
        let resume_ref = r#"{"tool_use_id":"t1","questions":[]}"#;
        assert!(validate_answer(&options, Some(resume_ref), "1A 2B").is_err());
    }

    #[test]
    fn session_owned_by_requires_exact_user() {
        let session = |user_id: Option<&str>| hank_db::Session {
            id: "s1".to_string(),
            user_id: user_id.map(str::to_string),
            title: "t".to_string(),
            provider: "p".to_string(),
            model: "m".to_string(),
            work_dir: None,
            local_agent: None,
            local_work_dir: None,
            environment: String::new(),
            session_type: String::new(),
            change_id: None,
            pending_ask_user: None,
            active_leaf_id: None,
            metadata: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(session_owned_by(&session(Some("u1")), "u1"));
        assert!(!session_owned_by(&session(Some("u1")), "u2"));
        // user_id 为 NULL 的会话不归任何人，一律 404
        assert!(!session_owned_by(&session(None), "u1"));
    }
}
