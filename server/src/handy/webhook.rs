//! handy webhook：交互单应答回推 + 话题留言推送的入口（public 路由，无 JWT）。
//!
//! 路由：`POST /api/channels/handy/{user_id}/webhook`——按 user_id 解析
//! handy_accounts 行（不存在 404 / 停用 403），用该行的 webhook_secret 验签。
//! handy 约定：POST 原始 body + `X-Handy-Signature: hex(HMAC-SHA256(webhook_secret, body))`
//! （纯 hex，无 `sha256=` 前缀），重试 3 次（1s/2s）后放弃。
//! - `interaction.answered`：闸门应答 → answer_and_resume；scheduler 另有 30s
//!   轮询兜底，两条路径都汇到 `answer_trace_interaction`
//!   （原子应答天然幂等，重复投递无害）。
//! - `message.created`：用户在外部话题的留言。**handy 收到 2xx 即标已读、不再重推**，
//!   所以 handler 只做验签/解析/幂等去重就快速返回，实际处理挪到异步 task
//!   （同步处理失败会把消息丢掉）。两类事件都按各自 id 去重（TTL 10min）。

use crate::handy::client::{HandyApi, HandyMessage};
use crate::handy::router;
use crate::interaction_flow;
use crate::AppState;
use anyhow::{bail, Result};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type HmacSha256 = Hmac<Sha256>;

/// 去重窗口：handy 重试都在几秒内到达，10 分钟绰绰有余
const DEDUPE_TTL: Duration = Duration::from_secs(600);

pub async fn webhook_handler(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let account = match state.db.get_handy_account(&user_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return (StatusCode::NOT_FOUND, "handy account not found").into_response(),
        Err(e) => {
            tracing::warn!("handy webhook 读取账号失败: {e:#}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
    };
    if !account.enabled {
        return (StatusCode::FORBIDDEN, "handy account disabled").into_response();
    }
    let signature = headers
        .get("X-Handy-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_signature(&account.webhook_secret, &body, signature) {
        tracing::warn!(user_id, "handy webhook 验签失败");
        return (StatusCode::UNAUTHORIZED, "invalid signature").into_response();
    }

    let payload = match parse_webhook_payload(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("handy webhook body 解析失败: {e:#}");
            return (StatusCode::BAD_REQUEST, "invalid payload").into_response();
        }
    };
    match payload.event_type.as_str() {
        "interaction.answered" => handle_interaction_answered(&state, account, payload),
        "message.created" => handle_message_created(&state, account, payload),
        // 其他事件类型本阶段不消费，正常接收避免 handy 重试
        _ => StatusCode::OK.into_response(),
    }
}

/// interaction.answered：按 handy 交互单 id 去重后立即 2xx，应答挪到异步 task。
fn handle_interaction_answered(
    state: &Arc<AppState>,
    account: hank_db::HandyAccount,
    payload: WebhookPayload,
) -> Response {
    if payload.interaction_id.is_empty() {
        tracing::warn!("handy interaction.answered 缺少 interaction_id");
        return (StatusCode::BAD_REQUEST, "missing interaction_id").into_response();
    }
    // 幂等：重试按 handy 交互单 id 去重（trace 侧原子应答还有第二道防线）
    {
        let mut dedupe = webhook_dedupe().lock().unwrap();
        let key = format!("{}:ia:{}", account.user_id, payload.interaction_id);
        if !dedupe.check_and_insert(&key, Instant::now()) {
            return StatusCode::OK.into_response();
        }
    }
    let state = state.clone();
    tokio::spawn(async move {
        // 优先 payload 里 handy 透传的 resume_ref.trace_interaction_id；
        // 丢失时退化用 resume_ref 映射反查（应对 handy 侧映射残缺）。
        let trace_interaction_id = match extract_trace_interaction_id(&payload) {
            Some(id) => Some(id),
            None => match state
                .db
                .find_handy_gate_by_ref(&payload.interaction_id)
                .await
            {
                Ok(Some(row)) => Some(row.id),
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!("handy 反查交互单映射失败: {e:#}");
                    None
                }
            },
        };
        let Some(trace_interaction_id) = trace_interaction_id else {
            tracing::warn!(
                handy_interaction_id = %payload.interaction_id,
                "handy webhook 缺少 resume_ref.trace_interaction_id 且反查无果"
            );
            return;
        };
        answer_trace_interaction(&state, &trace_interaction_id, &payload.answer, &account.user_id)
            .await;
    });
    StatusCode::OK.into_response()
}

