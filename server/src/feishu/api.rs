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
        }
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
        resp.data
            .and_then(|d| d["message_id"].as_str().map(|s| s.to_string()))
            .ok_or_else(|| anyhow!("飞书回复响应缺少 message_id"))
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
        resp.data
            .and_then(|d| d["message_id"].as_str().map(|s| s.to_string()))
            .ok_or_else(|| anyhow!("飞书发送响应缺少 message_id"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base_url_is_feishu() {
        assert!(super::FEISHU_BASE_URL.contains("feishu"));
    }
}
