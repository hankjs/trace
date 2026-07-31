//! 飞书 REST API 封装：tenant_access_token 缓存 + 消息回复/卡片更新。
//!
//! 手写极简实现（对齐 weixin/api.rs 风格，不引入第三方 SDK）。

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub const FEISHU_BASE_URL: &str = "https://open.feishu.cn";

/// token 提前刷新余量（官方有效期约 2 小时）
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(300);

struct CachedToken {
    token: String,
    expires_at: Instant,
}

#[derive(Clone)]
pub struct FeishuApi {
    http: reqwest::Client,
    base: String,
    app_id: String,
    app_secret: Arc<str>,
    token: Arc<RwLock<Option<CachedToken>>>,
    archive: Option<ArchiveContext>,
}

#[derive(Clone)]
struct ArchiveContext {
    db: hank_db::Database,
    account_id: String,
    account_name: String,
}

#[derive(Deserialize)]
struct TokenResp {
    code: i64,
    msg: String,
    tenant_access_token: Option<String>,
    expire: Option<u64>,
}

#[derive(Deserialize)]
struct ApiResp {
    code: i64,
    msg: String,
    data: Option<Value>,
}

impl FeishuApi {
    pub fn new(account: &hank_db::FeishuAccount) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            base: FEISHU_BASE_URL.to_string(),
            app_id: account.app_id.clone(),
            app_secret: account.app_secret.as_str().into(),
            token: Arc::new(RwLock::new(None)),
            archive: None,
        }
    }

    /// 创建带消息留档能力的 API 客户端。留档失败只记日志，不影响飞书发送结果。
    pub fn new_archived(account: &hank_db::FeishuAccount, db: hank_db::Database) -> Self {
        let mut api = Self::new(account);
        api.archive = Some(ArchiveContext {
            db,
            account_id: account.id.clone(),
            account_name: if account.name.trim().is_empty() {
                account.app_id.clone()
            } else {
                account.name.clone()
            },
        });
        api
    }

    /// 用 app_id/app_secret 直接验证凭证有效性（admin 创建账号时调用）。
    pub async fn verify_credentials(app_id: &str, app_secret: &str) -> Result<()> {
        let resp = reqwest::Client::new()
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                FEISHU_BASE_URL
            ))
            .json(&json!({ "app_id": app_id, "app_secret": app_secret }))
            .send()
            .await?
            .json::<TokenResp>()
            .await?;
        if resp.code != 0 {
            bail!("凭证校验失败 code={} msg={}", resp.code, resp.msg);
        }
        Ok(())
    }

    /// 取 tenant_access_token，带进程内缓存与提前刷新。
    async fn tenant_token(&self) -> Result<String> {
        {
            let guard = self.token.read().await;
            if let Some(cached) = guard.as_ref() {
                if Instant::now() + TOKEN_REFRESH_MARGIN < cached.expires_at {
                    return Ok(cached.token.clone());
                }
            }
        }
        let mut guard = self.token.write().await;
        // 双检：等待写锁期间可能已被其他任务刷新
        if let Some(cached) = guard.as_ref() {
            if Instant::now() + TOKEN_REFRESH_MARGIN < cached.expires_at {
                return Ok(cached.token.clone());
            }
        }
        let resp = self
            .http
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.base
            ))
            .json(&json!({ "app_id": self.app_id, "app_secret": self.app_secret }))
            .send()
            .await?
            .json::<TokenResp>()
            .await?;
        if resp.code != 0 {
            bail!("飞书鉴权失败 code={} msg={}", resp.code, resp.msg);
        }
        let token = resp
            .tenant_access_token
            .ok_or_else(|| anyhow!("飞书鉴权响应缺少 token"))?;
        let expire = resp.expire.unwrap_or(7200);
        *guard = Some(CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expire),
        });
        Ok(token)
    }

    /// 回复文本消息，返回新消息的 message_id。
    pub async fn reply_text(
        &self,
        message_id: &str,
        text: &str,
        reply_in_thread: bool,
    ) -> Result<String> {
        self.reply_message(
            message_id,
            "text",
            json!({ "text": text }),
            reply_in_thread,
        )
        .await
    }

    /// 回复交互卡片，返回卡片消息的 message_id（后续 update_card 要用）。
    pub async fn reply_card(
        &self,
        message_id: &str,
        card: &Value,
        reply_in_thread: bool,
    ) -> Result<String> {
        self.reply_message(message_id, "interactive", card.clone(), reply_in_thread)
            .await
    }

    async fn reply_message(
        &self,
        message_id: &str,
        msg_type: &str,
        content: Value,
        reply_in_thread: bool,
    ) -> Result<String> {
        let token = self.tenant_token().await?;
        let mut body = json!({
            "msg_type": msg_type,
            "content": content.to_string(),
        });
        if reply_in_thread {
            body["reply_in_thread"] = json!(true);
        }
        let resp = self
            .http
            .post(format!(
                "{}/open-apis/im/v1/messages/{}/reply",
                self.base, message_id
            ))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await?
            .json::<ApiResp>()
            .await?;
        if resp.code != 0 {
            bail!("飞书回复消息失败 code={} msg={}", resp.code, resp.msg);
        }
        let sent_message_id = resp
            .data
            .and_then(|d| d["message_id"].as_str().map(|s| s.to_string()))
            .ok_or_else(|| anyhow!("飞书回复响应缺少 message_id"))?;
        if let Some(archive) = &self.archive {
            let content_text = archive_display_content(msg_type, &content);
            if let Err(e) = archive
                .db
                .insert_channel_reply(
                    "feishu",
                    &archive.account_id,
                    message_id,
                    &sent_message_id,
                    msg_type,
                    &content_text,
                    chrono::Utc::now(),
                )
                .await
            {
                tracing::warn!(message_id, "feishu: archive reply failed: {e:#}");
            }
        }
        Ok(sent_message_id)
    }

    /// 原地更新卡片消息（进度刷新走这里）。
    pub async fn update_card(&self, message_id: &str, card: &Value) -> Result<()> {
        let token = self.tenant_token().await?;
        let resp = self
            .http
            .patch(format!(
                "{}/open-apis/im/v1/messages/{}",
                self.base, message_id
            ))
            .bearer_auth(&token)
            .json(&json!({ "content": card.to_string() }))
            .send()
            .await?
            .json::<ApiResp>()
            .await?;
        if resp.code != 0 {
            bail!("飞书更新卡片失败 code={} msg={}", resp.code, resp.msg);
        }
        if let Some(archive) = &self.archive {
            let content_text = archive_display_content("interactive", card);
            if let Err(e) = archive
                .db
                .update_channel_message_content(
                    "feishu",
                    &archive.account_id,
                    message_id,
                    "interactive",
                    &content_text,
                )
                .await
            {
                tracing::warn!(message_id, "feishu: archive card update failed: {e:#}");
            }
        }
        Ok(())
    }

    /// 主动发送文本消息（非回复）：receive_id_type = open_id（单聊）| chat_id（群）。
    /// 主动推送（巡检结果、任务完成通知等）走这里。
    pub async fn send_text(
        &self,
        receive_id_type: &str,
        receive_id: &str,
        text: &str,
    ) -> Result<String> {
        let token = self.tenant_token().await?;
        let resp = self
            .http
            .post(format!(
                "{}/open-apis/im/v1/messages?receive_id_type={}",
                self.base, receive_id_type
            ))
            .bearer_auth(&token)
            .json(&json!({
                "receive_id": receive_id,
                "msg_type": "text",
                "content": json!({ "text": text }).to_string(),
            }))
            .send()
            .await?
            .json::<ApiResp>()
            .await?;
        if resp.code != 0 {
            bail!("飞书主动发消息失败 code={} msg={}", resp.code, resp.msg);
        }
        let data = resp.data.unwrap_or_default();
        let sent_message_id = data["message_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("飞书发送响应缺少 message_id"))?;
        if let Some(archive) = &self.archive {
            let conversation_id = data["chat_id"].as_str().unwrap_or(receive_id);
            let (user_id, session_id) = if receive_id_type == "open_id" {
                match archive.db.get_feishu_binding(&archive.account_id, receive_id).await {
                    Ok(Some(binding)) => {
                        let session = archive
                            .db
                            .get_feishu_chat(&archive.account_id, conversation_id, "main")
                            .await
                            .ok()
                            .flatten()
                            .map(|chat| chat.session_id);
                        (Some(binding.user_id), session)
                    }
                    _ => (None, None),
                }
            } else {
                (None, None)
            };
            let content_text = archive_display_content("text", &json!({ "text": text }));
            if let Err(e) = archive
                .db
                .insert_channel_outbound(
                    "feishu",
                    &archive.account_id,
                    &archive.account_name,
                    conversation_id,
                    "main",
                    &sent_message_id,
                    "text",
                    &content_text,
                    (receive_id_type == "open_id").then_some(receive_id),
                    user_id.as_deref(),
                    session_id.as_deref(),
                    chrono::Utc::now(),
                )
                .await
            {
                tracing::warn!(message_id = %sent_message_id, "feishu: archive proactive message failed: {e:#}");
            }
        }
        Ok(sent_message_id)
    }
}