/// message.created：验签/解析/去重后立即 2xx，派发挪到异步 task。
/// handy 收到 2xx 就标已读，同步处理失败会把这条留言丢掉。
fn handle_message_created(
    state: &Arc<AppState>,
    account: hank_db::HandyAccount,
    payload: WebhookPayload,
) -> Response {
    if payload.topic_id.is_empty() || payload.message_id.is_empty() {
        tracing::warn!("handy message.created 缺少 topic_id / message_id");
        return (StatusCode::BAD_REQUEST, "missing topic_id or message_id").into_response();
    }
    // 契约上外部话题的留言 role 恒为 user；防御性跳过其他 role
    if payload.role != "user" {
        return StatusCode::OK.into_response();
    }
    // 幂等：重试（最多 3 次，几秒内到达）按 message_id 去重，直接 2xx 不再派发
    {
        let mut dedupe = webhook_dedupe().lock().unwrap();
        let key = format!("{}:msg:{}", account.user_id, payload.message_id);
        if !dedupe.check_and_insert(&key, Instant::now()) {
            return StatusCode::OK.into_response();
        }
    }
    let api = HandyApi::new(&account.base_url, &account.token);
    let user_id = account.user_id.clone();
    let topic_id = payload.topic_id.clone();
    let msg = HandyMessage {
        id: payload.message_id,
        content: payload.content,
    };
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = router::handle_user_message(&state, &api, &user_id, &topic_id, &msg).await {
            tracing::warn!(
                topic_id,
                message_id = %msg.id,
                "handy 留言派发失败（已标已读，不会重推）: {e:#}"
            );
        }
    });
    StatusCode::OK.into_response()
}

/// 进程内带 TTL 的去重集合（message_id / 交互单 id 共用，key 带类型与
/// user_id 前缀）。重试都在几秒内到达，不需要落表；
/// 重启丢窗口的风险 = 重启瞬间的重试可能重复处理一次，可接受
/// （消息侧表现为多派一轮，应答侧有原子应答兜底）。
struct WebhookDedupe {
    seen: HashMap<String, Instant>,
    ttl: Duration,
}

impl WebhookDedupe {
    fn new(ttl: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            ttl,
        }
    }

    /// true = 首次见到（已记录）；false = 窗口内的重复投递
    fn check_and_insert(&mut self, key: &str, now: Instant) -> bool {
        self.sweep(now);
        self.seen.insert(key.to_string(), now).is_none()
    }

    fn sweep(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.seen.retain(|_, at| now.duration_since(*at) < ttl);
    }
}

fn webhook_dedupe() -> &'static Mutex<WebhookDedupe> {
    static DEDUPE: OnceLock<Mutex<WebhookDedupe>> = OnceLock::new();
    DEDUPE.get_or_init(|| Mutex::new(WebhookDedupe::new(DEDUPE_TTL)))
}

/// webhook 与兜底轮询共用的应答入口：调现成的 answer_and_resume。
/// channel_ctx 传 None（handy 不是飞书卡片渠道，patch/restore 卡片逻辑对
/// 非 feishu 渠道自动短路）；resume 后的事件流由 run_chat_turn 的
/// source=="handy" hook 自动挂 pusher 消费。
/// operator_user_id 传 handy 账号属主：resume 派发的内部 JWT 以它签发。
/// 重复投递由原子应答（UPDATE WHERE status='pending'）去重，失败仅记日志。
pub async fn answer_trace_interaction(
    state: &Arc<AppState>,
    trace_interaction_id: &str,
    answer: &str,
    operator_user_id: &str,
) {
    match interaction_flow::answer_and_resume(
        state,
        trace_interaction_id,
        answer,
        operator_user_id,
        None,
    )
    .await
    {
        Ok(()) => tracing::info!(
            interaction_id = %trace_interaction_id,
            "handy 应答已派发 resume"
        ),
        Err(e) => tracing::warn!(
            interaction_id = %trace_interaction_id,
            "handy 应答未生效（可能已被其他路径应答）: {e}"
        ),
    }
}

/// 验签：hex 解码后与 HMAC-SHA256(secret, body) 常量时间比较。
fn verify_signature(secret: &str, body: &[u8], signature_hex: &str) -> bool {
    let Ok(expected) = hex::decode(signature_hex) else {
        return false;
    };
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    // verify_slice 内部是常量时间比较
    mac.verify_slice(&expected).is_ok()
}

#[derive(Debug, Deserialize)]
struct WebhookPayload {
    #[serde(rename = "type")]
    event_type: String,
    // interaction.answered 字段
    #[serde(default)]
    interaction_id: String,
    #[serde(default)]
    answer: String,
    #[serde(default)]
    resume_ref: Option<Value>,
    // message.created 字段（payload 里的 author 目前不消费，serde 忽略未知字段）
    #[serde(default)]
    topic_id: String,
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
}

