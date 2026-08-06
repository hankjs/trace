//! handy `/api/v1/*` REST client。
//!
//! 契约要点（与 handy 侧 `app/api/public.py` 逐字段对齐）：
//! - 成功：HTTP 200 + `{"code":0,"msg":"ok","data":...}`
//! - 业务错误：HTTP 200 + `{"code":400,"msg":"中文原因","data":null}`，判成功必须看 code==0
//! - 401（token 无效）/ 422（字段错）是 FastAPI 默认格式，非信封
//! - 认证：`Authorization: Bearer <token>`

use crate::config::HandyConfig;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct HandyApi {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

/// upsert_topic 的返回。
#[derive(Debug, Clone, PartialEq)]
pub struct TopicRef {
    pub topic_id: String,
    pub created: bool,
}

/// 建卡 / 原地刷新入参。空串 title/detail 在 handy 侧语义是「保留旧值」，
/// 所以这里用 Option：None 就不传该字段。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CardUpdate {
    pub topic_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_text: Option<String>,
}

/// 开人工闸门入参。resume_ref 只放 `trace_interaction_id` 一个键——
/// handy 侧会注入覆盖 `final_answer` / `partial_answers`，这两个键名禁用。
#[derive(Debug, Clone, Serialize)]
pub struct OpenInteraction<'a> {
    pub topic_id: &'a str,
    pub kind: &'a str,
    pub question: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// 多问题（≤5 题），每题 {id, question, options}；与单问题的 options 互斥。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub questions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<Value>,
    // ttl_minutes 故意不暴露：handy 是异步渠道，不适用微信式 5 分钟 TTL。
}

/// handy 交互单（GET /interactions/{id} 与 open 的返回同构）。
#[derive(Debug, Clone, Deserialize)]
pub struct HandyInteraction {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub resume_ref: Option<Value>,
}

/// handy 话题留言（webhook `message.created` 的载体）。
/// role 恒为 user（webhook 层已校验），author 目前不消费，不随结构体传递。
#[derive(Debug, Clone)]
pub struct HandyMessage {
    pub id: String,
    pub content: String,
}

impl HandyApi {
    pub fn new(config: &HandyConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            token: config.token.clone(),
        }
    }

    /// 建或取话题（按 external_id 幂等；title 只在新建时写入）。
    pub async fn upsert_topic(&self, external_id: &str, title: &str) -> Result<TopicRef> {
        let data = self
            .post(
                "/api/v1/topics",
                &serde_json::json!({"external_id": external_id, "title": title}),
            )
            .await?;
        Ok(TopicRef {
            topic_id: data["topic_id"]
                .as_str()
                .ok_or_else(|| anyhow!("handy topics 响应缺少 topic_id"))?
                .to_string(),
            created: data["created"].as_bool().unwrap_or(false),
        })
    }

    /// 建卡（card_id=None）或原地刷新。返回 card_id。
    /// 注意 handy 侧不节流，调用方自控频率。
    pub async fn upsert_card(&self, update: &CardUpdate) -> Result<String> {
        let data = self.post("/api/v1/cards", update).await?;
        data["card_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("handy cards 响应缺少 card_id"))
    }

    /// 开人工闸门（确认 / ask_user / task_gate）。
    pub async fn open_interaction(&self, req: &OpenInteraction<'_>) -> Result<HandyInteraction> {
        let data = self.post("/api/v1/interactions", req).await?;
        Ok(serde_json::from_value(data)?)
    }

    /// 轮询交互单状态（webhook 的兜底路径）。
    pub async fn get_interaction(&self, interaction_id: &str) -> Result<HandyInteraction> {
        let data = self
            .get(&format!("/api/v1/interactions/{interaction_id}"))
            .await?;
        Ok(serde_json::from_value(data)?)
    }

    // GET /api/v1/topics/{topic_id}/messages 在 handy 侧保留（人工兜底拉取），
    // trace 不再使用：入站留言已由 handy 的 message.created webhook 主动推送。

    /// 往话题发一条 assistant 消息（入站路由的回复 / 定时任务推送用）。
    /// author 不传，handy 侧默认取 token.name。
    pub async fn post_message(&self, topic_id: &str, content: &str) -> Result<String> {
        let data = self
            .post(
                "/api/v1/messages",
                &serde_json::json!({
                    "topic_id": topic_id,
                    "role": "assistant",
                    "content": content,
                }),
            )
            .await?;
        data["message_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("handy messages 响应缺少 message_id"))
    }

    async fn post<T: Serialize + ?Sized>(&self, path: &str, body: &T) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        parse_envelope(status.as_u16(), &body)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        parse_envelope(status.as_u16(), &body)
    }
}