fn archive_display_content(message_type: &str, content: &Value) -> String {
    if message_type == "text" {
        return content["text"].as_str().unwrap_or_default().to_string();
    }
    let mut values = Vec::new();
    if let Some(header) = content.get("header") {
        collect_card_text(header, &mut values);
    }
    if let Value::Object(map) = content {
        for (key, child) in map {
            if key != "header" {
                collect_card_text(child, &mut values);
            }
        }
    } else {
        collect_card_text(content, &mut values);
    }
    values.dedup();
    if values.is_empty() {
        format!("[{message_type}]")
    } else {
        values.join("\n")
    }
}

fn collect_card_text(value: &Value, values: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if matches!(key.as_str(), "text" | "content" | "title") {
                    if let Some(text) = child.as_str() {
                        let text = text.trim();
                        if !text.is_empty() && !values.iter().any(|item| item == text) {
                            values.push(text.to_string());
                        }
                    }
                }
                collect_card_text(child, values);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_card_text(item, values);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn base_url_is_feishu() {
        assert!(super::FEISHU_BASE_URL.contains("feishu"));
    }

    #[test]
    fn archive_content_extracts_text_from_cards() {
        let card = json!({
            "header": { "title": { "content": "Agent 任务" } },
            "elements": [
                { "tag": "markdown", "content": "执行完成" },
                { "tag": "note", "elements": [{ "tag": "plain_text", "content": "耗时 2s" }] }
            ]
        });
        assert_eq!(
            super::archive_display_content("interactive", &card),
            "Agent 任务\n执行完成\n耗时 2s"
        );
        assert_eq!(
            super::archive_display_content("text", &json!({ "text": "你好" })),
            "你好"
        );
    }
}