fn parse_webhook_payload(body: &[u8]) -> Result<WebhookPayload> {
    let payload: WebhookPayload = serde_json::from_slice(body)?;
    if payload.event_type.is_empty() {
        bail!("缺少 type 字段");
    }
    Ok(payload)
}

/// 从 resume_ref.trace_interaction_id 取 trace 交互单号（只认非空字符串）。
fn extract_trace_interaction_id(payload: &WebhookPayload) -> Option<String> {
    payload
        .resume_ref
        .as_ref()
        .and_then(|r| r["trace_interaction_id"].as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verify_signature_accepts_valid() {
        let body = br#"{"type":"interaction.answered"}"#;
        let sig = sign("s3cret", body);
        assert!(verify_signature("s3cret", body, &sig));
    }

    #[test]
    fn verify_signature_rejects_bad_inputs() {
        let body = br#"{"type":"interaction.answered"}"#;
        let sig = sign("s3cret", body);
        // 密钥不符
        assert!(!verify_signature("other", body, &sig));
        // body 被篡改
        assert!(!verify_signature("s3cret", br#"{"type":"x"}"#, &sig));
        // 非 hex / 空签名
        assert!(!verify_signature("s3cret", body, "not-hex!"));
        assert!(!verify_signature("s3cret", body, ""));
    }

    #[test]
    fn parse_payload_extracts_trace_interaction_id() {
        let body = r#"{
            "type":"interaction.answered",
            "interaction_id":"hi_1",
            "topic_id":"t1",
            "kind":"confirm",
            "status":"answered",
            "answer":"确认",
            "partial_answers":{},
            "resume_ref":{"trace_interaction_id":"ti_9","final_answer":"确认"}
        }"#
        .as_bytes();
        let payload = parse_webhook_payload(body).unwrap();
        assert_eq!(payload.event_type, "interaction.answered");
        assert_eq!(payload.answer, "确认");
        assert_eq!(
            extract_trace_interaction_id(&payload),
            Some("ti_9".to_string())
        );
    }

    #[test]
    fn parse_payload_missing_resume_ref_or_id() {
        let payload = parse_webhook_payload(
            r#"{"type":"interaction.answered","interaction_id":"hi_1","answer":"确认"}"#
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(extract_trace_interaction_id(&payload), None);

        let payload = parse_webhook_payload(
            r#"{"type":"interaction.answered","answer":"确认","resume_ref":{"trace_interaction_id":""}}"#
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(extract_trace_interaction_id(&payload), None);
    }

    #[test]
    fn parse_payload_rejects_garbage() {
        assert!(parse_webhook_payload(b"not json").is_err());
        assert!(parse_webhook_payload(br#"{"foo":1}"#).is_err());
    }

    #[test]
    fn parse_message_created_payload() {
        let body = r#"{
            "type":"message.created",
            "topic_id":"tp_1",
            "message_id":"msg_1",
            "role":"user",
            "content":"帮我查一下库存",
            "author":"idt_9",
            "created_at":"2026-08-06T00:43:58.123456"
        }"#
        .as_bytes();
        let payload = parse_webhook_payload(body).unwrap();
        assert_eq!(payload.event_type, "message.created");
        assert_eq!(payload.topic_id, "tp_1");
        assert_eq!(payload.message_id, "msg_1");
        assert_eq!(payload.role, "user");
        assert_eq!(payload.content, "帮我查一下库存");
        // payload 里的 author / created_at 不消费，未知字段被忽略
    }

    #[test]
    fn dedupe_rejects_replay_within_ttl() {
        let mut d = WebhookDedupe::new(Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(d.check_and_insert("u1:msg:msg_1", t0));
        // 窗口内的重试（handy 重试间隔 1s/2s）被去重
        assert!(!d.check_and_insert("u1:msg:msg_1", t0 + Duration::from_secs(3)));
        // 不同类型 / 不同用户 / 不同消息互不影响
        assert!(d.check_and_insert("u1:ia:msg_1", t0 + Duration::from_secs(3)));
        assert!(d.check_and_insert("u2:msg:msg_1", t0 + Duration::from_secs(3)));
        assert!(d.check_and_insert("u1:msg:msg_2", t0 + Duration::from_secs(3)));
    }

    #[test]
    fn dedupe_allows_after_ttl_and_sweeps() {
        let mut d = WebhookDedupe::new(Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(d.check_and_insert("u1:msg:msg_1", t0));
        // 超出窗口后同一 id 视为新消息
        assert!(d.check_and_insert("u1:msg:msg_1", t0 + Duration::from_secs(601)));
        // sweep 清掉了过期项
        assert_eq!(d.seen.len(), 1);
    }
}