/// 统一解信封：HTTP 200 且 code==0 → Ok(data)；code!=0 → Err(msg)；
/// 非 200（401/422 等 FastAPI 默认格式，非信封）→ Err(状态码 + 摘要）。
fn parse_envelope(status: u16, body: &[u8]) -> Result<Value> {
    if status != 200 {
        let snippet: String = String::from_utf8_lossy(body).chars().take(200).collect();
        bail!("handy HTTP {status}: {snippet}");
    }
    let v: Value = serde_json::from_slice(body)?;
    match v.get("code").and_then(|c| c.as_i64()) {
        Some(0) => Ok(v.get("data").cloned().unwrap_or(Value::Null)),
        Some(code) => {
            let msg = v["msg"].as_str().unwrap_or("未知错误").to_string();
            bail!("handy 业务错误 code={code}: {msg}")
        }
        None => bail!("handy 响应缺少 code 字段"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_success_returns_data() {
        let body = br#"{"code":0,"msg":"ok","data":{"topic_id":"t1","created":true}}"#;
        let data = parse_envelope(200, body).unwrap();
        assert_eq!(data["topic_id"], "t1");
        assert_eq!(data["created"], true);
    }

    #[test]
    fn envelope_business_error_is_err_even_on_200() {
        let body = r#"{"code":400,"msg":"话题不存在","data":null}"#.as_bytes();
        let err = parse_envelope(200, body).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("话题不存在"));
        assert!(msg.contains("code=400"));
    }

    #[test]
    fn envelope_non_200_is_err_with_status() {
        // 401：token 无效，FastAPI 默认格式（非信封）
        let err = parse_envelope(401, r#"{"detail":"token 无效"}"#.as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("HTTP 401"));
        // 422：字段错
        let err = parse_envelope(422, br#"{"detail":[{"loc":["body","question"]}]}"#)
            .unwrap_err();
        assert!(format!("{err:#}").contains("HTTP 422"));
    }

    #[test]
    fn card_update_skips_none_fields() {
        let update = CardUpdate {
            topic_id: "t1".into(),
            card_id: Some("c1".into()),
            status: "running".into(),
            progress: Some(42),
            ..Default::default()
        };
        let v = serde_json::to_value(&update).unwrap();
        assert_eq!(v["card_id"], "c1");
        assert_eq!(v["progress"], 42);
        assert!(v.get("title").is_none());
        assert!(v.get("detail").is_none());
        assert!(v.get("activities").is_none());
    }

    #[test]
    fn open_interaction_omits_ttl_and_empty_optionals() {
        let req = OpenInteraction {
            topic_id: "t1",
            kind: "ask_user",
            question: "继续吗？",
            title: None,
            options: Some(vec!["继续".into(), "停止".into()]),
            questions: None,
            resume_ref: Some(serde_json::json!({"trace_interaction_id": "i1"})),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["kind"], "ask_user");
        assert_eq!(v["resume_ref"]["trace_interaction_id"], "i1");
        assert!(v.get("ttl_minutes").is_none());
        assert!(v.get("title").is_none());
    }
}
